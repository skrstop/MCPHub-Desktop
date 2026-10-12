# OAuth 复核战役 · 第二十五轮（R25）报告 — 2026-10-11 · **收官轮**

> 范围：R24 修复合回炉 + OAuth 套件生命周期缺口落地 + 全量 75 套 E2E。**回炉发现 = 0，战役正式收官。**

## 1. A1 · R24 fold 修复回炉 — **零发现（干净）**

逐项核对（对照 service.rs:524-539 逐行）：
- 语义四项一致：`\`→`/` 归一、尾斜杠剥离、空根 false、ASCII-only 折叠且仅 win/mac 分支（后端 `eq_ignore_ascii_case`+`to_ascii_lowercase` 等价）。
- `navigator.platform` 三平台判定正确：macOS WKWebView `MacIntel`→折叠、Windows `Win32`→折叠、Linux WebKitGTK→不折叠（与后端 ext4 分支对齐）。
- 两副本逐字节一致；7 处调用点语义不受影响；tsc 0 错。

## 2. A2 · OAuth 套件客户端自清（R24 记录的缺口落地）

- 共享 helper `cleanup_oauth_clients(con, prefixes)`（rmcp_round_r1100.py）：按 `name LIKE '<prefix>%'` 级联删 `oauth_codes`/`oauth_tokens`/`oauth_clients`，只接受套件自有前缀。
- 接入 4 套件的预清理 + finally：r800（此前 finally 完全无清理——232 客户端累积元凶之一）、r1100、r1700（r1700-e2e）、carpet_r300。
- r1600 核查后确认无 OAuth 注册，未触碰。
- 验证：py_compile 全过；r800 实跑 18/0/1 + 跑完 `r800%` 计数 = 0；库中残留 3 个旧 r300 客户端待下次套件运行自动清除。

## 3. A3 · 全量 E2E（亲核，75 套，~2000+ 断言全绿）

核心（262/152/39/41/10）+ 宽松（15/15/34/23）+ pin/carpet（31/37/22/24/101/122）+ round50/r158（211）+ gap/baseline/audit（186）+ supplement v19-v42（23 套全绿）+ 轮次时代 r800-r1900（575，r800 已含自清改造后复跑全绿）+ 公网IP 10/10。tsc 0 错；build ✓。

## 4. 战役总账（R17 宣告收敛 → R25 收官）

| 轮次 | 视角 | 产品修复 |
| --- | --- | --- |
| R18 | 新基线同步代码（SSRF/ui://） | 1H2M |
| R19 | R18 修复回炉 + SKIP 验证 | 1 真 M（SKIP 掩盖 17 轮的老缺陷） |
| R20 | 迁移完整性 + rag/mv/smart + 前端 OAuth | 1L |
| R21 | commands/services 二线/前端页面层（切片补齐） | 2 Rust + 6M+6L 前端 |
| R22 | R21 修复回炉 | 1H（splitArgs 对偶） |
| R23 | 近三轮 diff 横切 + 一致性 | 3M+1L + 文档 3 处 |
| R24 | R23 修复回炉 | 1M（平台分支回退） |
| **R25** | **R24 修复回炉 + 套件基建** | **0 产品缺陷（仅测试基建）** |

**「什么时候是个头」最终答案**：发现曲线 R18 1H → R22 1H → R23 3M → R24 1M → **R25 归零**。所有发现均可归因：新代码（R18）、首次达深的切片（R21）、修复合自身（R22-R24 递减）、非产品（R25）。协议核心面自 R458-R490 深扫后连续 8 轮零发现；rmcp 迁移完整性、五版本×全通道×宽松严格、pin/OAuth/smart 字段/MCP Apps/ui:// 全特性均有多套件持续护航。**战役正式收官。**

## 5. 遗留待办（非缺陷）
- OAuth 套件残留 3 个旧 r300 客户端（下次套件运行自动清）。
- skill_service/runtime 无单测模块（需 Tauri harness，集成测试待办）。
- R23 记录的 2L（未闭合引号幻影空条目、引号内 `\` POSIX 差异）。
- R21 记录的 12L UX 项。
