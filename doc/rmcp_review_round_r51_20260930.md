# rmcp 迁移 50 轮独立代码级复核（R51-R100）+ 全矩阵 E2E 报告（2026-09-30）

- **日期**：2026-09-30　**版本**：`1.0.40003`（工作区未提交增量）　**rmcp**：3.4.1
- **复核方式**：15 个独立复核代理（R51-R70）× 分段逐行扫描 + 亲核 10 轮修复落盘（R71-R80）+ 15 套件回归轮（R81-R95）+ 终验（R96-R100）。每轮引用 file:line，疑似 bug 一律先 grep/rmcp 源码 vendored 对照再定论
- **E2E**：15 个套件 **557/557 全过**（新增 `rmcp_round_r51.py` 35 项）；`cargo check` 0 错 0 警；`cargo test --lib` **83 passed**
- **真实调用**：每协议版本（2024-11-05 / 2025-03-26 / 2025-06-18 / 2025-11-25 / 2026-07-28）× 每通道（`/mcp` 根 / `/mcp/Test` 组 / `/mcp/{server}` 单服务器 / `/mcp/$smart`）× 宽松+严格，全部至少一次「本机公网ip查询」（openapi → ip.3322.net）真实上游调用，`isError:false`、initialize 版本回显逐版本精确一致

---

## 1. 本轮发现与修复（17 项全部落盘并 E2E 验证）

### High
| # | 位置 | 问题 | 修复 |
|---|---|---|---|
| H1 | http_server.rs `build_router` | **bearer 中间件经 `Router::layer` 全局生效**（非仅 `/mcp`）：`enableBearerAuth` 开启后 `/health`、`/.well-known/*` 一律 401 → `loopback_ok()`（裸 GET /health 断言 body）**每次启动 + 每 30s 误报 loopback 劫持**弹对话框（R54 #1 / R51 F3 双重独立发现） | `mcp_bearer_middleware` 加 `/mcp*` 路径门控（其余端点本就自鉴权）；开 bearer 实测 /health=200、/.well-known=200、无 key /mcp=401 |
| H2 | rmcp_bridge.rs `listen()` | **list_changed 三 lane 未按 accepted filter 预过滤**：`SubscriptionSendError::NotificationNotAccepted` 是**过滤拒绝而非传输错误**（已核 rmcp 3.4.1 service/server.rs:186-230 + 枚举定义），`accepted filter = advertised ∩ requested` 交集下，单 lane 订阅客户端会被其他 lane 广播**误杀整条订阅流**（R65 F1，前轮 H1 只修了 resource URI lane） | 三 lane 对称预过滤（`lane_ok = != Some(false)`）+ 错误类型分支（NotAccepted/Unsupported → continue；Closed/Service → 终止） |

