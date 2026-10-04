# R491+ 复核第二十三轮报告（2026-10-03）

> 范围：服务器生命周期核心（mcp_manager / server_service / group_service）+ 前端表单→payload→Rust 契约（serverFormPayload / ServerForm / ServersPage）。2 个独立复核代理 + 亲核修复。

## 1. 修复清单（6 项）

| # | 位置 | 问题 | 修复 |
| --- | --- | --- | --- |
| C1 | `models/server.rs:175` + `server_service.rs:574` + `settings_import.rs` | **Critical**：`passthrough_headers: Option<HashMap<String,String>>` vs 前端恒发 `string[]`——sse/streamable-http 每次保存/新增都因 serde「sequence expected map」整单失败（UI-only 路径，HTTP E2E 不可见，历轮漏网根源） | Rust 改 `Option<Vec<String>>`（origin 类型一致），DB 读写/导入器/单测同步 |
| C2 | `commands/servers.rs delete_server` | disconnect 在 DB delete **前**——窗口期 30s session-rebuild 看到仍 enabled 的行重新 connect → 删除后活连接/子进程孤儿 | delete 提交后再 disconnect（闭合 TOCTOU） |
| C3 | `ServerForm.tsx:87` | args 含空格被破坏：init `join(' ')` + submit `split(' ')`——`--header Content-Type: application/json` 每次编辑都被静默重写（连接相关字段还触发重连） | quote-aware `quoteArg`/`splitArgs` round-trip（node 实测含引号/转义用例） |
| C4 | `ServerForm.tsx:97` | `passthroughHeaders?.join` 对非数组（旧对象形数据）抛 TypeError 崩编辑弹窗 | `Array.isArray` 守卫 |
| C5 | `ServerForm.tsx:1201` | 负数 timeout 直达 serde u64 → 整个保存被拒（`-5` truthy 绕过 `\|\| 60000`） | `Math.max(1000, ...)` clamp |
| F1 | 套件 | v23 初版严格模式写错键（`mcpServer` vs `mcp` 命名空间，R22 已踩） | 套件修正 |

## 2. 记录不修

- 服务级 OAuth 配置：UI 收集但 Rust `ServerConfig` 无 `oauth` 落点（R10 已裁定「需产品决策」）——继续记录。

## 3. 证伪（代理疑点亲核排除）

- starting 永久悬挂（pool 失败分支恒置终态）、rebuild 双 spawn（CONNECT_LOCKS + starting 占位）、disable×快速 toggle 全序、group 级联保留对象成员、FTS ref_id 一致性、update rename 拆除顺序、create TOCTOU（UNIQUE 索引友好映射）、keepAliveInterval round-trip、idleTimeoutMs 负值读侧归一、ServersPage 搜索竞态守卫。

## 4. 新套件

`scripts/e2e/rmcp_supplement_v23.py`（16 项）——版本回显 × 通道矩阵 + 生命周期观测：
- 请求版本==响应版本 × 4 legacy 版本 × root/scope/group 3 通道（12）
- 未知版本 fallback 2025-11-25；2026 initialize 协商 legacy（2）
- 服务器禁用/还原后工具列表观测（2，DB 直改+还原，缓存容忍）

## 5. 验证结果

| 项 | 结果 |
| --- | --- |
| cargo check --lib | 0 错 0 警 |
| cargo test --lib | **96 passed** |
| tsc | 0 错误 |
| **27 套 E2E** | **1136/1136 全绿**（重建二进制重启后全量重跑） |

## 6. 经验沉淀（为什么 UI-only 契约错能存活 20+ 轮）

1. **测试盲区 = Tauri 命令参数反序列化**：E2E 全打 HTTP 面，UI 保存走 invoke——serde 在 invoke 参数层就失败，错误只进前端 toast。补法：对每个前端 payload 字段与 Rust 模型做**类型级 diff 脚本**（本轮 C1 即此类），比人工阅读可靠。
2. **join/split 对偶是静默数据破坏的经典模式**：任何「数组 ↔ 字符串」编辑器都要用含空格/引号的 round-trip 用例验证。
3. **删除路径也要闭合 TOCTOU**：R17/R20 修了 enable/disable 的 post-check，delete 的对称缺口本轮才补——生命周期三态（增/改/删）应作为一组检查而非逐个。
