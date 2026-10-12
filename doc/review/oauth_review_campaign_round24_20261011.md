# OAuth 复核战役 · 第二十四轮（R24）报告 — 2026-10-11

> 范围：R23 修复合回炉（4 处逐行 + 交互推演）+ 全量 75 套 E2E + 1 Medium 修复 + 测试数据清理。

## 1. A1 · R23 修复合回炉 — **1 Medium（已修）+ 3 项干净**

1. disablePasswordLogin 重复块删除：干净（单块在位、JSX 结构完好、tsc 通过）。
2. ServerContext `++pollRunRef` 推进：四条路径逐项推演干净——启动期 stale 提前 return 不会卡 isInitialLoading（更新代际 run 必然接管置 false）；logout/重登录无死锁；页码切换 cleanup 先清 interval 再 ++ 使旧闭包 stale；maxAttempts 单次启动正确。
3. 顶层 passthroughHeaders String 回退：干净——保存路径 `parseCommaSeparatedList` 恒拆回 `string[]`，Rust `Option<Vec<String>>` 不会拒；回退仅影响表单展示。
4. **[M·已修] pathUnderRoot ASCII fold 丢了平台判断**——R23 修复让前端恒折叠，但后端 `path_under_root` 仅在 `cfg!(windows)||cfg!(target_os="macos")` 折叠、Linux 不折叠（ext4 大小写敏感）→ Linux 上 UI 判定「已排除」而后端照常索引，静默背离（R23 修复把本来一致的平台变成不一致）。→ 两副本恢复平台判断（win/mac 才折叠）+ 保留 ASCII-only fold。tsc 0 错；build ✓。

**「修复合的修复合」模式第 4 次应验**（R26→R22 splitArgs→R23 passthrough 对称面→R24 平台判断丢失）：每次修 fold/归一这类平台相关逻辑，改动时必须重新对照全平台分支表。

## 2. 测试数据污染发现与清理（非产品缺陷）

r800/r1100/r1700 首跑 3/5/2 失败：`too many registered clients — revoke unused ones first`——多轮 OAuth 套件累积 232 个测试客户端（r300×170、E2E Claude×32、r800×30）触发动态注册上限。清理 232 个测试客户端后**三套件复跑全绿**（18/67/42）。这是套件生命周期管理缺口：记录为套件待办（应注册即清理或按名称前缀自清），非产品 bug。

## 3. 全量 E2E（亲核，75 套，~2000+ 断言全绿）

核心（262/152/39/41/10）+ 宽松（15/15/34/23）+ pin/carpet（31/37/22/24/101/122）+ round50/r158（211）+ gap/baseline/audit（186）+ supplement v19-v42（23 套）+ 轮次时代 r800-r1900（575）+ 公网IP 10/10。修复后复验：full_matrix_v18 262 + public_ip 10 + r109 32 + pin 31 + 契约 0 mismatch。

## 4. 收敛终局判定

R24 回炉 R23 的 4 处修复：3 干净 + 1 处 R23 自身引入的平台回归（已修）。**修复合回炉发现率：R22 1 High → R23 3M+1L → R24 1M，且 R24 的 M 仍是「修复时丢失既有正确逻辑」而非新逻辑错误**。协议核心连续 6 轮零发现；业务逻辑零新发现；本轮唯一产品侧修复是把 R23 弄丢的平台分支放回去。至此：扫描面 ✓、修复合收敛进行中（发现均为回退类）、测试基建 ✓（唯一缺口=oauth client 生命周期，已记录）。**R25 起若回炉发现归零，战役可正式收官。**