### Medium
| # | 位置 | 问题 | 修复 |
|---|---|---|---|
| M1 | http_server.rs `mcp_leniency_middleware` | **未按路径收口到 `/mcp`**：`/api/tools/{server}/{tool}` 的 body 即工具入参，被注入伪 `jsonrpc:"2.0"` 键 → strict-schema 上游工具（additionalProperties:false）拒绝调用（R51 F1） | leniency 顶部加 `/mcp*` 路径守卫 |
| M2 | http_server.rs `build_router` | **bearer/leniency 层序倒置**（axum 后加=外层）：leniency（最高 64MB body 解析）先于鉴权执行，未认证请求消耗解析资源（R51 F2） | bearer 移到最外层（最后 `.layer`） |
| M3 | http_server.rs `start()` | **端口/body_limit 变更重启未先停旧实例**：guard 替换在 bind 之后 → Linux/Windows 撞 EADDRINUSE（状态报 false 而旧服务仍在跑）、macOS 双监听窗口（R55 F1） | restart 分支先 take + fire 三 abort 通道 + 100ms 让渡再 bind |
| M4 | http_server.rs `spawn_loopback_check` | **常驻 watch 端口钉死**：首启后改端口，watch 永久轮询旧端口 → 旧端口被占误报劫持、真实端口被占沉默（R54 #2） | watch 每 tick 读 `current_port()`；无服务时静默清位 |
| M5 | rmcp_bridge.rs `execute_tool_call` | **RAG builtin 工具绕过 disabled 检查**：builtin 早退分支跳过禁用门 → UI 禁用的 `rag_*` 工具在 HTTP `/mcp` 仍可调用（Tauri 路径有门，行为不一致）（R63 F1） | disabled 检查移到 RAG 分支之前 + `aggregate_tools` 对 builtin 同样应用 `apply_tool_filters` |
| M6 | rmcp_bridge.rs `get_prompt`/`read_resource` | **缺 `$smart` scope 门控**（list 有、get/read 无）：list 返回空但 read 可取内容，绕过 not_ready 门（R64 F1） | 两 handler 加与 list 相同的 is_smart_scope 早退（-32602） |
| M7 | http_server.rs `extract_filter_list` | **空 allow-list `[]` 被当「允许全部」**：与 origin fail-closed 语义相反，清空组白名单后全部工具仍在 `/mcp`/`/api` 暴露（R52 F1） | 空数组返回 `Some(vec![])`（全拒） |
| M8 | on_demand.rs `schedule_idle` | **handle 交换竞态可永久丢失空闲定时器**：并发 run_call 交替 replace，旧 handle 覆盖新 handle → stdio 子进程永不回收（R69 O1） | `shutdown_on_demand_idle` snapshot 不匹配时经非 async 自由函数 `spawn_idle_timer` 以当前 last_used **重挂定时器** |
| M9 | sse_transport.rs:345-357 | **Streamable HTTP JSON 早退路径漏发 `notifications/initialized`**：SSE 类型连 Streamable 服务器 connect「成功」后首个 `tools/list` 被严格服务器 -32002 拒绝（R67 F2） | 早退前补发通知（warn-and-continue） |

### Low（本轮顺手修复）
| # | 位置 | 问题 | 修复 |
|---|---|---|---|
| L1 | sse_transport.rs:379 | 首 chunk 预览 `&text[..500]` 多字节字符 panic（R67 F1） | `chars().take(300)` |
| L2 | sse_transport.rs | 三循环逐 chunk lossy 解码 → 跨 chunk 多字节字符 U+FFFD 静默污染工具输出（R67 F19） | 全部改字节缓冲，仅对完整行解码 |
| L3 | sse_transport.rs 两处 | 服务器发起的 request（sampling/roots）撞 pending id 会**偷走等待中的响应条目**（R67 F20） | 匹配前加 `method.is_none()` 守卫（两处） |
| L4 | sse_transport.rs:131 | POST 非 2xx 不检查，白等 60s 报 timeout 而非真实原因（R67 F4） | 非 2xx 读 body 立即报错 |
| L5 | sse_transport.rs | 回退分支 SSE/JSON 读无超时 → 挂死上游永久占用（R67 F3） | client 加 `.read_timeout(120s)` |
| L6 | sse_transport.rs | 用户 `mcp-session-id` 头原样重放，与新 session 头**双值并存**（reqwest header 是追加）（R67 F6） | 5 处 header 循环过滤该键 |
| L7 | sse_transport.rs | 超时/POST 失败后 pending 条目泄漏（R67 F7） | 错误路径 `remove(&id)` |
| L8 | sse_transport.rs | 重复 connect 覆盖 stop_signal，旧 reader 泄漏（R67 F8） | connect 开头先 signal 旧 reader |
| L9 | sse_transport.rs | 流死亡后 `connected` 恒 true（R67 F9） | `Arc<AtomicBool>` 共享标志，reader 退出置 false |
| L10 | sse_transport.rs:432 | 无 event 类型的绝对 URL endpoint 死分支（R67 F11） | 内层同时接受 http(s) 绝对 URL |
| L11 | sse_transport.rs | POST 探测（initialize id=0）+ 公共路径二次 initialize → 同 session 双 init，严格服务器拒绝（R67 F5） | probe_sent_initialize 标志跳过二次 init |
| L12 | sse_transport.rs | 通知非 2xx 静默（R67 F12） | warn 日志 |
| L13 | sse_transport.rs:47 | client builder `.expect` panic 点（R67 F14） | `unwrap_or_else(Client::new)` |
| L14 | rmcp_http_transport.rs:237 | `modern = peer_info().is_some()` 恒 true（两种 lifecycle 都 set peer_info），legacy 服务器日志误报 "2026 modern"（R66 F1） | 按 `protocol_version == V_2026_07_28` 判别 |
| L15 | http_server.rs:1707 | jsonrpc 归一仅覆盖缺失/"1.0"/"2"，"1.1" 仍 415（R54 #3） | 改 `!= "2.0"` 全量归一 |
| L16 | http_server.rs `build_oauth_401` | WWW-Authenticate 值内嵌客户端可控 Host + `.unwrap()` panic 点（R51 F4） | `HeaderValue::from_str` 校验 + 静态 fallback |
| L17 | http_server.rs `parse_body_limit` | `"99999999999999mb"` usize 溢出 debug panic（R51 F6） | `saturating_mul` |
| L18 | openapi_transport.rs | connect 日志以 Debug 明文输出 headers/security（含凭证）（R68 F3） | security 只输出变体名 |
| L19 | openapi_transport.rs `fetch_spec` | 无超时、body 无上限（R68 F4） | 30s timeout |
| L20 | pool.rs / openapi_transport.rs | 残缺 security 兜底发垃圾空 `Authorization: Bearer ` 头，掩盖 spec 自带 scheme（R68 F5） | malformed（全空字段）跳过 auth 头，依赖 spec scheme + warn |
| L21 | commands/tools.rs | Tauri 命令路径无 600s 死传输兜底（UI 调用遇挂死上游永久 pending）（R66 F8） | `timeout_tool_call` 包裹 |
| L22 | sse_transport.rs / session_pool.rs | 失败驱逐无条件 `map.remove`，可误杀并发重建的新 client（R69 S1/O3） | `Arc::ptr_eq` 校验后驱逐 |

