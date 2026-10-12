# OAuth 实施计划（v1.1.1004）

> 2026-10-08 立项。两大功能：
> - **A. 服务器级 OAuth**：MCPHub 作为 OAuth *客户端*，对接受 OAuth 2.1 保护的上游 MCP 服务器（Streamable HTTP / SSE / OpenAPI）。
> - **B. OAuth Provider**：MCPHub 作为 OAuth *授权服务器*，让第三方 AI 客户端（Claude 等）通过 MCP 规范 OAuth 2.1 接入 `/mcp` 与 `/mcp/$smart`。
>
> **非目标**：登录 SSO（Better Auth/OIDC 登录）——本地单用户工具，保持账密 + skipAuth。origin 的 Better Auth 链路继续维持桌面隐藏。

---

## 0. 现状与差距

| 项 | 现状 | 差距 |
| --- | --- | --- |
| 前端 OAuth UI | ServerForm 已有完整 OAuth 区块（clientId/secret/scopes/endpoints/tokens/allowInsecureTokenEndpoint #1293 镜像） | 仅 UI 收集，无触发授权流程按钮、无状态显示 |
| Rust ServerConfig | **无 oauth 字段**（serde 丢弃，R10 待办） | 需补结构体 + round-trip + 连接相关性 |
| 上游 token 注入 | streamable-http 仅静态 auth_header/headers；SSE 手写 transport；openapi 有 default_headers | 均无 OAuth 流程（发现/授权/刷新） |
| 下游鉴权 | Bearer Key（token 哈希表）+ `/.well-known/oauth-protected-resource` 占位端点（authorization_servers 指向自身但无授权服务器实现） | 无 `/oauth/*` 端点、无动态注册、无 token 签发 |
| rmcp 3.4.1 | 自带 `auth` feature（oauth2 5.0、PKCE、RFC 7591、挑战式发现、`AuthRequired` 错误带 www_authenticate） | 未启用；streamable_http_client 仅静态 auth_header，动态流程需我们在 transport 层驱动 |
| origin 参考 | oauthService/oauthClientRegistration/oauthCallbackController（上游 A）；mcpOAuthProvider/oauthServerController/oauthClientController/oauthDynamicRegistrationController（下游 B），共 ~4400 行 TS | 按语义移植到 Rust/axum，不逐行翻译 |

---

## A. 服务器级 OAuth（上游客户端）

### Phase A1 — 数据模型与迁移（v30）

- `models/server.rs`：`ServerConfig` 增加 `oauth: Option<ServerOauthConfig>`，字段对齐 origin + 前端既有类型：
  `allow_insecure_token_endpoint, client_id, client_secret, scopes(Vec<String>), access_token, refresh_token, authorization_endpoint, token_endpoint, resource, redirect_uri`。
  serde camelCase；`to_connection_relevant` 纳入 oauth（oauth 变更 → 重连，#1055 语义）。
- settings_import / 导出：透传 + 凭证脱敏（client_secret/access_token/refresh_token 打码，对齐 R10 导出脱敏惯例）。
- **DB v30**：`oauth_upstream_tokens` 表
  ```sql
  server_name TEXT PRIMARY KEY,
  issuer TEXT,                      -- authorization server issuer
  client_id TEXT, client_secret TEXT,
  scopes TEXT, resource TEXT,
  access_token TEXT, refresh_token TEXT,
  access_expires_at INTEGER, refresh_expires_at INTEGER,
  status TEXT,                      -- connected | pending | error | disabled
  pending_authorization_url TEXT, pending_state TEXT,  -- oauth_required 透出
  updated_at INTEGER
  ```
  （token 明文存储——与 BearerKey 明文存量债一致，记录已知债。）
- 迁移五步 SOP：TARGET_VERSION=30、migrate_v30、apply_migration 分支、000N sql、文档。

### Phase A2 — OAuth 客户端核心（`services/oauth_upstream.rs` 新模块）

**P0 技术选型 spike 结论（2026-10-08，已完成）**：**直接采用 rmcp 3.4.1 `auth` feature**。`AuthorizationManager` 内含 challenge 发现（RFC 9728 → 8414 → 端点合成）、CIMD/预注册/RFC7591 注册三级选择、PKCE S256、proactive 刷新（REFRESH_BUFFER）、`StoredCredentials`/`StoredAuthorizationState` 可 JSON 序列化。坑位记录：①`AuthorizationMetadata` non_exhaustive 需 `Default::default()` + 字段赋值；②`StateStore` trait 方法名是 `load`/`delete`（非 load_state/delete_state）；③`OAuthState::handle_callback_url(&mut self)` 挂在 state 上不在 manager；④`AuthorizationManager` 不可 Clone → 完成任务从 FLOWS 注册表 take 所有权；⑤oneshot 不可 clone → Arc<Mutex<Option<Sender>>> 槽位单次投递。

