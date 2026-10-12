# OAuth 复核战役 · 第二十二轮（R22）报告 — 2026-10-11

> 范围：R21 修复合自身复审（Rust×2 + 前端×12）+ 新套件 rmcp_supplement_v42 + 亲核全量 75 套 E2E + 1 High 修复。

## 1. 切片结论

### A1 · R21 Rust 修复合自审 — **零发现（干净）**
- `reconcile_pending` 补 `skill_fs_lock`：唯一调用点 lib.rs:331（后台 spawn，调用前零锁）无重入死锁；全仓核对**不存在任何跨 skill_fs_lock/agents_lock 双锁调用序**（注释声明准确）；锁内最坏 remove_dir_all 几十 MB 阻塞 import/export 数秒 = 加锁目的本身，可接受。
- uv timeout：`start_kill()` 后 child drop → tokio runtime orphan 队列后台 reap（不泄漏 zombie）；stdout/stderr task SIGKILL 后管道 EOF 自然结束；poisoned lock `unwrap_or_default` 降级与既有失败路径一致；early return 正常 drop `_install_guard` 释放 PYTHON_INSTALL_LOCK（正是修复目标）；错误映射与原 `?` 等价。
- 记录：skill_service.rs / commands/runtime.rs 无 `#[cfg(test)]` 模块（历史欠账，非本轮引入）。

### A2 · R21 前端修复合自审 — **1 High（R21 修复自身不完整）+ 11 项干净**
- **[High·已修] `splitArgs` 丢失空引号 token——quoteArg('') 修复 round-trip 不成立**：`quoteArg('')` 产出 `""`，但 splitArgs 闭引号分支 `if (!inQuotes && cur)` 对空 cur 不 push、结尾 `if (cur)` 同样不 push → `["", "--flag"]` 经 textarea 任意编辑重解析后静默变 `["--flag"]`（正是注释声称要防止的「编辑静默改写存储配置」；单个空串参数完全无法表达）。→ `sawQuote` 标志：闭引号无条件 push（含空串）、空格/结尾分支 `cur || sawQuote`。**验证：13 个定向用例（含 `["-e","","code"]`、`["",""]`、连续空串）+ 50k 次模糊 round-trip 全过**；tsc 0 错；build ✓。
- 其余 11 项（RagPage 4M 修复的 ref 时序/方向边界/平台折叠/补条目顺序、epoch/noChanges、installDraftKeys、ServerContext 代际交互 + logout bump + max-attempts 防御路径、ServerCard 双 disposed、GroupCard timer、openapi 守卫、ActivityPage finally）逐项确认干净。

### A3 · 新行为测试补全 — 新套件 v42 + 2 项说明
- **Rust 单测**：quoteArg 空串为纯前端 form 序列化问题（Rust 侧 args 以 JSON 数组直存 DB，无 quote 镜像逻辑，已 grep 确认）→ 无需镜像单测；reconcile_pending 锁不可单测（私有 fn + 需 AppHandle/live DB）→ 补说明性 doc comment；uv timeout 进程交互不可单测。
- **新套件 `rmcp_supplement_v42.py` 9/9**：5 版本 initialize 回显（2026 按既有约定断言降级 2025-11-25——rmcp 设计，非缺陷）+ 2026 discover ttlMs=3600000/public + 宽松裸请求升格 getPublicIp 真实调用 + 严格 DB 热切拒绝/还原放行（try/finally 还原基线）。

## 2. 全量 E2E（亲核，**75 套**逐一执行 = 74 + 新 v42）

| 批次 | 覆盖 | 结果 |
| --- | --- | --- |
| 核心 | full_matrix_v18 262 / matrix_v2 152 / matrix 39 / full_regress 41 / strict 10 | 全绿 |
| 宽松家族 | leniency_v2 15 / pairs 15 / matrix 34 | 全绿 |
| pin/carpet | r111 31 / r460 37 / r461 22 / r462 24 / r200 101 / r300 122 | 全绿 |
| round50+r158 | 33+77+19+18+14+14+12+24 | 全绿 |
| gap/baseline/audit | 43+12+24+22+32+23+15+20 | 全绿 |
| supplement v19-v40 | 22 套 | 全绿 |
| 轮次时代 | r800 18 / r900 36 / r1000 71 / r1100 67 / r1200 45 / r1300 46 / r1400 99 / r1500 20 / r1600 33 / r1700 42 / apps_ui 7 / r1800 21 / r1900 23 / r458 16 / r51 35 / round100 44 | 全绿 |
| 公网IP | public_ip_matrix 10/10 | 全绿 |
| **新** | **supplement_v42 9/9** | 全绿 |

每版本（5 个）× 每通道 × 宽松/严格 + 公网 IP MCP 真实调用全覆盖。cargo test --lib **152 passed**；cargo check 0 错 0 警；tsc 0 错；build ✓。

## 3. 「什么时候是个头」——R22 答案

本轮把 R21 的 14 个修复合全部回炉：Rust 侧零问题，前端侧抓到 1 个 High——**且恰好是 R21 修复自身不完整**（修了 quoteArg 没修对偶的 splitArgs）。「修复合一轮后必须以自身是否引入新 bug 的视角重读」这条教训第 N 次应验（R26 4 项全属此类 → R21 → R22）。对偶函数/对称面必须同步修复并配 round-trip 断言——本轮已用 50k fuzz 钉死。覆盖面层面：R21 已补齐全部切片，本轮无新切片可扫，发现数为 1（且在修复自身）——**曲线已触底**。

## 4. 记录不修
- 2026 initialize 降级 2025-11-25 返回 200（rmcp 3.4.1 设计，v42 套件按既有约定断言）。
- skill_service/runtime 无单测模块（历史欠账，Tauri harness 缺失，记为集成测试待办）。