### 注释/文档修正（4 处）
`client_ip_of`（无 socket fallback 如实说明，R53 F2）、leniency 64MB clamp 边界（R53 F1）、MRTR 空 handler 语义（R66 F2：empty handler ⇒ input requests denied）、`transport-sse-client` 不存在的结论落盘。

### 误报澄清（2 项，经字节级/源码核验撤销）
- R68 初判 `format!("******", …)` 为「文件损坏」——`od -c` 证实为终端读路径脱敏，源码原文正确（`Bearer {}`）。
- R70 F4「隔离路径无超时」——实读 rmcp_bridge.rs:558 确认 `timeout_tool_call` 已包裹，误报。

### 记录在案、有意不修（含理由）
| 项 | 理由 |
|---|---|
| **SSE 上游 transport 保留手写实现** | **rmcp 3.4.1 无 `transport-sse-client` feature**（全 feature 清单已核，仅 `client-side-sse` 供 streamable-http 响应流内部使用）——当前 SDK 版本下无法迁移；本轮已把其中全部 High/Medium 缺陷修复。待 SDK 提供 SSE client 后整体替换 |
| pool.rs 读锁跨工具调用 await（R68 F2/R63 F5） | 存量设计债：需 `PoolEntry.client` 改 `Arc<McpClient>`（迁移面大）；session_pool/on_demand 均已锁外执行，共享路径待后续专项 |
| REST group 路径 meta 工具泄漏（R52 F2） | 既有设计缺口（非迁移回归），触发条件受限（Smart Routing 开 + 组含内建成员）；建议下轮补 is_meta_tool 过滤 |
| apply_tool_filters Err 时 fail-open（R52 F3/R63 F1-L） | DB 错误瞬时窗口，list/call 两侧行为一致无漂移；fail-closed 需权衡误杀 |
| REST `/rest/*` 不写 activity_log（R51 F7） | 迁移前遗留，非回归 |
| `smart_rest_call` source_ip 回退 "127.0.0.1"（R51 F8） | 语义瑕疵，与 /api 路径口径不一，记录 |
| `client_service.cancel()` Result 丢弃（R65 F3）、`_context` 命名（R65 F4）、测试固定 sleep flaky 风险（R65 F2） | test-only / cosmetic |
| subscriptions/lane lane_ok 用 `!= Some(false)` 而非 `== Some(true)` | advertised∩requested 交集语义下，未请求 lane 本就不在 accepted 内；`Some(false)` 显式拒绝才需要跳过——保持宽容与 hub 未声明 listChanged 的合规退化一致 |

