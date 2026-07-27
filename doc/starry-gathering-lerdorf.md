# MCP 多版本协议支持(策略模式)

## Context

当前 `http_server.rs` 的 `/mcp` 端点**只实现了 Streamable HTTP(2025-03-26 风格)**。SSE 端点(`GET /mcp`)只发 keep-alive 心跳,从不推送业务数据 —— 所以**老式 2024-11-05 SSE-only 客户端连上后永远拿不到响应**(本次"用 sse 方式不生效"的根因)。版本差异(`MCP_PROTOCOL_VERSION` 常量、annotations/structuredContent 透传)全部**内联硬编码**,新增版本要在多个地方改 if/else,不可扩展。

目标:用**策略模式**让桌面端同时服务 2024-11-05 / 2025-03-26 / 2025-06-18 / 2025-11-25 四种客户端,版本差异集中在策略 trait 里,**版本选择走注册表查找而非 if/else**,加新版本只需加一个 struct + 注册。规范细节已用 shell 从 modelcontextprotocol.io 抓取核对。

## 设计:策略 trait + 注册表

### 新模块 `src-tauri/src/services/mcp_version.rs`

```rust
pub enum TransportMode { StreamableHttp, LegacySse }

/// 一个 MCP 协议版本的策略。所有版本差异集中在此,默认实现=2025-03 行为。
pub trait VersionStrategy: Send + Sync {
    fn version(&self) -> &'static str;                              // "2025-03-26" 等
    fn transport(&self) -> TransportMode { TransportMode::StreamableHttp }
    fn requires_version_header(&self) -> bool { false }            // 2025-06+ true
    fn capabilities(&self) -> Value { json!({"tools":{},"prompts":{},"resources":{}}) }
    fn shape_tool(&self, t: Value) -> Value { t }                   // 2024 strip annotations/outputSchema
    fn shape_tool_call_result(&self, r: Value) -> Value { r }      // 2024 strip structuredContent
    /// 处理共享分派器不认识的方法(如 2025-11 的 tasks/*)。返回 None 落到 -32601。
    fn handle_extra_method(&self, _m: &str, _ctx: &MethodCtx) -> Option<Response> { None }
}

/// 协商:按客户端请求的版本查注册表,找不到回退到 2025-03-26。
fn strategy_for(requested: &str) -> &'static dyn VersionStrategy {
    STRATEGIES.iter().find(|s| s.version() == requested).map(|s| s.as_ref())
        .unwrap_or(&V2025_03_26)
}
```

`MethodCtx` 携带 `dispatch_mcp` 预计算的共享状态(scope、bearer_key、name_sep、session_id、params、id、client_ip),作为 `&MethodCtx` 传给 `handle_extra_method`(以及若需要的共享 handler)。各策略是 `Send + Sync` 的静态单例,放 `OnceLock<Vec<Box<dyn VersionStrategy>>>` 注册表。

**扩展点**:加未来版本 = 写一个 struct impl `VersionStrategy`(只 override 差异点)+ 在注册表加一行。无 if/else、无修改 dispatch_mcp 主体。

### 各版本策略(Step 2 落地,Step 1 先建骨架 + 两个)

| 版本 | transport | requires_version_header | 差异 override |
|---|---|---|---|
| 2024-11-05 | `LegacySse` | false | `shape_tool`/`shape_tool_call_result` 剥 2025 字段 |
| 2025-03-26 | StreamableHttp | false | 默认(passthrough) |
| 2025-06-18 | StreamableHttp | **true** | enforce `MCP-Protocol-Version` 头 |
| 2025-11-25 | StreamableHttp | true | `handle_extra_method` 处理 `tasks/get\|result\|list\|cancel` → 因上游未声明 `taskSupport` 返回 `-32601`(experimental,代理不实现任务状态机) |

### 每会话状态(新)

- `SESSION_STRATEGY: OnceLock<RwLock<HashMap<String, &'static dyn VersionStrategy>>>` —— session_id → 协商到的策略。initialize 时写入,DELETE 时清除。仿 `session_pool.rs:45` 的 `OnceLock<Arc<RwLock<HashMap>>>` 模式。
- `SSE_CHANNELS: OnceLock<RwLock<HashMap<String, mpsc::UnboundedSender<Value>>>>` —— session_id → SSE 推送通道,仅 legacy 会话用。仿 `app_logger.rs` 的 `mpsc::UnboundedSender` 模式。

## 分两步稳健交付(每步独立可重建验证)

### Step 1 — 策略骨架 + 2024 老 SSE 传输(修复 sse 不生效)

