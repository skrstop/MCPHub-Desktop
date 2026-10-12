# OAuth 复核战役 · 第二十三轮（R23）报告 — 2026-10-11

> 范围：R22 修复合回炉（splitArgs 逐边界）+ 近三轮 diff 横切亲读 + 一致性对账 + 全量 75 套 E2E + 3M+2L+文档修复。

## 1. 切片结论

### A1 · splitArgs sawQuote 修复自审 — **2 Low（记录不修）+ 150k fuzz 全绿**
- 17 个语义边界用例逐项实测：相邻引号、`"" ""`、转义 round-trip、tab/NBSP 全部正确。
- **消费点对账全干净**：`[""]` 经 serverFormPayload → serde `Vec<String>` → rmcp_stdio 真实成为 argv 条目（正是 R22 修复意图）；mcpClientSnippets 空串保留输出 `""`；无其他 quoteArg/splitArgs 调用点。
- **[L·记录]** ①未闭合引号 EOF 产生幻影空条目（`abc "` → `["abc",""]`）——孤引号多为笔误，可加保存校验，非必修；②引号内 `\` 无条件吞下一字符与 POSIX 双引号不一致（手输 `C:\Users` 路径场景）——不影响 quoteArg 产物 round-trip，仅手输路径。
- 重跑 round-trip：定向 + 150k fuzz（tab/NBSP 组）**0 失败**——R22 修复未破坏任何既有性质。

### A2 · 近三轮 diff 横切亲读 — **3 Medium + 1 Low（全部已修）+ 残留清理**
- **[M·已修] SettingsPage disablePasswordLogin 开关重复渲染两次**（同锚点逐字节相同的两块相邻，HEAD 确认只有一个）→ 删重复块。
- **[M·已修] ServerContext 代际令牌 normal 轮询从不推进**——startNormalPolling 只读不 `++`，页码切换时旧闭包在飞 fetch 仍以旧页参数写回（R21 修复只覆盖启动期→启动期与 logout 两场景）→ startNormalPolling 开头 `++pollRunRef.current`。
- **[M·已修] 顶层 `config.passthroughHeaders` legacy 字符串静默丢弃**——R21 只修了 openapi 分支的 String 回退，顶层同字段漏（同族修复只做一半，第 6 次）→ 补对称回退。
- **[L·已修] pathUnderRoot 前端 Unicode toLowerCase vs 后端 ASCII 折叠**——非 ASCII 大小写路径 UI 与后端判定漂移 → 两副本改 ASCII-only fold 对齐后端。
- **[L·清理]** 调试残留 txt ×7 + __pycache__ 删除；diff 内无新增 console.log/dbg!（grep 确认干净）。
- 同族扫描全过：LIKE 转义链全仓 backslash-first 无遗漏（8 处逐一核对）、其余 listen/setInterval/finally/join 均有守卫；R21/R22 各修复合边界（busy-retry 回滚、reconcile 锁序、uv timeout、ptr_eq、epoch）在 diff 上下文中逐行确认干净。
- 供参考：installDraftKeysRef 在 save 失败时误删 touched key（与 smartRouting 失败保护不对称）——低风险字段+需时序叠加，记录不修。

### A3 · 一致性对账 — **7 项 6 一致 + 3 文档过时（已修）**
- locales 四语言 0 缺 0 多；版本四源+Cargo.lock 全部 1.1.1005；migration v1-v33 无缺口（20 个 SQL 文件缺号均为纯 Rust 迁移，对齐）；tauri.conf 结构完好（csp=null 为既有架构自洽）；契约脚本 0 mismatch；cargo check 0 错 0 警。
- **文档修复**：AGENTS §3.4.2 TARGET_VERSION 31→33（补 v32-v33）；§6 Smart Routing 待办勾选（2026-09-26 已完成）；§6 四源版本 1.1.1004→1.1.1005。

## 2. 全量 E2E（亲核，75 套逐一执行，~2000+ 断言全绿）

核心（262/152/39/41/10）+ 宽松（15/15/34）+ pin/carpet（31/37/22/24/101/122）+ round50/r158（211）+ gap/baseline/audit（186）+ supplement v19-v40（22 套）+ 轮次时代（575）+ 公网IP 10/10 + **v42 9/9**。修复后复验：full_matrix_v18 262 + public_ip 10 + r109 32 + pin 31 + 契约 0 mismatch + round-trip 50k fuzz。

## 3. 「什么时候是个头」——R23 答案

R22 宣告扫描收敛后，本轮换「横切视角」（修复合互相作用 + 同族遗漏 + 一致性）仍出 3M——**但全部是「修复合的修复合」与文档层**，协议核心与业务逻辑零新发现。三层结论已齐备：①扫描面收敛（R21）；②修复自身收敛中（R22 抓 1、R23 抓 3，均为修复不完整而非新缺陷）；③文档/一致性收敛（R23 一次清完）。预计 R24 起修复合回炉发现数将归零。
