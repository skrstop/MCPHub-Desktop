# 相关度搜索（Smart Routing Relevance Search）100 轮独立复核报告

> 日期：2026-11-11 ｜ 特性提交基线：`2c716e4`（特性）/ `7e9f0c3`（复核 diff 基线）
> 形式：10 批次 × 10 个独立 code-review 代理（R1–R100），每轮独立视角、互不知晓；每批结束后亲核修复并回炉验证。

## 1. 特性概述

ServersPage 工具栏新增「相关度搜索」按钮（`isTauri() && smartRoutingConfig.enabled` 门控）→ `SmartSearchDialog`：范围选择（全部 / 分组 / 单服务器）+ 查询输入 → `smart_routing_search` 命令（require_admin）→ 混合检索（向量 + 关键词）→ 带评分结果渲染。

关键设计（均有单测钉死）：
- **范围 deny-by-default**：`group:`/`server:` 前缀空值/未知值 → 空候选集，绝不静默降级全局；裸名未知 → 单服务器范围；`group:web` 同名组仅可经双前缀触达。
- **阈值有意差异**：对话框传 `threshold_override=Some(0.0)`（人类浏览看得到低于阈值匹配、附评分自行判断）；`$smart` 元工具传 `None` 保持配置阈值（路由决策保守）。
- **空索引区分**：0 结果时探测 `smart_routing_performance`，`toolRows===0` → 「索引为空，请重建」提示（区别于真实无匹配）。
- **截断提示**：`effectiveLimit = min(20, maxResults)`，结果数触顶时提示「仅显示前 N 条」。
- **焦点管理**：捕获 trigger → effect 内聚焦输入框（先捕获后聚焦，避免 autoFocus 吞掉捕获）→ 关闭归还焦点（CopyClientConfigDialog 约定）。
- **哨兵本地化**：`mv runtime not running` / `smart routing is not enabled` 经 `localizeSmartSearchError` 映射 i18n，未知错误原样透出；契约测试以 `anyhow!("...")` 带引号字面量钉死 Rust 源（含 search.rs 对话框路径副本）。

## 2. 测试清单与通过率

### Rust（`cargo test --lib smart_search`：**23 passed / 0 failed**）

| 组 | 用例 |
| --- | --- |
| 范围解析 ×16 | all/空/None 全局、group 前缀按 id/名展开、前缀空值/未知 deny、server 前缀 verbatim（含 `server:all`、含冒号名）、前缀空值 deny、裸组名/组 id、裸未知→单服务器、legacy 字符串成员、trim、双前缀可达性 ×2 |
| 查询归一 ×3 | trim、空/纯空白拒绝、保留内部空白 |
| Payload serde ×4 | camelCase、默认值、未知字段容忍、错误类型拒绝 |

### 前端（`npx vitest run`：**12 files / 137 passed / 0 failed**）

| 文件 | 数量 | 覆盖对象 |
| --- | --- | --- |
| `SmartSearchDialog.test.tsx`（jsdom + RTL） | 20 | 组件全状态机：空查询守卫、Enter 搜索、verbatim scope、stale-scope 丢弃（成功+错误双路径）、dangling scope 回退、错误本地化、非 Error rejection、空态/索引空/探测失败回退、截断提示、maxResults 钳制、textContent 回退、焦点归还、Escape 关闭 |
| `smartRoutingService.search.test.ts` | 22 | searchSmartRouting + 既有 smart 服务函数（成功/失败/error/message/网络异常/payload 形状） |
| `smartSearchScope.test.ts` | 7 | 范围构建（空输入、组 id 作 value、含冒号服务器名） |
| `smartSearchSentinels.test.ts` | 8 | localizeSmartSearchError 全分支 + Rust 源 anyhow! 字面量契约（mv/mod.rs + search.rs 双文件） |
| `tauriClient.smartSearch.test.ts` | 7 | mapRestToCommand 映射（payload 包装/默认值/null body/异方法响亮失败）+ transformTauriResponse（数组/非数组/空数组） |

### 覆盖率（新代码 3 文件，全量套件下 v8 实测）

| 文件 | Statements | Branches | Functions | Lines |
| --- | --- | --- | --- | --- |
| `components/SmartSearchDialog.tsx` | 100% (72/72) | 100% (47/47) | 100% (16/16) | 100% (64/64) |
| `utils/smartSearchScope.ts` | 100% | 100% | 100% | 100% |
| `services/smartRoutingService.ts` | 100% | 100% | 100% | 100% |

**覆盖率与通过率均 100%**（新代码文件全指标；全部 160 测试零失败）。

### 其他验证（R95/R100 终验）

- `tsc --noEmit` 0 错误；`npm run build` ✓；`cargo check` 0 错 0 警；`cargo test --lib` 全量 411 passed；`cargo clippy` 新代码 0 警告；`check_frontend_rust_contract.py` 0 mismatch；release.yml YAML 合法。

## 3. 100 轮总账

