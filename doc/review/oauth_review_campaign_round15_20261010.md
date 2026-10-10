# OAuth 复核战役 — 第十五轮（R1xxx）报告（2026-10-10）

## 本轮扫描面

| 代理 | 切片 | 结果 |
| --- | --- | --- |
| r15-a1 | 第 14 轮修复自查 + 门控横扫 | **1 High**（skipAuth 损坏态 fail-open）+ 验证 6 项全过 |
| r15-a2 | rag/ 全目录重读（service/git/chunker/vectordb/extract） | **1 High + 2 Medium + 1 Low + 2 Info** + META_LOCK/导入会话/CAS/sweep 全验证在位 |
| r15-a3 | 修复回归套件 r1500 + 单测 | 20 PASS/0 FAIL/6 SKIP（具名化 R9-R14 修复）+ lib 148（+2） |

## 修复 7 项

| # | 级别 | 内容 |
| --- | --- | --- |
| 1 | **[H]** | `is_skip_auth_enabled` 用 `config_service::get()`——损坏 JSON 被吞成 `Ok({})` → skipAuth=true，auth 部署全部 require_admin 失守；HTTP 面（hub_admin_gate）已 fail-closed 而 Tauri 面 fail-open，两面不一致 → 切 `get_strict()`（NULL 仍 Ok({})，新装不受影响） |
| 2 | **[H]** | `reindex_all` symlink 分支 `read_to_string` 对 GBK/GB18030/UTF-16 源失败 → "" → delete_all + 0 chunks，**模型切换即永久清空索引**（其它读取路径全走 decode_text 唯此处漏）→ 字节读取 + is_likely_text 守卫 + decode_text；不可解码原文件跳过并 warn，绝不 wipe |
| 3 | **[M]** | `plan_source_sync` file 源 original 消失即自动删文档 **连同健康本地 copy 与向量**（拔 U 盘/移动文件即丢内容）→ 仅 symlink 法文档自动删；copy 法降级 lost-original 徽标 |
| 4 | **[M]** | `canonicalize_url` 只缓存真重定向——普通 https 仓库每次调用（delete_doc refcount/每 tick 同步/取消路径）都全价 10s 探测，离线用户串行超时 → identity/离线/SSO 四分支全部入 600s 缓存 |
| 5 | **[L]** | `update_doc` 改名 rename 目标已存在时（Unix 静默覆盖）可毁另一 legacy 命名文档内容文件 → 目标存在即拒绝 rename 并 warn |
| 6 | Info 记录 | ≤64MiB 同步读/解码在 async worker 上跑（可 spawn_blocking，非正确性） |
| 7 | Info 记录 | MCP 错误串嵌入绝对路径（本地面，产品决策改文件名） |

## 套件与验证

- `rmcp_round_r1500.py`：20 用例具名钉死 R9-R14 修复（A 零 pin 门关态/B pin 门开态/C merge_limit no-cut/D OAuth 字段级保存/E get_strict 损坏态 fail-closed 端到端）×2 稳定；6 SKIP 有据（睡眠 on-demand/鉴权上游/IPC-only 门控等）
- 单测 +2：`get_strict_empty_string_is_err_while_get_falls_back_ok`、`path_under_root_normalization_round18_semantics` → **lib 148 passed**
- cargo check 0 错 0 警；tsc 0 错误
- 修复后全量 E2E 回归：见文末统计

## 证伪/验证通过（a2 亲核）

META_LOCK 全部新获取点锁序恒 meta→runtime；delete_doc 在 git refcount 段前显式释放；run_source_sync 逐迭代守卫真释放；zero_all/reindex_all 锁内重读；write_meta_atomic 与 0600 凭证 tmp 路径；pdf 括号深度守卫；chunker 工作栈/超长阀；vectordb 自愈与 dim 预检；keyword_search 反斜杠优先转义；OCR 限额与临时清理；office 光栅计数。

## 收敛分析（回答「为什么每次都能跑出问题 / 什么时候是个头」）

### 缺陷趋势（每轮 High 数 + 来源分类）

| 轮 | High | 来源 |
| --- | --- | --- |
| R9 | 1 | 新面（零 pin 门） |
| R10 | 1 | R9 修复复发（call-lane） |
| R11 | 1 | rmcp 传输层新面（Authorization 双 scheme） |
| R12 | 1 | 新面（损坏配置 fail-open） |
| R13 | 1 Critical | R12 修复自伤（agents_lock 死锁） |
| R14 | 2 | 新面（门控缺失：list_servers/copy-config） |
| R15 | 2 | 1 新面（rag/ 首次全量重读）+ 1 修复失配（R14 的 get_strict 没接到 is_skip_auth_enabled） |

**规律**：自 R10 起「修复合复发」存活期压缩到 ≤1 轮——修复确实在收敛；剩余 High 几乎全部来自**首次覆盖的新切片**（本轮 rag/ 是最后一个未重读的大目录），而不是已覆盖面的回归。

### 剩余风险面（本轮后）

- rag/ 全量重读**完成**（最后一个 ~10k 行大目录）；transport/pool/bridge/http_server/smart/commands/services/前端/迁移均已过至少一轮全量逐行。
- 未覆盖或薄覆盖：rag/extract 的 office/pdf 畸形输入 fuzz 级深读（低危，已有 Limits/守卫）、E2E 套件自身的弱断言审计（第 14 轮已审计一轮）。

### 量化收敛标准（停止条件）

1. 连续 **2 轮**在**已覆盖切片**上 0 新发现（本轮 rag/ 是首次覆盖，不计入）；
2. 所有切片至少完成一次全量逐行（本轮达成 ✅）；
3. lib 单测 ≥148 且全量 E2E 零回归。
满足 1+2 后转为「按需触发」模式：仅在新功能/上游同步后做定点复核，不再无限轮询。

**诚实的回答**：不能一次性处理完，因为每轮修复本身就是新代码（R13 的死锁是 R12 修复引入的），新代码需要一轮验证；但首次覆盖清单本轮已清空，若下一轮（R16）在已覆盖切片 0 新 High，即可宣布收敛并停止无限轮次。

---
*回归统计：*（见 AGENTS.md 本轮条目）
