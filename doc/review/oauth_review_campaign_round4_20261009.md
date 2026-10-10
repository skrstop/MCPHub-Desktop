# OAuth 复核战役 第四轮（2026-10-09，W1 亲核修复 + W2 三代理 + 断言审计 + 全量回归）

> 本轮为用户「50 轮独立复核」战役的第 4 轮。前 3 轮见 round2/round3 报告。

## 1. 覆盖面

- **W1 亲核**：第 3 轮全部修复的 fix-of-fixes 复审 + db/migration/lib/auth 全量 + 前端 OAuth 终态。
- **W2 三代理**：①rmcp 迁移清理全新仓 grep 终审；②全部 60 套 E2E 断言质量审计（同义反复/弱断言/吞 PASS）；③rmcp_bridge.rs 2291 行 + smart_routing 全 6 文件逐行真读。

## 2. 修复清单

### W1（8 项）
1. **[H] oauth_provider reaper 丢 `AND dynamic = 1` 子句** —— 第 3 轮 python replace 空白错位致子句静默消失（编译不报错），reaper 会误删手动注册的过期 client → 修复 + grep 验证。
2. **[H] redact_args_for_export 值位泄漏** —— redact_next 分支 `continue` 跳过内联 `=` 检查，`--header --api-key=sk` 形态泄漏明文 → 重构为「非 flag 值位打码 + 落入内联检查」；补 `-H`/`-a` 短 flag。
3. **[H] user_service delete_by_username 嵌套 BEGIN** —— `pool().begin()` 内再发裸 `BEGIN IMMEDIATE` 恒失败（**删除操作此前每次都失败**）→ `begin_with("BEGIN IMMEDIATE")`。
4. **[M] migrate_v21 rag_tag_stats 回填非原子** —— count 失败 unwrap_or(0) 后无条件 DROP 毁源表 → BEGIN IMMEDIATE 事务包 count+INSERT+DROP + 错误传播。
5. **[M] en.json dynamicRegistrationAuthDescription 是 JSON 数组**（英中拼接，脚本写 tuple 所致）→ 单字符串。
6. **[H] clientTtl UI 默认 2592000 vs Rust 0=disabled** —— SettingsContext + SettingsPage 两处默认改 '0'。

### W2 代理（4 项 Rust + E2E 断言）
7. **[L] meta.rs live 列表被 toolDefinitionFields 门控** —— `[]`（仅核心四字段）时描述覆盖刷新被跳过（回到 stale 索引快照，恰是本修复要解决的）→ live 列表抓取不再以 tool_fields 非空为条件。
8. **[L] rmcp_bridge pin 名歧义序依赖** —— `a` 与 `s-a`（sep=`-`）双匹配取 pin 向量首序 → 两遍解析：裸名精确匹配恒优先。
9. **[L] rmcp_http_transport.rs:2 死文件引用注释**（http_transport.rs 已删）→ 更新。
10. **[L] 死赋值/无用 mut 清理**（config.rs redact_next 双赋值、http_server.rs body）。

### E2E 断言审计（4 High + 6 Medium/Low 修复）
- **[H] carpet G1 requireState 恒真**（`A and B or A` ≡ `st==302`，且查 body 不查 Location）→ 断言 Location 头含 `error=invalid_request`。
- **[H] oauth_provider P1b protected-resource 同义反复**（`or len>=1` 使任意非空列表通过）→ 只留自指 issuer 精确相等。
- **[H] oauth_provider P6 `st != 401`**（500 也过）→ 断言 ∈ {200, 400}（400=session-required，证明鉴权门已过）。
- **[H] oauth_upstream 连接检查 `st < 600` 恒真** → 断言具体错误面。
- **[M] v30-v40 公网 IP 断言裸 regex 接受 loopback/私网** → 全部接 ipaddress 语义校验（排除 private/loopback/link-local/reserved）。
- **[M] r460 严格缺 Accept 检查带 `or error` 旁路** → 只认 4xx；**版本回显** → 逐版本精确 `echoed == v`（2026 白名单唯一文档化 fallback 2025-11-25）。
- **[M] v32 bearer 面接受 404** → 只认 401/403；r158 D0 环境自检改 info 不计 PASS；r461 死代码清理；oauth_upstream PKCE 重复断言去重。

### 证伪/记录
- rmcp 清理终审：旧协议栈符号全仓零代码残留；invoke_handler 双向 diff 通过；Cargo 依赖无死项。
- bridge/smart 复核：fail-closed 三 lane、esc_like 四字符、merge_hits 确定性、scrypt 参数、TTL min(5000-elapsed) 全部验证一致；睡眠 on-demand pin「可调不可列」为已文档化边界（记录不修）。

## 3. 验证

- cargo check 0 错 0 警；cargo test --lib **119 passed**；tsc 0 错误。
- **60 套 E2E 全绿**（exit-code 判定，两轮全量）+ 8 套重点复跑确认。

## 4. 复盘：为什么总能跑出问题

1. **字符串替换修改必须 grep 验证**：W1 的 reaper 子句丢失是连续第二轮 python replace 空白错位（第一轮改错、本轮修错时又错）——SQL 子句丢失不产生编译错误，纯靠 grep/单测兜底。
2. **sqlx 事务 API 语义**：`begin()` 急切发 BEGIN，内部再裸发 BEGIN 恒失败——「代码看着对」但每调用必败，只有运行时路径覆盖才暴露。
3. **断言同义反复是慢性病**：`A and B or A`、`st < 600`、`st != 401`、`or len>=1` 四种形态全部存活多轮——修复时只验证「有断言」，未验证「断言能否为假」。**判据：写下每个断言时自问「这个条件为假时测试会不会红」。**
4. **UI 默认值必须逐字镜像 Rust 契约**（2592000 vs 0——一个是「30 天」一个是「禁用」，语义相反）。
5. **收敛态势**：W2 代理在 2291+6000 行真读中仅出 2 Low（均为本轮前新改动区），存量逻辑面已连续两轮零 High/Medium——质量收敛成立，新改动区仍是缺陷主产地。

## 5. 基线同步涉及功能、范围与影响点（本轮回看）

| 功能 | 范围 | 影响点 |
| --- | --- | --- |
| OAuth Provider | oauth_provider.rs / http_server hub 路由 / oauth_clients v33.dynamic | consent POST scope 门、hub_admin_gate skipAuth、scope_allowed 真值表、reaper dynamic-only、clientTtl 契约、redact 导出打码 |
| OAuth 上游客户端 | oauth_upstream 链 / servers.oauth 列 | challenge 捕获、PKCE、刷新竞态补偿、断连清理 |
| Smart 六字段 | meta.rs attach / Tool 模型 / 前端复选框 | live 列表抓取门控（本轮修）、渐进披露剔除 |
| pin（服务器级+组级） | server_tool_config.pinned / bridge resolve | 裸名 vs 前缀两遍解析（本轮修）、睡眠 on-demand 边界 |
| rmcp 迁移 | 全仓 | 清理零残留终审通过 |
