# rmcp 迁移 50 轮独立代码复核 + 全矩阵 E2E 报告（2026-09-30）

- **日期**：2026-09-30　**版本**：`1.0.40003`（工作区未提交 rmcp 迁移增量）　**rmcp**：3.4.1
- **复核方式**：5 个独立复核代理 × 10 轮视角（http_server / rmcp_bridge / tasks+subscription / 传输层 / 宽松专项）+ 亲自逐段复核 1100 行未提交 diff；每轮引用 file:line
- **E2E**：14 个套件 **522/522 全过**（本轮新增 `rmcp_matrix_v2.py` 152 项 + `rmcp_leniency_v2.py` 15 项）；`cargo test --lib` **83 passed**
- **实测矩阵**：5 协议版本 × 4 通道（`/mcp` 根、`/mcp/Test` 组、`/mcp/{server}` 单服务器、`/mcp/$smart`）× 宽松/严格，**每版本每通道至少一次「本机公网ip查询」真实上游调用**（openapi → ip.3322.net），全部 `isError:false`、请求版本与响应版本精确一致

---

## 1. 复核发现与修复（全部落盘并验证）

### High
| # | 位置 | 问题 | 修复 |
|---|---|---|---|
| H1 | rmcp_bridge.rs `listen()` | hub 总线全局广播任意 URI 的 resource-updated，sink 对 filter 外 URI 返回 Err → **整条订阅流被终结**（别的客户端更新资源会杀掉本订阅） | 发送前用 `sink.accepted().resource_subscriptions` 本地过滤，URI 不匹配 `continue`；仅传输错误终止 |
| H2 | rmcp_stdio_transport.rs:169 | Windows 下用户 PATH 合并硬编码 `:`（应为 `;`），stdio 服务器命令解析必坏 | `#[cfg(windows)]` 分隔符分流（与 `resolve_in_path` 一致） |
| H3 | session_pool.rs:279 | rmcp 迁移后 `DELETE /mcp` 钩子丢失 → perSessionClient 隔离子进程**无限泄漏**（原 `cleanup_session` 成死代码） | 新增 `mcp_session_cleanup_middleware`（DELETE + /mcp* → 会话结束后 fire-and-forget `cleanup_session`） |

### 宽松模式缺陷（用户要求「只要不影响工具调用，都可以放行」，均已实测确认后修复）
| # | 场景 | 修复前 | 修复后 |
|---|---|---|---|
| D1 | `params` 缺失/非对象的裸 tools/call | 422 | 补 `params:{}` 走升格 → 200 |
| D2 | `jsonrpc` 缺失 / `"1.0"` | 415 | 宽松下规范化为 `"2.0"` → 200（含真实公网IP调用验证） |
| D3 | GET /mcp 缺 Accept（SSE 打开） | 406 | 宽松下补 `text/event-stream` → 200 SSE；严格仍 406 |
| D4 | 非法版本头带 session（`1999-01-01`/`garbage`/`9999-01-01`） | 400 | 宽松下 strip（会话握手已协商版本，头不承载新信息）→ 200；严格仍 400 |
| — | `v >= "2026-07-28"` 字典序把 `9999-01-01` 误判 modern | 误判 | `is_known_protocol_version()` 白名单门控（KNOWN_VERSIONS 镜像） |

### Medium
| # | 位置 | 问题 | 修复 |
|---|---|---|---|
| M1 | leniency 中间件 8MB 硬上限 | 本轮 diff 移除 `needs_body_check` 短路后，配置 `jsonBodyLimit`>8MB 的合法大请求被替换为空 body（Content-Length 不符）静默破坏 | 解析上限读同一配置（下限 8MB、上限 64MB clamp），超限透传空 body 让路由层报准确 413 |
| M2 | rmcp_bridge 后台任务 | `execute_tool_call` panic → 无 TTL 任务永久滞留 `working`，客户端无限轮询 | `AssertUnwindSafe(...).catch_unwind()` → `fail()` 回写 |
| M3 | bridge 上游调用 | 四传输中 3 个无调用超时，挂死上游永久占用 handler | 新增 `mcp/time.rs::timeout_tool_call`（600s 兜底，只杀真死挂不限制慢工具），隔离/共享两路径均包裹 |
| M4 | `client_ip_of` | XFF/X-Real-IP 无条件信任，活动日志 source_ip 可被任何客户端伪造；`TRUST_PROXY` 只是死变量 | XFF 仅在 `TRUST_PROXY` 显式开启时采信 |
| M5 | broadcast Lagged | 慢消费者丢通知无任何信号 | 记 `[rmcp] lagged` warn 日志（CacheableResult 30s TTL 自愈） |

