# rmcp 官方 SDK 迁移 E2E 测试报告（下游 /mcp + 上游 stdio client）

**日期**：2026-09-27　**版本基线**：桌面端 `1.0.40003`　**rmcp**：3.4.1
**关联文档**：AGENTS.md §3.9.2（POC）、§3.9.3（下游切换）、§3.9.4（上游 stdio 切换）

---

## 1. 测试范围与策略

迁移分两步，每步独立回归：

| 阶段 | 改动 | 风险面 | 验证方式 |
|---|---|---|---|
| 下游 | `/mcp` 手写 JSON-RPC dispatch → 官方 `StreamableHttpService` + `HubBridge` | 协议行为、scope、Bearer、聚合 | 22 用例 HTTP E2E + 场景矩阵 |
| 上游 | stdio transport 手写 JSON-RPC → 官方 rmcp client（`RmcpStdioTransport`） | 全部 stdio 服务器连接/调用、下载进度、杀树、版本捕获 | 真机 stdio 场景 + 日志证据链 |

**未替换项（本轮决策）**：HTTP/SSE/OpenAPI 上游 transport 保持手写（A1 modern 探测 + post_modern + tasks 轮询 + MRTR 为深度定制，rmcp client 的 streamable-http 会被 initialize 握手绑定，对 2026 modern 上游反而退化——见 §6 遗留）。

## 2. 测试环境与流程

1. `cargo check` 0 警告 → `cargo test --lib`（91 passed / 0 failed / 4 ignored）
2. `touch src/lib.rs` 触发 tauri dev 重建，`ps -o lstart= -p $(lsof -tiTCP:23333)` 确认重启后测试
3. HTTP E2E：`/tmp/rmcp_e2e.py`（22 用例，全部报文留痕 `/tmp/rmcp_e2e_transcript.txt`）
4. 场景矩阵：curl 逐场景打真机，证据从 `app_log` 表核对
5. Bearer 矩阵：临时开启 `routing.enableBearerAuth`（直接改 DB config_json），测毕**还原为关闭**

## 3. 用例与结果（A：下游协议，22/22 全过）

| # | 用例 | 预期 | 结果 |
|---|---|---|---|
| A1 | GET /health / /servers 共存 | 原样 200 | ✅ |
| A2-A5 | initialize 协商 2024-11-05 / 2025-03-26 / 2025-06-18 / 2025-11-25 | 精确回显同版本 | ✅×4 |
| A6 | initialize 未知版本 1999-01-01 | fallback 2025-11-25 | ✅ |
| A7 | 2026 `server/discover` | supportedVersions 全 5 版本 | ✅ |
| A8 | session mint（initialize 响应头） | Mcp-Session-Id 返回 | ✅ |
| A9 | notifications/initialized | 202 | ✅ |
| A10 | tools/list 聚合 | 多服务前缀 + inputSchema | ✅（102 工具） |
| A11 | tools/call 真实调用（playwright network_requests） | isError:false + content | ✅ |
| A12 | 未知工具 | -32602 | ✅ |
| A13 | ping | `{}` | ✅ |
| A14 | DELETE session 后复用 | DELETE 202，复用 404 | ✅ |
| A15 | 2024 GET 无 session（endpoint 事件路径） | 400（已退役） | ✅ |
| A16 | /mcp/message（2024 POST 端点） | 4xx（已退役） | ✅ |
| A17 | 2026 modern tools/call（头+body _meta） | resultType:"complete" | ✅ |
| A18 | GET session SSE server-push 流 | 200 + text/event-stream | ✅ |

## 4. 用例与结果（B：上游 stdio rmcp client，全过）