| 批次 | 轮次 | 视角 | 判定 | 修复 |
| --- | --- | --- | --- | --- |
| 1 | R1–R10 | 契约/边界/i18n/错误路径/命令注册 | 6 CLEAN | scope 前缀显式解析、scope 变更清结果、错误本地化、es/vi locale、normalize_search_query 提取 |
| 2 | R11–R20 | 竞态/a11y/暗色/嵌入权重/web 模式 | 6 CLEAN | scopeRef 竞态守卫、dialog 语义+Escape+autoFocus、错误卡配色 |
| 3 | R21–R30 | 前缀歧义/渲染/服务层/回归 | 9 CLEAN | 前缀歧义 doc + 2 pinning 测试 |
| 4 | R31–R40 | 测试质量/文档准确性 | 7 CLEAN | localizeError 提取+契约测试、score [0,1]→2.0 文档 ×3 处 |
| 5 | R41–R50 | 错误 UX/i18n/Rust API/props/limit/IPC/perf/es-vi/scope 边界/测试质量 | 6 CLEAN | dangling scope 回退 effect、截断提示+locale ×6、isTauri 门控、tauriClient 映射测试、组件测试（RTL+jsdom 基建） |
| 6 | R51–R60 | a11y/后端性能/暗色/错误分类学/scope 语义/locale 完整性/门控/Rust 测试/服务层/回归 | 6 CLEAN | 焦点归还、error-box 类、空索引提示+locale、禁用服务器过滤、误导测试名改名+serde 测试、effectiveLimit |
| 7 | R61–R70 | 焦点时序/tsconfig/CI/阈值/空索引/scope UI/Rust 文档/覆盖率/测试 mock/终验 | 4 CLEAN | autoFocus→effect 聚焦（修捕获时序）、node types 收窄+三斜线引用、CI npm test 步骤、threshold_override 参数、toolRows 语义、hint count=effectiveLimit、indexEmpty 重置、覆盖率补齐至 100% |
| 8 | R71–R80 | 阈值正确性/焦点/CI/空索引探测/locale 脚本安全/mock 保真/Rust 文档/终态/回归 | 8 CLEAN | 探测改 toolRows、空态测试显式 mock、焦点归还测试 |
| 9 | R81–R90 | memo 轮询/哨兵健壮性/Rust 测试终审/前端终审/a11y 终审/i18n 终审/安全/文档/对抗输入/提交就绪 | 6 CLEAN | 暗色 error-box 文字色（High）、scoreHint locale ×6（High）、哨兵钉 anyhow! 字面量、*.tsbuildinfo gitignore、AGENTS.md 条目、changelog 新功能节 |
| 10 | R91–R100 | 暗色修复回炉/scoreHint 回炉/哨兵回炉/changelog 回炉/全量验证/记录项复核/clippy/测试报告输入/终审 | 9 CLEAN | search.rs mv 哨兵补钉、暗色文字去 alpha 提对比度 |

**发现趋势**：批次 1–5 高密度（每批 2–5 项）→ 批次 6–8 收敛（每批 1–4 项）→ 批次 9–10 仅 2 High（均为前轮修复的伴生缺口：scoreHint 漏 locale、暗色对比度）→ 末批 9/10 CLEAN。修复的修复均被后续轮回炉验证。

## 4. 记录不修（有据不修）

| 项 | 轮次 | 理由 |
| --- | --- | --- |
| `mv::with_model` 嵌入在 tokio worker 同步执行、持 runtime_slot 锁 | R52 M | **存量**，与 `$smart` 元工具路径完全一致；延迟风险非死锁；修复需重构共享 mv 运行时，风险大于收益 |
| merge_hits keyword-only 去重 O(n²) | R52 L | 上限 KEYWORD_FETCH_CAP=10000，桌面索引规模（百~千行）实际不可达 |
| ServersPage 无条件 `/groups` 拉取 | R47/R60 L | 本地 IPC 一次轻量请求；门控需改 useGroupData 契约 |
| 非管理员（多用户模式）按钮死端 + 原始英文错误 | R75 L | 桌面默认 skipAuth（游客即 admin）；仅显式开启多用户时出现 |
| A→B→A scope 往返的陈旧响应渲染 | R44 L | 渲染的是当前 scope 的正确结果，无跨 scope 泄漏；注释已收敛措辞 |
| 焦点陷阱（Tab 可离开 dialog） | R51/R100 | 与全仓 dialog 基线一致（无一处 trap） |
| 截断提示恰在 limit 整倍时误报 | R79 | 启发式，注释已声明「likely truncated」 |
| 命令体 `Some(0.0)` 决策无直接单测 | R83 L | 薄组合 + 注释 + merge_hits 阈值测试覆盖；提取 helper 价值边际 |
| 查询长度无上限 | R87 L | 本地单用户 admin-gated；embed 有 max_context 截断，最坏自伤慢查询 |

## 5. 基建变更（随本战役落地）

- 前端测试栈：`vitest` + `@vitest/coverage-v8` + `jsdom` + `@testing-library/{react,dom,user-event}` + `@types/node`（均 frontend devDeps）。
- `release.yml` 新增「Run frontend tests」步骤（deps 安装后、构建前，失败阻断发布）。
- `tsconfig.node.json` 加 `"types": ["node"]`（vite.config.ts 需要）；应用 tsconfig 不泄漏 node 全局（哨兵测试用三斜线引用按文件引入）。
- `frontend/.gitignore` 加 `*.tsbuildinfo`。
- `index.css`：`.dark .error-box` 补文字色（唯一消费者即本对话框错误卡）。

## 6. 结论

100 轮独立复核收官：**160 测试（Rust 23 + 前端 137）全绿，新代码 3 文件覆盖率 100%（全指标），通过率 100%**。跨语言契约（scope 前缀、哨兵字面量、serde camelCase、响应形状）双侧测试钉死；末批终验 READY（唯一 blocker 为 3 个未跟踪测试文件需 git add，属提交操作非代码缺陷）。
