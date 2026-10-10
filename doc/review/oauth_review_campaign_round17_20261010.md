# OAuth 复核战役 — 第十七轮（R17xx）报告（2026-10-10）— 收敛判定轮

## 本轮扫描面

| 代理 | 切片 | 结果 |
| --- | --- | --- |
| r17-a1 | R16 四项修复自查 | **1 Medium + 1 Low + 1 Info** + 4 项验证全过 |
| r17-a2 | E2E 弱断言全量审计（65 套，7 类缺陷模式） | **8 项发现（全在测试侧，产品代码零缺陷）** + 60 套零发现 |
| r17-a3 | 套件 r1700 + 冒烟 | **39/39**（新二进制）+ 冒烟 3 套全绿 |
| r17-a4 | 8 项套件加固落地 | 8/8 + 1 项后续发现（r109 写 null）亲修 |

## 产品修复 2 项（均 R16 修复自查发现）

| # | 级别 | 内容 |
| --- | --- | --- |
| 1 | **[M]** | R16 NAME_COLLISION 守卫盲区：裸 `{id}` 探测对 upload 文档（`{id}.{ext}`）失效——①合法改名可误报 ②改名块 warn-skip 在盲区把 `meta.name` 持久化 → content_path_for 解析到他文档文件。守卫重构为「`content_path_for(old)` 解析到裸名路径且存在才触发」（upload 天然豁免）；改名块 warn-skip 升级硬 Err（互补覆盖 upload 场景） |
| 2 | **[L]** | 大小写改名（"Report"→"report"）在不区分大小写文件系统上误报碰撞 → `eq_ignore_ascii_case` 短路 |
| - | Info 记录 | 遗留 pre-P2b 同名文档今天仍可能共享 content 路径（read/reindex/delete 错位）——诊断性启动扫描记待办；OCR OnceLock 失败永久缓存（重启生效，符合探测一次设计） |

## 测试基建修复 10 项（弱断言审计 + 竞态根治）

| # | 类 | 内容 |
| --- | --- | --- |
| 1-2 | 状态污染 | r461 六字段循环 + pin 种子全部纳入 try/finally（此前中途崩溃泄漏部分配置/残留 pin——正是 round-8 级联的成因类） |
| 3 | 恒真式 | oauth_upstream:401 上游断言第二析取支包含第一支（塌缩为 400≤st<600）→ 收紧；**收紧后暴露真实映射**：REST 面对不可达上游返回 500+"not connected"（文档化映射）→ 断言改为接受 200/401/502 + 该 500 形态，拒绝裸 5xx |
| 4 | 弱恢复断言 | r1600 A3/A4 `st != 401` → `st == 200` |
| 5 | pin 泄漏 | r458 main 循环 try/finally unpin（KeyboardInterrupt/sys.exit 安全） |
| 6 | 模式无关断言 | r458 `len in (2,3)` → 读实际 progressiveDisclosure 后精确断言 |
| 7 | 基线还原错误 | 4 套（matrix_v2/strict_matrix/leniency_v2/v22）硬编码还原 False → 捕获 pre-run 基线 + finally 还原；matrix_v2 先捕获后变更；v22 的 `set_strict(set_strict(False))` 惯用语实际写 JSON null → 修复 |
| 8 | 无保护全局切换 | r1400 路由矩阵 try/finally + cfg_snapshot_path |
| 9 | 亲修后续 | r109 同款 `set_strict(set_strict(False))` 写 null → set_strict 返回 prev + ORIG_STRICT 基线捕获 |
| 10 | 测试竞态根治 | config_service 4 个 get_strict 测试共享文件池快照/还原互踩（sqlx row 解码 panic，两次全量复现）→ CONFIG_TEST_LOCK 互斥后**默认并行 148/148 稳定** |

## 套件与验证

- **rmcp_round_r1700.py 39/39 + 5 具名 SKIP**：A 组守卫双布局验证（by-name 碰撞 → -32603 整单拒绝；by-uuid 改名成功 + 双文档内容回读校验）；B CJK 解码链真实索引；D 契约脚本 CI 化；**E 收敛扫 5 版本 initialize 回显精确 + 真实公网 IP 调用全过**；F 宽松 4 版本裸请求放行；G OAuth 全 headless 生命周期（register→consent→code→refresh）
- 加固后受影响 9 套全部复跑绿：r461 22/22、r458 16/16、r1400 99/99、matrix_v2 152/152、strict 10/10、leniency_v2 15/15、v22 16/16、oauth_upstream 10/10、r109（含 null 修复）
- 冒烟：r1600 31/31、r1500 20/20、pin_matrix 31/31
- cargo check 0 错 0 警；**cargo test --lib 148 passed（默认并行，竞态已根治）**；契约脚本 OK
- 基线终态：strictValidation=false（双层）+ exposeHttp=true + pins=0 + health 200

## ✅ 收敛判定达成

R15 预设停止条件：
1. **连续 2 轮已覆盖切片 0 新 High**：R16（rag 切片外已覆盖面 0 High）+ R17（0 High——本轮 1M 为 R16 修复自身的边界精化，非新缺陷类）→ **达成**
2. **全部切片至少一轮全量逐行**：R15（rag/）+ R16（extract/models/tray/lib/tauriClient）→ **达成**
3. lib 单测全绿 + 全量 E2E 零回归 → **达成**（见回归统计）

**结论：战役收敛。**「为什么每次都能冒出问题」的最终答案：每轮修复是新代码需一轮验证（R13/R16 两次自伤证明），加上首轮覆盖的新切片总有盲区——两者均已耗尽：修复链收敛（存活期 ≤1 轮）、覆盖清单清空、断言面经本轮审计可信。

**后续模式**：不再无限轮次；新功能/上游同步 → 定点复核 + 对应套件；遗留待办（legacy 同名诊断扫描、cloud 死链复活、OAuth RFC8707 resource 字段等）见 AGENTS.md 记录项。

---
*全量回归统计见 AGENTS.md 本轮条目。*