流程实现（无论选哪条路，语义一致，对齐 origin + MCP spec）：

1. **挑战式发现**：上游 401 + `WWW-Authenticate: Bearer resource=...` → 拉 `/.well-known/oauth-protected-resource` → `/.well-known/oauth-authorization-server`；手填 endpoint 配置存在时跳过发现（静态配置模式）。
2. **RFC 7591 动态注册**：无 clientId 时 POST `/register`（origin oauthClientRegistration 语义：metadata、software_id、失败友好报错）。
3. **授权码 + PKCE**（S256，强制）：生成 state + verifier → 构造授权 URL（含 `resource` RFC 8707 防令牌混用）→ `opener` 开系统浏览器 → **临时 loopback 监听**（`http://127.0.0.1:{ephemeral}/callback`，RFC 8252；监听仅在 pending 期间存在；同时支持用户手填 redirect_uri）→ code 交换 token（`allowInsecureTokenEndpoint` gate 在此生效，#1293 镜像）。
4. **client_credentials 授权**：有 clientId+secret 无授权端点时直取 token（无 refresh）。
5. **静态 token 模式**：仅手填 accessToken → 直接注入（现有行为升级）。
6. **刷新**：proactive（过期前 60s）+ reactive（401 后单次刷新重试，防循环）。
7. **`oauth_required` 状态机**（origin mcpService.ts:2029/2235 语义）：连接遇 401 → server 状态置 `oauth_required` + 存 pending_authorization_url/state → 事件 `server://oauth-required`（camelCase payload）→ 前端按钮触发授权 → 完成后自动重连。

### Phase A3 — 传输接线（`mcp/pool.rs` + 传输层）

- `build_client` 三分支注入：streamable-http（auth_header 动态取 token store）、SSE（手写 transport 加 Authorization header）、openapi（default_headers 合并）；refresh 失败 → 传播 `oauth_required`。
- 前置 token 解析在 connect 锁内完成；pending 授权期间保存/重连不阻塞（复用 starting 后台 spawn 模式）。
- `upstreamOAuthDisconnect` 语义：清 token 行 + 状态复位（origin upstreamOAuthDisconnectService）。

### Phase A4 — 前端 + Tauri 命令

- 命令（均 require_admin）：`oauth_connect(server_name)`（触发发现+授权，返回授权 URL 供嵌入 webview 场景）、`oauth_callback_received(server_name, code, state)`（loopback 由 Rust 侧监听，此命令备用）、`oauth_disconnect(server_name)`、`get_server_oauth_status(server_name)`。
- ServerForm：OAuth 区块增加「通过 OAuth 连接 / 断开」+ 状态徽标（connected/pending/error）；编辑已有 token 的服务器显示脱敏状态。
- ServerCard：`oauth_required` 状态角标 + 一键授权按钮（监听 `server://oauth-required`）。
- ServerContext 轮询/oauth 事件合并；types 增加 `Server.oauth`（authorizationUrl/state/connected，origin 既有前端契约已在 types 中预留）。
- locales：`server.oauth.connect/disconnect/required/success/error` ×4 语言。

### Phase A5 — 测试

- 单测：挑战头解析、PKCE 往返、token 过期判定、allowInsecureTokenEndpoint gate、动态注册 metadata 形状。
- E2E（新套件 `scripts/e2e/oauth_upstream_e2e.py`）：Python mock 授权服务器 + 401 挑战 MCP 上游（aiohttp），真实走完 发现→注册→授权(loopback)→token→连接→调用→401 触发刷新→断开；负路径：deny、过期 code、错 state。

---

## B. OAuth Provider（下游，Claude 等客户端接入）

> 前提约束：**仅在 `exposeHttp=true`（内置 HTTP 服务器开启）时可用**——外部客户端要能到达 hub。关闭时端点返回 404 + Settings UI 明示依赖关系。

### Phase B1 — 配置与迁移（v31）

- system config `auth.oauthServer`：`{ enabled: bool(false), accessTokenLifetimeSec(3600), refreshTokenLifetimeSec(30d), requirePkce: true(恒 true, OAuth 2.1) }`。
- DB v31 三表：
  - `oauth_clients`：client_id、client_secret_hash、name、redirect_uris(JSON)、scopes、created_at、revoked
  - `oauth_tokens`：access_hash、refresh_hash、client_id、scope、expires_at、refresh_expires_at、resource
  - `oauth_codes`：code_hash、client_id、challenge、challenge_method、scope、resource、redirect_uri、expires_at(60s)

### Phase B2 — 端点（http_server.rs 新 `/oauth/*` 路由组）

