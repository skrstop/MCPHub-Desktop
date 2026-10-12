# OAuth 复核战役 · 第二十轮（R20）报告 — 2026-10-11

> 范围：3 代理切片（rmcp 迁移完整性 / rag+mv+smart 修复合自身复审 / 前端 OAuth·pin·Apps 链路）+ 亲核全部 74 套 E2E + 1 Low 修复 + 修复后复验。

## 1. 代理切片结论

### A1 · rmcp 迁移完整性 + 清理（代理）— **六项全确认干净**
1. 清理零残留：dispatch_mcp/mcp_version/request_cancel/SESSION_STRATEGY/SSE_CHANNELS/jsonrpc_response/create_tool_task/http_transport.rs//mcp/message 全仓无代码命中（仅行为对照注释）；2024 双端点确认退役。
2. pool.rs build_client 四分支（stdio/sse/streamable-http/openapi + builtin 明确报错）无旧实现；on_demand 复用同一 build_client。
3. lib.rs 167 命令注册双向对账：无孤儿、无漏注册。
4. 五版本协商 / 宽松严格（-32022 保护）/ server/discover（3600000+Public）/ tasks 扩展 / subscriptions/listen / ui:// read_resource 回退 / list TTL（#1277 语义）逐项在位。
5. 路由层序：bearer（最外）→ rest_post_bearer（/rest+/api POST + oauth/connect 防预缓冲）→ leniency → session_cleanup → route_gate（fail-closed）→ rmcp。
6. 近期修复合（list_freshness / on_demand ensure 三级检查 / ssrf_guard IPv6 位运算逐项验算）零回归。

### A2 · rag+mv+smart 修复合自身复审（代理）— **1 Low**
六个专项（remove_doc_sql retry / delete_doc 锁序 / git.rs canonical hash / mv 注册制 / smart 六字段 / vectordb+chunker）全部确认干净。唯一发现见 §2。

### A3 · 前端 OAuth/pin/Apps 链路（代理）— **零发现**
- OAuth Server 全部 9 组配置项（enabled/requireClientSecret/requireState/三个 lifetime/allowedScopes/dynamicRegistration×4）「UI→保存键→Rust 消费点」三元组逐一对账干净（oauth_provider.rs 行号级证据）。
- disablePasswordLogin 无 SSO 通知块 + locales 四语言键齐全。
- pin 链路（ServerToolConfig 裸名 item.key → tauriClient 映射 → Rust 签名）对账干净。
- ui:// 展示无 crash 风险（布尔标签 + 回退渲染）。
- locales 四语言递归 diff：zh/fr/tr 相对 en **0 缺失 0 多余**。
- tauriClient oauth-clients/source-update 映射与 Rust 签名精确匹配。

## 2. 【Low·已修】rag LIKE 转义链漏反斜杠自转义

**位置**：`rag/service.rs` `list_tags_paged`（:1844）与 `search_docs_paged`（:2380）——`key.replace('%',"\\%").replace('_', "\\_")` 后以 `LIKE ? ESCAPE '\\'` 执行，但 `\` 自身未转义：搜索词含 `\`（如 Windows 路径片段 `docs\readme`）语义偏离，以 `\` 结尾时 pattern 悬空转义符 → **静默空结果**。

**对比信号**：smart_routing/store.rs 的 `esc_like` 已正确先转义 `\`（R25/R35 修复），rag 侧两处是同族漏网第 4/5 处。

**修复**：两处 pattern 构造改为 `key.replace('\\',"\\\\").replace('%',"\\%").replace('_', "\\_")`（反斜杠优先，顺序敏感）。

**验证**：cargo check 0 错 0 警；cargo test --lib rag 34 passed；重建二进制后 r1700 42/42 + v27 3/3 + full_matrix_v18 262/262。

## 3. 全量 E2E（亲核，全部 74 套逐一执行，新二进制）

| 批次 | 套件 | 结果 |
| --- | --- | --- |
| 核心 | r109 32 / matrix_v2 152 / leniency_v2 15 / matrix 39 / full_regress 41 / strict 10 | 全绿 |
| pin/搜索 | pin_matrix_r111 31 / r460 37 / r461 22 / r462 24 / carpet_r200 101 / carpet_r300 122 | 全绿 |
| 版本矩阵 | full_matrix_v18 262 ×2（修复前后各一）/ matrix_gaps_r10 23 / leniency_pairs 15 | 全绿 |
| round50 家族 | 33+77+19+18+14+14+12（50g） | 全绿 |
| gap/supplement | r107 43 / r35x 12 / r36x 24 / r30x 22 / v19-v40 22 套 / r158 24 | 全绿 |
| 轮次时代 | r800 18 / r900 36 / r1000 71 / r1100 67 / r1200 45 / r1300 46 / r1400 99 / r1500 20 / r1600 33 / r1700 42 / apps_ui 7 / r1800 21 / r1900 23 / r458 16 / r51 35 / round100 44 / audit_r5xx 15 / audit_r6xx 20 / baseline_r110 23 | 全绿 |
| 公网IP | public_ip_matrix 10/10（221.225.50.167） | 全绿 |

**合计 ~2000+ 断言零失败**；每版本（2024-11-05/2025-03-26/2025-06-18/2025-11-25/2026 modern）× 每通道（/mcp root、单服务器 scope、group、$smart、GET SSE、REST、/api）× 宽松/严格均覆盖；公网 IP 查询 MCP 每版本真实调用。

## 4. 收敛态势

R17 宣告收敛 → R18（1H2M 新代码）→ R19（1 真 Low-M，SKIP 掩盖的老缺陷）→ R20（3 切片仅 1 Low 转义链）。**连续两轮 0 High、0 Medium**，且本轮 74 套全量回归零失败——质量曲线稳定收敛。剩余发现均为同族转义链收尾（esc_like 家族第 5 处），无结构性新面。

## 5. 记录不修
- `list_tags_paged`/`search_docs_paged` 的 LOWER() 包列 + lowercase key 对非 ASCII 大小写折叠依赖 SQLite ASCII 语义（既有设计，全平台一致）。
- 其余同 R19 §4。
