# OAuth 基线复核战役（W1-W3 + 地毯式 E2E，2026-10-08）

## 一、代码级复核修复清单（9 代理 × 3 波，12 项缺陷）

| # | 级别 | 位置 | 问题 | 修复 |
|---|------|------|------|------|
| 1 | H | oauth_upstream.rs DbStateStore.load | 忽略 csrf_token（CSRF 绕过） | load 校验 csrf |
| 2 | M | oauth_upstream.rs | 流重启不取消旧任务 + 陈旧任务偷取消条目 | notify_one + ptr_eq 守卫摘除 |
| 3 | M | oauth_upstream.rs | notify_waiters 丢失未 poll 任务的取消 | notify_one（存许可） |
| 4 | H | oauth_provider.rs consume_code | SELECT-then-DELETE 非原子（重放窗口） | begin_with BEGIN IMMEDIATE |
| 5 | M | oauth_provider.rs refresh | 并发刷新双双成功（轮换竞态） | 条件 UPDATE gate，竞态败方=族吊销 |
| 6 | M | http_server.rs deny | open redirect（redirect_uri 在 deny 分支后才校验） | 先校验后分支 |
| 7 | H | 前端 6 个 UI 键 | requireClientSecret/requireState/allowedScopes/allowedGrantTypes/clientTtl 等 Rust 从不读取 | ProviderConfig 扩展 + effective_scopes/scope_allowed 接入 metadata/authorize/token/register |
| 8 | M | 前端默认值 | enabled:true 与 Rust false 矛盾；code lifetime 300 vs 60 | 对齐（false / 60 / []） |
| 9 | M | 前端 | 无 server://oauth-connected/error 监听；revoke 无确认框 | ServerCard 监听 + ConfirmDialog |
| 10 | H | smart_routing | resolve_pinned_target 拒绝 search 自身返回的前缀名（单服务器作用域） | 接受两种拼写 |
| 11 | H | on_demand.rs | 冷启动拿 connect lock 却从不 .lock()（引入即坏） | lock + run_call 前 drop |
| 12 | M | 导出链 | export_settings 丢 oauth 块；ServerOauthConfig 丢 dynamicRegistration/revocationEndpoint | 共享 redact helper + 两 Option 字段 |
| 13 | M | http_server REST | list_group_tools 传 "/mcp/{name}" 致组 allow-list 静默跳过；/rest 调用不写活动日志 | 裸组名 + write_activity 双 handler |

## 二、E2E 发现并修复的真实缺陷（地毯式测试抓出）

1. **[H] v30 迁移静默 no-op（R11 fresh-DB 教训再现）**：存量 DB 持旧形态 `oauth_upstream_tokens`（issuer/client_id 列），`CREATE TABLE IF NOT EXISTS` 永不重建，rmcp 凭据写入全部 `no such column: credentials_json` —— OAuth 授权流程必崩。修复：`migrate_v32` pragma 守卫重建 + v30 自身修复守卫 + 2 个单测（`v30_repairs_legacy_…` / `v32_repairs_existing_db_legacy_shape`）。
2. **[M] hub REST 缺 `/servers/{name}/oauth/{connect,disconnect,status}` 三路由**：tauriClient 有映射（origin REST parity）但 hub HTTP 面缺失 → 补齐（skipAuth 门控，Bytes 容空 body）。

## 三、套件修正（断言/环境类，非代码缺陷）

- carpet_r300：TTL 断言 30000 → 有界 ≤5000（#1277 freshness 契约）；2026 scope initialize 回显 2025-11-25 系 rmcp 规范回退；`$smart/{group}` 直呼已改为 pinned 门控（R110 同步变更）→ 套件先经 DB 打 pin；Idea 上游工具 isError 属 IDE 端环境态。
- oauth_upstream_e2e：mock AS 补 seed oauth 列；**rmcp 拒绝跨源 resource_metadata URL（安全策略）** → mock 上游同源服务 protected-resource；connect 须带 401 挑战（镜像前端真实流）。

## 四、复盘：为什么每次都能跑出问题

1. **新功能引入新面**：本轮 13 项修复全部位于新写的 OAuth 模块与同步新特性——存量经百轮已收敛，风险永远在"上一轮刚写的代码"。
2. **fresh-DB vs 存量-DB 盲区**：v30 旧形态表问题在存量开发机上静默存活（16+ 轮 E2E 全绿），一跑真实 OAuth 流即崩。E2E 无法扫到，只有"从零构造环境"的迁移单测能补。
3. **平台/环境条件分支本地不编译**：Linux cfg 分支（decode_limited）只在 CI 报错——跨平台代码必须 `cargo check --target` 或 CI 早跑。
4. **SDK 安全策略与 mock 不符**：rmcp 拒绝跨源 resource_metadata，mock 跨端口布局直接命中策略——写 mock 前必须读 SDK 的校验源码。
5. **断言腐化**：TTL 30000、$smart 直呼、display parity 等断言随同步语义变更失效——行为变更必须全量 grep 断言面。

## 五、基线同步涉及功能/范围/影响点

- **范围**：pin（服务器级+分组级+`$smart` 直呼门控）、oauth（上游客户端 12 字段配置 + Provider 全部 UI 配置项）、smart 动态字段配置（toolDefinitionFields 六字段 + fullSchemaTopN）、TTL freshness、组路由开关。
- **影响点**：OAuth 流程（新 REST 三路由 + v32 迁移——**用户需重启**使迁移生效）；E2E 断言契约（TTL/直呼/回显）。
- **验证终态**：cargo check 0 错 0 警；cargo test --lib **117 passed**（+3 新迁移单测）；tsc 0 错；build ✓；**60 套 E2E 全部 FAIL=0**（含 carpet_r300 120 项：5 版本 × 3 上游传输 × 4 通道真实公网 IP 调用 + 宽松/严格 + 2026 新特性 + OAuth 全配置项）。

## 六、记录不修

- 上游 OAuth token 过期无 mid-session refresh-on-401（仅 connect 时刷新，长连接到期后需手动重连）。
- clientTtl 为 UI-only 键（Rust 不消费，待产品决策）。