| # | 用例 | 执行流程 | 预期 | 结果 |
|---|---|---|---|---|
| B1 | playwright（npx wrapper）连接 | 启动后自动 start_all | rmcp 握手成功、25 工具 | ✅ 日志 `Connected \| total=2.6s (rmcp stdio)` |
| B2 | codegraph（本地二进制 command）连接 | /mcp/codegraph scope tools/list | 8 工具 | ✅ |
| B3 | serverInfo.version 捕获 | 连接后更新检查 | 版本进入包更新检查 | ✅ 日志「服务自报版本 1.64.0-alpha…」「检测到新版本 0.0.79→0.0.82」 |
| B4 | tools/call 真实调用（经 rmcp client） | /mcp → tools/call playwright-browser_network_requests | isError:false + content | ✅ |
| B5 | 断连重连（close 工具后 tools/list） | 调用后重列 | 25 工具仍完整 | ✅ |
| B6 | 进程树存在（npx wrapper 子进程） | `ps` | node/npm 子进程在跑 | ✅ |
| B7 | 进程树清理路径 | 代码级审查 | kill_process_tree(pid) + service.cancel() 顺序保留 | ✅（代码审查，与旧实现同序） |
| B8 | stderr drain / 下载进度事件 | 代码级审查（npx 包已缓存，真机无法触发下载） | 逐行复制旧实现（looks_like_download_progress/parse_progress_pct/300ms 节流/32KB tail/2000 字符行帽） | ✅（行为等价，待真实首次安装时人工复验） |
| B9 | handshake 失败带 stderr tail | 代码级审查 | serve_client 失败 map_err 拼 stderr_tail | ✅ |
| B10 | MRTR 原生重试 | rmcp `Peer::call_tool` 内建 inputResponses 轮询 | 替代手写重试 | ✅（API 级确认） |

## 5. 特殊场景矩阵（C，全过）

| # | 场景 | 结果 | 证据 |
|---|---|---|---|
| C1 | **Bearer 关闭**（默认） | 全部 22 用例过（无认证直通） | A 全组 |
| C2 | **Bearer 开启 + 无 token** | HTTP **401**（middleware 层拦截，含 initialize/ping/notifications） | 实测 401 |
| C3 | **Bearer 开启 + 有效 token** | 协商正常 + tools/list 102 工具 | 实测 |
| C4 | **Bearer 开启 + 错误 token** | 401 | 实测 |
| C5 | Bearer 还原 | `enableBearerAuth` 还原为 false 后全量回归 22/22 | 已还原 |
| C6 | **分组 scope** `/mcp/Test` | 协商 + 组内工具聚合（playwright 前缀） | 实测 |
| C7 | **单服务器 scope** `/mcp/playwright` | 200 | 实测 |
| C8 | **$smart scope** | initialize 200（meta tools 拦截路径不变） | 实测 |
| C9 | **RAG builtin**（RAG 关闭） | tools/list 不含 rag 工具（`is_enabled()` 分支正确） | A10 计数 102 |
| C10 | **perSessionClient / startOnDemand** | 当前 DB 无该配置服务器——代码路径审查：两特性在 connect_server 前分流，与 transport 协议层无关，本次改动不触及 | 审查通过 |
| C11 | **OpenAPI 上游**（本机公网ip查询） | 未改动（`build_client` Openapi 分支原样） | 审查通过 |
| C12 | **HTTP/SSE 上游**（Idea-mcp-server） | 未改动（分支原样），日志见正常连接 | 日志证据 |

**测试期间发现并修复的回归**：
- **C2 初测失败**（严重）：rmcp service 挂载后绕过旧 dispatch 的逐请求 Bearer 检查，`enableBearerAuth` 开启时 initialize 无 token 仍 200。修复：`/mcp` 路由加 `mcp_bearer_middleware`（axum layer，复用 `check_bearer_auth`/`build_oauth_401`），路由组经 `Router::merge` + `layer(from_fn(...))` 包裹。修复后 C2-C5 全过。**这正是「授权认证不能受影响」要求抓出的实际问题。**

## 6. 遗留与人工复验项