1. 新建 `services/mcp_version.rs`:trait + `TransportMode` + `MethodCtx` + 注册表 + 两个 impl(`V2024_11_05`、`V2025_03_26`)。
2. `http_server.rs`:
    - `dispatch_mcp` 的 `initialize`:读 `params.protocolVersion` → `strategy_for()` → 存入 `SESSION_STRATEGY` → 响应 `strategy.version()`/`strategy.capabilities()`/serverInfo(去掉硬编码 `MCP_PROTOCOL_VERSION` 常量)。
    - `dispatch_mcp` 的 `tools/list`:`strategy.shape_tool(entry)` 包裹每个工具(替代当前内联条件透传,http_server.rs:688-697)。
    - `dispatch_mcp` 的 `tools/call`:`strategy.shape_tool_call_result(call_resp)` 包裹响应(http_server.rs:810 区)。
    - `dispatch_mcp` 的 `_` 通配:先 `strategy.handle_extra_method(method, ctx)`,None 才 -32601。
    - 新增 legacy SSE GET handler(挂在 `/mcp` GET,`Accept: text/event-stream` 时):生成/复用 session_id → 存策略为 `V2024_11_05`(或由客户端请求版本定)→ 发 `endpoint` event(指向 `/mcp/message`)→ `mpsc` 通道循环把收到的 Value 推成 SSE `message` event → 注册到 `SSE_CHANNELS`。保留现有 keep-alive。
    - 新增 `POST /mcp/message`(静态路由,优先于 `/mcp/*path` 通配):读 `sessionId` query → 从 `SSE_CHANNELS` 取通道 → `dispatch_mcp` 算出结果 → 推 `message` event + 返回 202(legacy);无 sessionId 走原逻辑。
    - DELETE 清 `SESSION_STRATEGY` + `SSE_CHANNELS`(http_server.rs:977-1007)。
3. import 加 `crate::services::mcp_version` + 在 `mod.rs`/`lib.rs` 注册新模块。

**验证**:2024 SSE 客户端 GET /mcp → 收到 `endpoint` event → POST /mcp/message?sessionId=… → 通过 SSE `message` event 收到 tools/list 响应。2025 客户端 POST 不变(JSON 响应),回归无破坏。

### Step 2 — 2025-06 / 2025-11 策略

1. `mcp_version.rs` 加 `V2025_06_18`(`requires_version_header()=true`)和 `V2025_11_25`(`handle_extra_method` 处理 `tasks/get|result|list|cancel` → -32601;capabilities 可声明 tasks experimental,但不实现状态机)。
2. `dispatch_mcp`:若 `strategy.requires_version_header()` 且请求带 `MCP-Protocol-Version` 头,校验版本,无效回 400(规范要求);无头则按 2025-06 规范假定 2025-03-26(向后兼容,不破坏现有客户端)。
3. 注册表加入两个新策略。

**验证**:2025-06 客户端带 `MCP-Protocol-Version: 2025-06-18` 握手通;缺头时回退不报错。2025-11 客户端 initialize 拿到 2025-11-25 协商;调 `tasks/get` 得 -32601(因上游无 taskSupport)。

## 关键复用(避免新造)

- 每 session 状态存储 → 仿 `mcp/session_pool.rs:45`(`OnceLock<Arc<RwLock<HashMap<...>>>>`)。
- mpsc 通道 → 仿 `services/app_logger.rs:7`(`OnceLock<mpsc::UnboundedSender>`,后台 task 接收)。
- 静态路由优先于通配 → axum 静态段优先匹配,`/mcp/message` 与 `/mcp/*path` 共存无冲突。
- 共享方法分派(`tools/list` 等)主体逻辑保留在 `dispatch_mcp`,仅在其出口套策略 hook —— 不重写整条 match。
- `jsonrpc_response`/`jsonrpc_error`(http_server.rs:408/422)、`new_session_id`(http_server.rs:48)、`extract_session_id`(http_server.rs:1010)、`mcp_scope_server_filters`(http_server.rs:509)继续复用。

## 风险与限制

- **沙箱编不了**(Rust 1.83 / Cargo.lock 需 1.85+ edition2024)。每步交付后需你用 ≥1.85 工具链重建重启,我端到端 curl 验证。我对照模块字段/axum API/serde 仔细检查,但不能打包票编译通过 —— 重建报错你贴出来我对着改。
- Step 1 改动面大(新模块 + 两个全局 store + SSE 通道 + 新路由 + legacy POST 推送),是主要风险点;Step 2 主要是加 struct + 一个头校验,风险低。
- legacy SSE 的并发:同一 session 的 POST 推送与 GET 流接收靠 `mpsc::UnboundedSender`,GET 端断开时通道自然失效,POST 端 send 失败可忽略(仿 app_logger)。

## 验证(end-to-end,重建重启后)

```bash
# 2024 老 SSE 客户端模拟
curl -N http://192.168.120.47:23333/mcp -H 'Accept: text/event-stream'   # 应见 endpoint event + keep-alive
# 用 endpoint 指向的 /mcp/message POST initialize/tools/list,从同一条 SSE 流收 message event
# 2025 客户端不变
curl -s -X POST http://192.168.120.47:23333/mcp -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26",...}}'
# 2025-06 带版本头
curl ... -H 'MCP-Protocol-Version: 2025-06-18' ...
# 2025-11 tasks/get → -32601
```
