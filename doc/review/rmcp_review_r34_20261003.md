# rmcp 复核第三十四轮（R34，2026-10-03）：OpenAPI 三项（流式帽/死凭证/参数泄密）+ SSE 两项（trim/超时越界）

> 复核范围：mcp/sse_transport.rs + mcp/openapi_transport.rs 全量重读（多轮修复后的最新状态）；
> http_server.rs REST/API 尾部 2200 行重读（16 handler 门控/生命周期/竞态——本轮清洁）。
> 方法：侵蚀审计全在 → 2 个并行逐行代理 → 亲核验证 5 项 → 修复 → v34 套件 → 全量回归。

## 修复（5 项：Medium×3，Low×2）

### O1 [M] OpenAPI spec 32MB 帽在**整包缓冲后**才生效
`resp.bytes().await?` 先读完全部 body 再查 `body.len()`——chunked（无 content-length）
恶意服务器可在 30s 窗口内灌入 GB 级内存（正是注释声称要防的 OOM）。**修复**：改
`resp.chunk()` 流式累积，超帽立即中止。

### O2 [M] query/cookie 位置的 apiKey 凭证静默失效
location 非 header 时只打 warn「relying on spec security schemes」——**经 vendored
rmcp-openapi 0.32 源码核实该回退根本不存在**（securitySchemes 从不注入出站请求，唯一
凭证源是 default_headers）→ 每次调用裸奔 401，日志还把用户指向不存在的机制。**修复**：
connect 时响亮报错（不支持的 location 明确提示改 header 或显式参数），
build_default_headers 改返回 Result 传播。

### O3 [M] 工具调用参数全文写入持久化日志 DB
`args={}` 直接 `format!` 进 app_logger::log_to_db——参数常含 API key/token/个人数据，
且本文件自己建立了威胁模型（header 值为此脱敏）。**修复**：改记参数键名列表 + 字节数
摘要，不落值。

### S1 [L] SSE inline 解析器残留 `trim()`
背景 reader 已修（只剥尾部 `\r`），同文件内 post_request 的 inline 副本仍是全 trim
——Streamable-HTTP 跨 data 行分割的 JSON 内显著空格被吃掉（同文件里两份解析器行为不
一致即信号）。**修复**：镜像 reader 修复。

### S2 [L] SSE 总超时可越界 120s
deadline 检查在 60s chunk-wait **之后**——涓流喂入时实际上限是 deadline + 一整段等待。
**修复**：`timeout_at(deadline, ...)` 以剩余预算约束等待。

## 清洁
- http_server.rs REST/API 尾部（16 handler 门控逐一确认、含 GET 变体冷启动前置门控）
  、生命周期（start 持锁跨 teardown→探测 bind→真 bind）、loopback watch、ttl sweeper
  幂等——**本轮零发现**
- SSE reader-generation/pending 清理/端点事件 4 形态/UTF-8 跨块边界全部完好
- openapi Basic/OAuth2 header 构造（od 级验证过一次的 `******` 伪影再确认非源码损坏）

## 新套件 rmcp_supplement_v34.py（9 项）
- 4 legacy 回显 + 公网 IP ×3（2024-11-05 / 2025-03-26 + 2026 stateless，IPv4 Body 断言）
- 2026 discover（ttlMs 3600000）+ /health

## 验证
- `cargo check --lib` 0 错 0 警；`cargo test --lib` 102 passed；cargo build ✓
- 38 套件 **1234/1234 全绿**（v34 9/9 新计入；重建二进制热跑）

## 教训
- **「上限检查」必须在分配之前**：`bytes()` 后查长度等于没有上限——凡资源上限都问一
  句「检查发生在分配前还是后」。
- 注释声称的回退机制必须到依赖源码里验证存在性（O2 的「spec security schemes 回退」
  从未实现，日志欺骗了用户三年）。同文件两份近似代码行为不一致（S1 trim）= 复制体
  未同步修复的信号。