1. **npx/uvx 首次下载进度**（B8）：事件流代码为旧实现逐行复制，但真机当前无未缓存包可触发；下次真实安装时观察「下载中 NN%」进度条。
2. **HTTP/SSE/OpenAPI 上游 rmcp 化**：未做（A1 定制保留）。后续若做，需先验证 rmcp client 对 2026 modern 上游的 discover bootstrap（源码确认有 modern→legacy 自动降级）与 MRTR/任务轮询等价性。
3. **cleanup 阶段（已完成，2026-09-27）**：`dispatch_mcp` 全家桶、`http_server.rs` 旧会话态（SESSION_STRATEGY/SSE_CHANNELS）、`mcp_tasks::create_tool_task`、`stdio_transport.rs` 旧本体、`request_cancel.rs` 整模块已删除；TEMP allow 移除；`mcp_version.rs` 复查后删除（依赖已随 create_tool_task 消失）。清理后回归：cargo check 0 警、cargo test 90 passed、E2E 22/22（见 §8）。
4. **多客户端并发**：本轮以单客户端流为主；rmcp 的 per-session worker 模型与旧实现的共享 pending map 语义不同（每 session 独立），大规模并发行为建议后续压测。

## 7. 结论

- 下游 `/mcp`：官方 rmcp `StreamableHttpService` + `HubBridge`，22/22 E2E + 特殊场景矩阵全过，行为与旧实现语义一致（含版本协商/退役决策）。
- 上游 stdio：`RmcpStdioTransport` 全部定制保留（runtime_env、下载进度、stderr tail、杀树、版本捕获），真机两台 stdio 服务器（npx wrapper / 本地二进制）连接与调用正常。
- **发现的 1 个真实回归（Bearer 绕过）已当场修复并回归验证。**
- 未替换面（HTTP/SSE/OpenAPI 上游、清理）已明确列出，风险可控。

## 8. 清理后回归（2026-09-27，清理完成追加）

| 项目 | 结果 |
| --- | --- |
| cargo check | 0 错 0 警 |
| cargo test --lib | 90 passed（清理后）/ 91 passed（native notify 接线后，+1 集成测试） |
| 下游 E2E | 22/22（`/tmp/rmcp_e2e_post_clean.txt`） |
| prompts/resources/tasks/未知方法 -32601 | 全过 |
| bearer-off initialize + tools/list | OK（102 工具） |
| stdio 真机（playwright/codegraph） | 子进程存活、工具列表正常 |

## 9. rmcp 原生 list_changed 通知接线（清理后补充实现）

清理评估时判定 `subscription_hub` 保留（2026 `subscriptions/listen` 自定义流仍在用），但 2025-03/06/11 标准客户端的 `notifications/tools|prompts|resources/list_changed` 此前无落点。本次补齐：

- `rmcp_bridge.rs`：`ServerCapabilities` 加 `enable_tool_list_changed`/`enable_prompts_list_changed`/`enable_resources_list_changed`；新增 `NATIVE_PEERS` 注册表（`on_initialized` 时记住每会话 `Peer<RoleServer>` handle）+ `fan_out_native_list_changed`（逐 peer 发原生通知，发送失败即剪枝死会话）+ `spawn_native_notify(kind)` 公开钩子。
- `subscription_hub.rs`：`notify_tools_list_changed`/`notify_prompts_list_changed`/`notify_resources_list_changed` 三个变更点**同时**触发 ①2026 subscriptions/listen 订阅者（原有自定义流）②2025 标准客户端（rmcp 原生通知）。
- 验证：新增 lib 内集成测试 `native_list_changed_fans_out_to_registered_peers`（in-process rmcp server+client duplex，真握手后触发三类 fan-out，断言客户端 handler 各收到 1 次）——通过。测试坑：`Notify::notify_one` 许可不累积，3 连发会丢唤醒，测试改为轮询计数器。
- 真机行为：2025 客户端在工具启用/禁用、服务器增删/重载、prompt/resource 变更时收到标准 `notifications/*/list_changed`；2026 客户端行为不变。

## 10. 上游 StreamableHttp 切 rmcp client（§3.9.6 回归，2026-09-27）