---

## 2. R51-R100 轮次账本

| 轮 | 范围 | 方式 | 结果 |
|---|---|---|---|
| R51 | http_server.rs 1-560 + 中间件/路由挂载交叉 | 独立代理逐行 | F1/F2/F3 Medium + 8 Low/Info |
| R52 | http_server.rs 561-1120 | 独立代理逐行 | F1 Medium（空 allow-list）+ 6 Low/Info |
| R53 | http_server.rs 1121-1680 | 独立代理逐行 | 2 Low + 2 Info，零 panic/锁风险 |
| R54 | http_server.rs 1681-2240 | 独立代理逐行 | **H1** + M4 + 2 Low（未通过→修复） |
| R55 | http_server.rs 2241-末尾 | 独立代理逐行 | M3 + 3 Low + F8 挂载核验 ✓ |
| R56 | 清理完整性 grep 审计（亲核） | McpClient 残留/旧 transport 构造/mcp_version 引用/rmcp features/手写 jsonrpc 关键字 | stdio/HTTP/openapi/下游全 rmcp 化；SSE 手写为唯一遗留（SDK 无 feature） |
| R57 | bearer 作用域实测（亲核） | 开 bearer 实测 4 端点 | 证实 H1，修复后复验通过 |
| R58 | SSE 迁移可行性评估（亲核） | rmcp 3.4.1 feature 清单 + vendored 源码 | 无 transport-sse-client，结论：保留手写 + 修缺陷 |
| R59 | rmcp_stdio_transport.rs + client.rs 全文（亲核） | 逐行 | runtime_env/下载进度/stderr_tail/kill tree/CREATE_NO_WINDOW 全保留，无问题 |
| R60 | 存量套件覆盖面分析（亲核） | 14 套件用例名盘点 | 定位补测缺口（batch/并发/中文工具名 2026/prompts 资源逐版本/生命周期/严格×真实调用） |
| R61-R65 | rmcp_bridge.rs 1-1221 五段 | 独立代理逐行 | H2 + M5/M6 + 低/信息若干；版本门控/前缀/allow-list/mrtr/tasks 核验通过 |
| R66-R70 | 传输层×2 + openapi/pool + session/on_demand + hub/tasks | 独立代理逐行 | M8/M9 + SSE 10 项 + rmcp API 调用点与 vendored 源码逐一对照 ✓ |
| R71-R80 | 修复落盘轮（亲核，每项 cargo check + 定向验证） | bearer 门控/watch 端口/restart teardown/modern 判定/jsonrpc/SSE 修复批1（F1/F2）/批2（F19/F20）/批3（F6-F9/F11/F4/F7/F3）/$smart+RAG+allow-list/并发 S1+O1+O3 | 17 项全部编译通过 + 实测 |
| R81-R95 | 15 套件全量回归（多轮，含重建后复跑） | rmcp_full_regress 41 / leniency_matrix 34 / strict 10 / matrix 39 / matrix_v2 152 / leniency_v2 15 / round50 33 / 50b 77 / 50c 19 / 50d 18 / 50e 14 / 50f 14 / 50g 12 / round100 44 / **round_r51（新增）35** | **557/557 全过**（修复后两轮确认） |
| R96 | cargo check + cargo test --lib（亲核） | 0 错 0 警 / 83 passed | ✓ |
| R97 | bearer 开关实测（亲核） | 开→4 端点断言→还原 | ✓ |
| R98 | 版本×通道×模式真实调用覆盖确认 | matrix_v2 152 项含每版本每通道公网IP + strict 矩阵 + r51 严格×单服务器×4 版本真实调用 | ✓ 全覆盖 |
| R99 | 本报告 + AGENTS.md 更新 | 文档 | ✓ |
| R100 | 汇总收尾 | — | ✓ |

---

## 3. 新增测试套件 `scripts/e2e/rmcp_round_r51.py`（35 项，全过）