- `GET /.well-known/oauth-authorization-server`：issuer=`http://{host}:{httpPort}`，authorize/token/register 端点、`code_challenge_methods_supported:["S256"]`、`grant_types`、scopes。
- `POST /oauth/register`：RFC 7591 动态注册（Claude 必需）——返回 client_id/secret；redirect_uri 白名单校验。
- `GET /oauth/authorize`：**自包含 consent HTML**（简化移植 origin generateAuthorizeHtml：i18n、客户端名、请求 scope、RFC 8707 resource 目标展示、Approve/Deny 按钮）——本地单用户无需跳主窗口，浏览器内直接完成。校验 client_id/redirect_uri/state/PKCE。
- `POST /oauth/authorize`：approve → 302 `redirect_uri?code&state`；deny → `error=access_denied`。
- `POST /oauth/token`：`authorization_code`（PKCE S256 校验）+ `refresh_token` 两 grant；常量时间哈希比对（复用 bearer_key_service 模式）。
- **改写既有 `/.well-known/oauth-protected-resource`**：enabled 时 `authorization_servers` 指向 issuer；关闭时保持现状占位。

### Phase B3 — 下游鉴权集成

- `/mcp*` 与 `/mcp/$smart`：Authorization Bearer **双轨**——先查 `oauth_tokens`（命中→按 scope 映射 allowed_servers/allowed_groups），再查 bearer key；均未命中 401（WWW-Authenticate 带 resource，MCP spec 形状）。
- scope 语义：`mcp:all` 全量；否则空格分隔服务器/组名（复用 bearer `allowed_servers/allowed_groups` 门控与组 gate、pin、smart 过滤全链——**不新开一套过滤逻辑**）。
- 活动日志 source 标 `oauth:{client_name}`；cleanup 复用既有日志清理。

### Phase B4 — 前端

- SettingsPage「OAuth Server」区块解除隐藏（桌面化改造：无 issuer 可编辑——恒为内置 HTTP 地址；enabled 开关 + lifetime 展示 + exposeHttp=false 时禁用并提示）。
- 客户端管理：已注册客户端列表（名称/创建时间/scope）+ 撤销；撤销即吊销其全部 token。
- AccessUrlDialog：增加 OAuth 接入说明行。
- Tauri 命令：`list_oauth_clients / revoke_oauth_client / oauth_server_status`（require_admin）。

### Phase B5 — 测试

- E2E（新套件 `scripts/e2e/oauth_provider_e2e.py`）：模拟 Claude 客户端全链——discovery → 动态注册 → authorize（HTTP 层 approve）→ token → `/mcp` initialize + tools/call 真实调用 → `$smart` 通道 → refresh → revoke 后 401；负路径：错 PKCE、过期 code、错 redirect_uri、未启用 404、scope 外服务器 403。
- 回归：全部 53 套既有 E2E 零回归（重点：bearer 中间件层序、`/mcp` 401 形状、loopback 劫持检查）。

---

## C. 风险与决策点

| # | 风险/决策 | 处置 |
| --- | --- | --- |
| 1 | rmcp `auth` feature 与桌面浏览器流适配性未知 | Phase A2 开头 spike 半天，结论记录本文件 |
| 2 | loopback 回调需供应商允许 `http://127.0.0.1` | RFC 8252 标准做法（Claude/GitHub/Google 均支持）；另留用户手填 redirect_uri |
| 3 | token/secret 明文落 SQLite | 与 BearerKey 明文一致，记已知债（后续可 keyring/加密批次） |
| 4 | Feature B 依赖 exposeHttp | 强门控 + UI 明示；`/.well-known/*` 关闭时行为不变 |
| 5 | Claude 对 OAuth 2.1 强制 PKCE + 动态注册 | B2 已按强制设计（PKCE 恒开、RFC 7591 必备） |
| 6 | 与 bearer key 的 401 语义冲突 | 双轨查询顺序固定（oauth 先、bearer 后），401 头带 resource 指向我们的 issuer——测试钉死 |
| 7 | oauth 字段入 `to_connection_relevant` 后改描述误断连 | 仅 oauth 子字段变更比对，遵循 #1055 strip nulls 深比对 |
| 8 | 迁移 v30/v31 需 fresh-DB 单测 | 按 R11 教训：in-memory 全量 run_pending + 直插直查 |

## D. 顺序与交付

1. **A1→A5**（上游客户端，用户价值最高，~3 天）：A1 模型/迁移 → A2 spike+核心 → A3 接线 → A4 UI → A5 测试。
2. **B1→B5**（下游 Provider，~2.5 天）：B1 迁移/配置 → B2 端点 → B3 鉴权 → B4 UI → B5 测试。
3. 收尾：changelog `upgrade/1.1.3010.md`（OAuth 为主功能）、AGENTS.md §3 新节 + §6 待办、locales、53 套全量回归 + 2 新套件。

每 Phase 完成即 cargo check / cargo test --lib / tsc / 相关套件增量验证；两 Feature 落地后按惯例做独立复核轮（修复自身引入的 bug 视角）。
