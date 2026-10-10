# OAuth 基线复核战役第三轮（2026-10-09 深夜，W1-W2 六代理 + 全量回归）

> 第二轮见 `oauth_review_campaign_round2_20261009.md`。本轮 = 二轮修复自身复审 + scope 链全程追踪 + 配置持久化/导出链 + 双轨鉴权链。

## 一、修复清单（11 项：C×1 H×4 M×4 L×2）

| 级 | 发现 | 修复 |
|---|---|---|
| C | **consent POST 完全绕过 scope 校验**——GET 页有 scope_allowed，POST 的 form.scope 直达 store_code→issue_tokens；nonce 是 CSRF 门不是 scope 门（隐藏字段可被请求者改写）。受限配置下 `scope=mcp:all` / `scope=` 一发入魂 | POST 分支补 scope_allowed 强制 |
| H | **空 scope 逃逸**：受限配置下 `scope=`（空白/空）→ scope_allowed 恒 true（空 items 短路）→ token 落空 scope → scope_to_allowed_servers("")→None=无限制 | 空白 scope 在受限配置下拒绝（unrestricted 才放行） |
| H | **合法空 allowed set 被塌缩成 "all"**：scope 命中空成员组 → Some({}) → check_bearer_auth 的 `unwrap_or_default()+is_empty()→"all"` → 全权限 | None=无限制 / Some(含空)=deny-all 二分 |
| H | **clientTtl 回收器误删预注册客户端**——oauth_clients 无 dynamic 标记，回收条件只看"无 token+超龄"，预注册集成未取 token 即被静默注销 | **migration v33**：oauth_clients 加 `dynamic` 列；register_client 写 1；reaper 只回收 dynamic=1 |
| H | hub_admin_gate 的 skipAuth 缺省 false 与全仓 true 矛盾（默认部署从不写该键 → 管理路由恒 401）——二轮修复自身引入 | 缺省对齐 true |
| H | hub 管理路由丢弃 BearerKey——servers/groups 限定的 key 可操作任意服务器的 OAuth（凭据驱逐） | server_scope_gate：get_allowed_servers 按 {server} 校验，越界 403 |
| M | unrestricted 配置下 "read mcp:all" 从放行变 invalid_scope——存量客户端破坏性变更 | unrestricted + 显式 mcp:all → 整体放行（存量语义，无逃逸面） |
| M | clientTtl UI 承诺的回收功能不存在（locale 文案 vs 零消费者） | 实现 reaper（cleanup_expired 内：dynamic=1 + 无任何 token 行 + 超龄；0=禁用） |
| M | requiresAuthentication 文案承诺「认证后可注册」实为关死——误导 | 四语言文案改为「关闭动态注册，请用预注册客户端」 |
| M | 导出文件 args 明文携带凭据（`--header "Authorization: …"` / `--api-key sk-…` 是远程 MCP 标准形态） | 导出路径 flag-scan 脱敏（复制路径有意保留——用户粘贴自己的配置需要可用参数） |
| L | refreshTokenLifetime 前端默认 14d vs Rust 回退 30d——UI 显示与运行时背离 | Rust 回退对齐 14d |
| 记录 | ①bearer_keys 明文存储 vs OAuth token SHA-256（存量债，需迁移+强制重发，单独排期）；②导出→导入 groups 静默丢弃（R52 裁定的功能增强待办）；③/rest POST 中间件+handler 双重鉴权（同结果纯性能，~2×DB 往返）；④在途授权中的客户端超龄可被回收（符合「无存活 token」文档语义） | 不修（记录） |

## 二、E2E 增补与回归

- **carpet_r300 +2 用例（122 项）**：受限配置 × 空/空白 scope → invalid_scope（空 scope 逃逸回归钉死）。
- **验证终态**：cargo test --lib **119 passed**；tsc 0 错；build ✓；**59 套 E2E 全 PASS**（含 carpet 122、oauth_provider 35、oauth_upstream 11）。
- expires_at=0 轮换补偿显式核验为安全（expires_at 仅 access/refresh_used 消费，refresh 行 TTL 由 refresh_expires_at 治理）。

## 三、复盘：为什么每次都能跑出问题（第三轮答案）

1. **GET 校验 ≠ POST 校验**：同一端点的两个 handler 各自带可选校验，漏一半 = 全漏（consent POST 是 Critical 的根源）。「校验必须落在不可绕过的单点」——最好在 store_code 层强制，handler 层只做 UX。
2. **修复在错误层生效**：scope_allowed 修在函数层，但真正发 token 的路径（POST→store_code）根本不调用它——测试绿≠链路绿，必须按数据流逐 hop 验证。
3. **None vs Some(empty) 的语义塌缩**：`unwrap_or_default()+is_empty()` 把「无限制」和「合法的空集合」合并成同一个分支——Option 语义在 unwrap 时死掉，与 bearer key 侧 get_allowed_servers 的正确二分不一致。
4. **UI 承诺先行于实现**：clientTtl 文案承诺回收功能而 Rust 零消费者——写 locale 文案前先确认运行时真的有对应行为。
5. **缺省值不一致是沉默的回归**：skipAuth 默认 true（auth.rs）vs 管理路由缺省 false——同一默认在 N 处重复书写，必须在单一 helper 收口（本轮已改为各读各的但仍建议后续统一）。

## 四、基线同步涉及功能/范围/影响点（三轮增量）

- **范围**：OAuth Provider 授权全链（GET+POST+token+check_bearer）、clientTtl 生命周期、requiresAuthentication 语义、导出脱敏、迁移 v33。
- **影响点**：①受限 allowedScopes 下空 scope 现在明确拒绝（此前=全权限）；②unrestricted 下 "read mcp:all" 保持放行（兼容）；③空成员组的 scope 现在拒绝所有调用（此前=全权限）；④开启 clientTtl 后预注册客户端不再被回收；⑤导出文件的 args 中敏感 flag 值变为 ***REDACTED***（导入后需手填）；⑥**用户需重启**（v33 迁移 + 路由/语义生效）。