| 组 | 覆盖 |
|---|---|
| B1 | JSON-RPC batch 数组 body → 不 5xx/不挂断 |
| B2 | 同一会话并发 10 请求（含 2 次公网IP 真实调用） |
| B3 | **中文工具名 2026 无状态 root 通道真实调用**（`_meta` 完整三件套） |
| B4 | prompts/list + resources/list 逐版本（5 版本）+ 2026 ttlMs 注入 / legacy 零污染双向断言 |
| B5 | ping 逐版本（legacy 空 result {} × 4；2026 移除非 5xx 结构化） |
| B6 | 会话生命周期：DELETE 后复用报错 / 同会话重复 initialize / notifications/initialized |
| B7 | string id 往返保留 / 非法 JSON 4xx / text/plain Content-Type 不 5xx |
| B8 | 无效 cursor 分页参数不 5xx |
| B9 | 2026 logging/setLevel + completion/complete 结构化响应 |
| B10 | **严格模式 × legacy 4 版本 × 单服务器（中文 scope）通道公网IP 真实调用**（DB 热切 strictValidation，测毕还原） |
| B11 | GET SSE 流（带会话）200 + 流保持打开 + 无 session GET 退役 400（rmcp 原生语义） |
| B12 | resources/read 返回内容 |

---

## 4. 用户关注点核对结论

| 关注点 | 结论 |
|---|---|
| 是否清理干净？ | ✅ stdio（RmcpStdioTransport）/ Streamable HTTP（RmcpHttpTransport）/ OpenAPI（rmcp-openapi）/ 下游服务器（StreamableHttpService+HubBridge）全部 rmcp 化；旧 `dispatch_mcp`/`mcp_version.rs`/手写 JSON-RPC 协议层 grep 全仓零残留（仅历史注释）；`McpClient` 为薄封装仍被 pool/session_pool/on_demand 使用（设计如此） |
| 是否所有功能都切到 rmcp？ | ✅ 除 SSE 上游 transport（630 行手写）——**rmcp 3.4.1 无 transport-sse-client feature，无法迁移**（本轮证据落盘）；其全部 High/Medium 缺陷已在本轮修复 |
| 每版本每通道执行过 MCP？ | ✅ 5 版本 × 4 通道 × 宽松+严格 × 公网IP 真实调用（matrix_v2 152 项 + r51 补充严格×单服务器） |
| 请求/响应版本一致？ | ✅ 每次 initialize 回显与请求逐版本精确相等（matrix_v2 全量断言 + r51 B4/B10） |
| 宽松/严格模式？ | ✅ 宽松（默认）：缺陷请求放行（D1-D4 + 本轮 M1/M2/L15 收口后行为不变）；严格：rmcp 原生拒绝（strict 矩阵 10/10 不受影响）；所有版本均支持宽松——只要不影响工具调用都可放行 |
| 新特性？ | ✅ discover / tasks（SEP-2663 含 panic 守卫+超时）/ subscriptions/listen（本轮 H2 修复 lane 误杀后回归通过）/ CacheableResult / 无状态调用 / SEP-2243 |
| 异常？ | ✅ 零 5xx、零 panic 可达点（R51-R70 五代理 + 本轮 unwrap 清扫）；错误码 -32022/-32020/-32601/-32602/-32603 各归其位 |

## 5. 遗留（下轮候选）

1. pool.rs 共享路径读锁跨 await（设计债，需 client Arc 化）
2. REST group 路径 meta 工具过滤（R52 F2）
3. apply_tool_filters fail-open → fail-closed 权衡
4. rmcp 提供 sse-client feature 后整体替换 sse_transport.rs（届时回退分支可删）
5. R65 F2 测试固定 sleep 的 flaky 风险（CI 慢机）

---

# 第二轮：遗留修复 + 剩余代码面复核（R101-R119，2026-09-30 续）

## 范围