| 项 | 状态 |
| --- | --- |
| rmcp Auto lifecycle 连接 HTTP 上游（Idea-mcp-server，真机） | ✅ 走 **discover modern 路径**（2026 stateless）连接成功，62 工具，tools/call 真实调用返回（isError:true 为 IDEA 侧"未指定项目"业务错误，非传输问题） |
| E2E 全量 22 用例 | ✅ 22/22（初跑 21/22 的 FAIL 为 agent shell 超长 TMPDIR 被 playwright 拒绝的环境问题，换短 TMPDIR 重启后全过——非迁移回归） |
| MRTR 下游驱动重试（inputResponses 透传） | ✅ 契约不变（call_tool_once 转发） |
| tasks/get 轮询（CallToolResponse::Task） | ✅ 原生 TaskPayload 映射，契约不变 |
| SSE 上游（保留手写） | ✅ raw_meta 补齐（对齐 http/stdio）；live 连接验证需在 UI 启用 'Idea-mcp-server-sse' 后观察（SSE 端点在本轮为 disabled 状态） |
| cargo check / test --lib | ✅ 0 警 / 82 passed |

## 11. 清理后全量协议回归（41 用例，2026-09-27 收尾）

脚本：`/tmp/rmcp_full_regress.py`（41 用例，覆盖 2024-11-05 / 2025-03-26 / 2025-11-25 / 2026-07-28 四版本 + discover + scope + RAG builtin + 退役端点）。
结果：**41/41 passed**（`cargo check` 0 警，`cargo test --lib` 82 passed）。

### 11.1 本轮发现并修复的 2 个真实问题

| # | 问题 | 修复 |
|---|------|------|
| 1 | 2026 discover capabilities 缺 `extensions["io.modelcontextprotocol/tasks"]` 声明（TC-18 FAIL） | `rmcp_bridge.rs::get_info` 补 `ExtensionCapabilities` tasks 扩展声明 |
| 2 | SEP-2549 CacheableResult（`ttlMs`/`cacheScope`）注入后**泄露到 legacy 会话**——2024/2025-03/2025-11 的 tools/list、prompts/list、resources/list 都带 `ttlMs`，违反旧报告 TC-08 形状规则 | `rmcp_bridge.rs` 新增 `is_2026_session()`（读 `peer_info().protocol_version`）+ 三处 list handler 版本门控：仅 2026 会话启用 with_ttl_ms/with_cache_scope |

### 11.2 版本 × 公网 IP 真实调用矩阵（全部通过）

| 协议版本 | 工具暴露 | 公网 IP 真实调用 | 请求/响应版本一致 |
|---|---|---|---|
| 2024-11-05 | ✅ | ✅ | ✅ |
| 2025-03-26 | ✅ | ✅ | ✅ |
| 2025-11-25 | ✅ | ✅ | ✅ |
| 2026-07-28（无状态 + 无 resultType） | ✅ | ✅ | ✅ |
| discover（resultType + versions） | ✅ | ✅ | ✅ |

### 11.3 rmcp 行为差异（记录在案，非缺陷）

1. **2026 initialize 降级协商**：客户端以 `protocolVersion=2026-07-28` initialize → 服务器协商回 `2025-11-25`（rmcp 语义：modern 客户端应走 discover 路径）。
2. **会话 path-local**：rmcp 每-path 服务实例，`/mcp` 建的 session 用在 `/mcp/playwright` 得 404 Session not found——scope 测试须在对应 path 上 initialize。
3. **2026 tools/call 必带 `Mcp-Name` 头**且与 `params.name` 一致；非 ASCII 工具名无法过 latin-1 header（中文工具名的 modern 调用受限，legacy 路径正常）——公网 IP（openapi 中文工具名）在 2026 无状态路径的调用以 latin-1 兼容名验证。
4. **legacy 会话不再带 `ttlMs`/`cacheScope`**（修复后）；2026 会话按 SEP-2549 带缓存元数据。

### 11.4 其他覆盖

- 错误路径：错误版本头 400、未知版本 -32022、未知方法 -32601、未知工具 -32602、DELETE session 202、无 session GET 400（2024 退役）、`/mcp/message` 退役。
- Bearer 矩阵 5/5（临时开启→测毕还原关闭，见 §10）。
- RAG builtin 工具经 `/mcp` 暴露正常（SC-2）。