### Low
- `subscription_hub::notify_task_status` 的 `taskIds` 只判非空不匹配具体 id（跨任务状态泄漏，test-only 路径）→ 精确 per-id 匹配（`task_ids: Vec<String>`）
- `to_json_ext` 对 `ttl=None` 输出 `"ttlMs": null`（严格客户端校验风险）→ 缺省省略，与 2025-11 shape 对称
- `spawn_ttl_sweeper` 无幂等守卫（每次服务器重启泄漏一个 60s 循环）→ AtomicBool 一次性守卫
- REST 404 判定 `contains("not found")` 子串过宽（上游错误内嵌该词组误标 404）→ 收窄为 `Tool '<name>' not found` / `no such tool`
- 模块头 `MCP 2025-11-25 Tasks` 过时 → 更新为 2025-11 核心 + 2026-07-28 扩展

### 明确不修（记录理由）
- `/mcp` 字面量与 `/mcp/{*path}` 各挂独立 `LocalSessionManager`（尾斜杠 session 不互通）：注释已声明 per-scope 有意设计，客户端混用尾斜杠无真实流量
- SSE 上游 transport 仍为手写实现（见 §4 遗留）
- 严格模式（`mcp.strictValidation=true`）保持 rmcp 原生规范拒绝（406/415/400），本轮修复全部以宽松模式为界，严格矩阵 10/10 不受影响

---

## 2. 全矩阵 E2E（新增 `scripts/e2e/rmcp_matrix_v2.py`，152 项）

**阶段 1（宽松）**：4 个 legacy 版本 × 4 通道：initialize 版本回显一致 + session mint + tools/list（IP 工具暴露、无 ttlMs/resultType 污染）+ **公网IP真实调用** + 2024 structuredContent 剥离；2026 版 × 4 通道：无状态 tools/list（CacheableResult）+ 无状态公网IP调用 + server/discover（supportedVersions/ttlMs/cacheScope）+ 未知版本 -32022 结构化错误。

**阶段 2（严格）**：5 版本 × 4 通道规范请求全通过 + 版本回显一致；缺陷请求（缺 Accept、2026 缺 client 元数据）拒绝。

**阶段 3（`rmcp_leniency_v2.py`，15 项）**：D1/D2/D3/D4 放行 + 严格模式对应拒绝 + unknown `_meta` pv 保留结构化 -32022（宽松不吞协议错误）。

## 3. 套件汇总（2026-09-30 最终回归）

| 套件 | 结果 |
|---|---|
| rmcp_full_regress | 41/41 |
| rmcp_leniency_matrix | 34/34 |
| rmcp_strict_matrix | 10/10 |
| rmcp_matrix | 39/39 |
| **rmcp_matrix_v2（新增）** | **152/152** |
| **rmcp_leniency_v2（新增）** | **15/15** |
| rmcp_round50/b/c/d/e/f/g | 33+77+19+18+14+14+12 |
| rmcp_round100 | 44/44 |
| **合计** | **522/522** |
| cargo test --lib | 83 passed / 0 failed |

**旧断言更新（2 处）**：`full_regress TC-14`、`round50b D` 原断言「错误版本头 → 400」——该行为按宽松新策略有意改变（放行），断言同步更新；严格模式 400 由 `rmcp_strict_matrix` 继续守护。

## 4. rmcp 迁移完整性结论（R31-R40 轮回答）

- **已切换**：stdio（`RmcpStdioTransport`，旧实现文件已删，仅存工具函数被复用）、Streamable HTTP（`RmcpHttpTransport`）、OpenAPI（rmcp-openapi 库）、下游 HTTP 服务器（`StreamableHttpService` + `HubBridge`）、旧 `McpClient` JSON-RPC 协议层无残留调用点（grep 全仓验证）；下载进度/更新检测/stderr_tail/CREATE_NO_WINDOW/进程树 kill/perSessionClient/on-demand 全部保留工作
- **未完成（遗留）**：**SSE 上游 transport（sse_transport.rs，630 行）仍是迁移前手写 JSON-RPC 实现**——rmcp 的 `transport-sse-client` feature 未启用，该实现还内嵌 Streamable-HTTP 回退分支（与 `RmcpHttpTransport` 重复）且丢弃 request 级 `_meta`。替换需验证 rmcp SSE client 对旧版 SSE 服务器的兼容性，属独立改动，本轮未动（记录为待办）

## 5. 用户关注点核对

| 关注点 | 结论 |
|---|---|
| 清理干净？ | stdio/HTTP/openapi/下游服务端：✅ 全部 rmcp 化无残留；❌ SSE transport 待办（§4） |
| 每版本每通道执行过 MCP？ | ✅ 5 版本 × 4 通道 × 公网IP 真实调用（宽松+严格），522 项全过 |
| 请求/响应版本一致？ | ✅ 每次 initialize 回显逐版本精确相等（矩阵 v2 全量断言） |
| 宽松/严格模式？ | ✅ 宽松：缺陷请求放行（D1-D4 修复后）+ 版本头未知值放行；严格：rmcp 原生拒绝不受影响 |
| 异常/负路径？ | ✅ -32022/-32020/-32601/400/406/422 各归其位；零 5xx、零内部错误 |
| 新特性？ | ✅ discover / tasks（SEP-2663 含 panic 守卫+所有权门控+超时）/ subscriptions/listen（URI 过滤修复）/ CacheableResult / 无状态调用 / SEP-2243 头派生（含非 ASCII base64 包装） |