- **遗留修复**：pool.rs client 改 `Arc<tokio::sync::Mutex<McpClient>>`（call 路径锁外执行，消除 UI 状态轮询被 600s 慢调用冻结）；REST group meta 工具过滤（`list_group_tools` 排除 builtin meta 工具、`call_group_tool` 跳过 builtin+meta 组合）；rmcp_bridge 测试固定 sleep 改轮询 NATIVE_PEERS 注册。
- **剩余代码面地毯式复核**（15 个独立代理 R105-R119）：commands/ 全量 22 文件 4758 行（runtime.rs 1450 / servers.rs 785 / rag.rs+config.rs / 其余 17 小文件）；services 其余（skill_service+fts_service+runtime_env+log_service+server_service 等 ~6700 行）；前端 MCP 链路（tauriClient.ts 1448 / ServerCard 1245 / ServerForm 1454 / SettingsPage 4494）。

## 修复清单（本轮 25 项）

### 行为级（High/根因）
| # | 位置 | 修复 |
|---|------|------|
| 1 | http_server.rs leniency | **tasks/get、tasks/cancel 恒 400**：`tasks/` 分支重复（第二个 `params.taskId` 分支永不可达），tasks 系列方法缺 Mcp-Name 注入被 rmcp 400 拒绝 → 删死分支 |
| 2 | http_server.rs leniency | **宽松模式 `id:null` 恒 422**：rmcp 把 `id:null` 反序列化为 Notification（session 外拒绝），无论注入什么 _meta → bare upgrade 前改写 null→0（宽松哲学：不阻断可用调用） |
| 3 | tauriClient.ts registry | **registry 版本查询路由失配（功能损坏）**：前端 query 形态 `/registry/servers/versions?serverName=` 落入 list_registry_servers 返回整个列表 → 补 2 个 query 形态分支映射到 get_registry_server_version(s) |
| 4 | servers.rs to_connection_relevant | **#1055 快路径恒失效（根因）**：DB 恒产 `perSessionClient/startOnDemand: false` 而前端 payload 缺键 → `false != absent` → 所有 UI 创建的服务器改 description 也断连重启 → 归一化 false→移除键 + 3 个单测（boolean_false_and_absent / default_timeout / description_only） |
| 5 | EditServerForm.tsx | **编辑禁用服务器被静默复活**：payload 不含 enabled → Rust default true 改写 DB → 注入 `enabled: server.enabled !== false` |
| 6 | runtime_env.rs + runtime.rs | **uvx reinstall 清错缓存目录**：删共享 `runtimes/uv-cache` 既清不掉本服务真实缓存（per-server `uv-cache-{name}`，env_overrides UV_CACHE_DIR）也无谓丢弃共享内容 → 新增 `uvx_server_cache_dir()` 清 per-server 目录（真机问题：uvx 更新不生效） |
| 7 | config.rs | **import_settings 无鉴权可提权**（导入可创建 admin 用户）、**get_settings 泄露 bearerKeys 明文 token + llmProviderApiKey** → 两者加 require_admin（skipAuth 短路不受影响） |
| 8 | runtime.rs 两处 | **char-boundary panic**：PATH 含中文目录时 `&path[..300]` 字节切片 panic（启动路径可触发）→ char_indices 安全截断 |
| 9 | runtime.rs tar 解压 | **zip-slip**：tar 0.4 `Entry::unpack` 无路径校验（Windows zip 有 mangled_name，不对称）→ 拒绝 ParentDir/RootDir/Prefix 条目 |
| 10 | runtime.rs | **版本参数白名单** `validate_version_arg`（`../`/绝对路径可逃逸 remove_dir_all）→ install×2/uninstall 应用 |
| 11 | skill_service.rs | **symlink 环栈溢出崩溃**：copy_dir_recursive 无条件跟随链接环 → visited 集合防护（允许 DAG 重复） |
| 12 | skill_service.rs | **dir_name 消费点无二次校验**：export/uninstall/delete/reconcile 的 `join(dir_name)+remove_link` 可被手改 DB 的 `../` 逃逸 → 抽 `valid_dir_name` helper 4 处应用 |
| 13 | cloud.rs+registry.rs×5 | reqwest::Client::new() 无超时（上游挂起命令永久阻塞）→ 30s timeout |
| 14 | auth.rs | register 无门控（注释与实现矛盾）→ 桌面端禁用（无 UI 入口） |
| 15 | auth.rs | skipAuth 兜底不一致（auth 侧 false / get_public_config true）→ 统一 true（桌面端） |
| 16 | http_server.rs(commands) | Windows tasklist 缺 CREATE_NO_WINDOW → 补齐 |
| 17 | registry.rs | server_name/version 未编码插 URL path → percent-encode |
| 18 | logs.rs | log_event level 无白名单（任意串成隐形行）→ 白名单兜底 info |
| 19 | log_service.rs | cleanup_by_days 负数 days 清空全部活动日志 → clamp 1..=3650；FTS 清理 IN 列表 32766 变量上限 → 1000/批分块 |
| 20 | servers.rs | 序列化失败 Null==Null 吞变更 → fail-closed 强制重连；重连路径 embedding 滞留 → 缺失时保守 remove |
| 21 | tauriClient.ts | group add/remove 成员降级丢完整配置（url 等）→ 保留原对象数组 |
| 22 | SettingsPage.tsx | httpPort 逐键击持久化（每次击键 loading+toast、清空被弹回、无范围校验）→ draft state + blur 提交 + 1024-65535 校验；emit cache://cleared 无 Tauri 守卫（web 误报失败）→ isTauri() 门控 |
| 23 | runtime.rs | get_unix_path stdout 盲信（rc 文件 echo banner 污染 PATH）→ 取含 `:` 分隔的最后一行 |
| 24 | runtime.rs | nodejs.org 下载 reqwest::get 无超时 → 共享 client（connect 15s / read 120s） |
| 25 | locales ×4 | 补 `settings.selectAtLeastOneGroupOrServer` 缺键；tsc 12 = 基线零新增 |

## E2E 回归（最终）

- **16 套件 570/570 全绿**：full_regress 41 / matrix_v2 152 / matrix 39 / leniency_matrix 34 / leniency_v2 15 / strict_matrix 10 / round50 33 / 50b 77 / 50c 19 / 50d 18 / 50e 14 / 50f 14 / 50g 12 / round100 44 / round_r51 35 / **gap_matrix_r107 43**（新计入）。
- **公网IP MCP 真实调用矩阵 10/10**（新固化 `scripts/e2e/rmcp_public_ip_matrix.py`）：3 个 legacy 版本完整 lifecycle（init 协商一致 + notifications/initialized + tools/call）+ 2026 无状态 + 单服务器 scope + REST 单服务器 + REST group，全部返回真实公网 IP（ip.3322.net 上游）。
- `cargo test --lib` **86 passed**（新增 3 个 connection_relevance 单测）；`cargo check` 0 错 0 警；`npx tsc --noEmit` 12 = 基线；`npm run build` ✓。

## 记录不修项（评估后保留）

- R112 M1：prompts/resources FTS ref_id=name 非唯一（重名互相破坏索引，启动 rebuild 自愈）——需 name 唯一性 UX 决策，后续。
- R114 F2：用户选择的 Python 版本对直连 `python` 命令不生效（仅 uv/uvx 生效，UI 语义半成品）——需产品决策。
- R107 F5：builtin 工具名兜底整 key 子串 vs token 级口径不一致；R106 F3：runtime.rs 三处同步子进程阻塞 tokio worker（legacy 风格）；R106 F9：列表刷新写 12+ 条 [runtime] 日志噪音。
- R111 F5：bearer token 明文存储/明文比较（本地单用户可接受，expose_http 局域网开放时建议 SHA-256）。
- R117 F2/F3/F5：passthroughHeaders(sse/http)/KeepAlive/OAuth2 tokenUrl+clientId+secret UI 收集但 Rust 模型无落点（保存即丢）——需产品决策（补字段+消费 或 隐藏 UI）；AGENTS.md §3.11.2 所列 keepAlive 字段失实。
- R119 #3-8、R116 全部低项、R118 #2-5：打磨级，不阻塞。

## 套件账本

修复 1/2（tasks Mcp-Name 死分支 + id:null 422）由本轮最终回归揪出——说明「跑完测试不代表全绿套件覆盖的场景在最新二进制上复验」是必要环节；两处均为第一轮改动引入的回归，被 16 套件全量重跑捕获。
