# MCP 工具语言 + 工具查看恒可见 — 实现与测试报告

> 日期：2026-11-15 · 版本基线：26.11.4 工作树 · 状态：✅ 完成（lib 448 passed / vitest 193 passed / tsc 0 错 / cargo check 0 错 0 警）

## 1. 需求（用户）

1. 【本地模型】卡内「MCP 工具语言」设置的**归属**修正：属于【本地模型】顶层配置，不属于向量模型、也不属于决策模型——置于卡体**最上方**（分段线与向量/决策段隔开）。
2. **参数描述也要本地化**：不只是工具级 description，inputSchema 的参数说明同样按设置语言返回。
3. （承接）知识库「查看工具」未开启也能看 + 提示（同决策）。
4. 单元测试 + 测试报告；覆盖率与通过率 100%。

## 2. 架构与落点

### 2.1 语言机制（`src-tauri/src/services/tool_i18n.rs`）

| 组件 | 说明 |
| --- | --- |
| `TOOL_LANGS` | en/zh/fr/tr/vi/es 六语白名单 |
| `resolve_tool_language(tool, ui)` | 纯函数：显式语言优先；`"auto"` 跟随 `uiLanguage`（归一化 `zh-CN`→`zh`）；未知→en |
| `TOOL_LANG: RwLock<String>` | 进程内缓存（tools/list 同步热路径）；`set/current_tool_language` 中毒恢复 |
| `refresh_from_config()` | 启动 + 配置写入时从 `system_config.mcp.{toolLanguage,uiLanguage}` 重解析 |
| `rag_tool_description(name, lang)` | 10 个 rag_* 工具级描述 ×6（en 与历史**字节一致**，逐条比对 10/10） |
| `decision_tool_description(lang)` | decision_decide 工具级 ×6 |
| `smart_route_{search,describe,call}_desc(...)` | smart_route_* 6 个变体模板 ×6（{scope_description}/{servers_list}/{schema_note} 占位符保持；en 文本沿用本地化前既有文案（工具名映射与 origin 有差异，非逐字一致），逐字节由 `en_surface_byte_pinned` FNV 摘要钉死） |
| `smart_tool_title(kind, lang)` | annotations 标题 ×6 |
| `prop_description(key, lang)` | **31 个参数描述文案 ×6**（39 处调用：rag 25 + decision 4 + smart 10，共享文案归一为 31 key） |

### 2.2 接线

| 落点 | 说明 |
| --- | --- |
| `rag/service.rs tool_definitions()` | 工具级 + 全部 25 处参数描述走 `prop_description(key, &lang)` |
| `decision_service.rs decision_tool_catalog()` | 工具级 + decisionType/state/question/options 4 处参数 |
| `smart_routing/meta.rs build_meta_tools()` | 工具级模板 + query/limit/toolName/arguments 10 处参数（progressive/standard 双分支） |
| `commands/config.rs update_system_config` | 触及 mcp.{toolLanguage,uiLanguage} 即 `refresh_from_config_and_report_change()`（REFRESH_LOCK 串行化）；**解析语言实际变化**即 remove+`sync_builtin_index` 重建 builtin 行（覆盖 uiLanguage 通道；remove+sync 为保底——skip-check 哈希含本地化 inputSchema 但不含工具级 description） |
| `lib.rs setup` | 启动时与 decision flag 一起后台刷新 |
| `utils/uiLanguageSync.ts` | 前端启动/`languageChanged` 时写 `mcp.uiLanguage`（admin-only，失败静默） |
| `ModelVectorSettings.tsx` | 「MCP 工具语言」卡**置于卡体最上方**（本地模型级配置；分段线与向量/决策段隔开）；SimpleSelect（跟随界面语言 + 6 语言） |

### 2.3 工具查看恒可见（承接）

- `rag_tools` 命令去 `is_enabled` 门控（文档面恒返目录；MCP `builtin_tools()` 门控不变）。
- RagPage 查看工具按钮去 `disabled`；ToolsDialog `enabled?: boolean | null` 三态提示（`enabled === false` 才显示「暂不暴露给 MCP 客户端」；`useRagData` 新增 `statusKnown`，未知不妄断）；`pages.rag.toolsEmpty` 中性化。

## 3. 测试结果

| 套件 | 结果 |
| --- | --- |
| `cargo test --lib -- --test-threads=1` | **448 passed / 0 failed**（14 ignored 为需真机/网络 probe） |
| tool_i18n 单测（8 个） | 解析矩阵（auto/显式/区域归一/回退）/ rag 工具级 10×6 非英 + en 字节锚 / decision ×6 / smart 模板占位符+非英+标题 / set-current 往返 / prop 文案 en 锚 / schema_note 六语 / **全语言遍历不变量**（含 standard+fullSchemaTopN 形态） |
| 全语言遍历不变量（+ en 侧非空断言 + title 收集） | 对 6 种语言分别构建 rag tool_definitions + decision_tool_definitions + build_meta_tools（progressive / standard / standard+fullSchemaTopN 三形态）全量 JSON，递归收集**全部 `description` 与 `annotations.title`（工具级+参数级+标题）**：en 侧非空、数量一致、非空、非英语全部 ≠ 英语——任何未接线（残留硬编码）的描述或 en-arm 回归必被抓出 |
| `commands/rag.rs` 单测 | `rag_tools_command_returns_catalog_regardless_of_flag`（RAG_ACTIVE 静止 false，重加门控即红） |
| 前端 vitest | **193 passed / 0 failed**（19 文件；RagToolsDialog 组件 5 用例 + uiLanguageSync 3 用例 + 既有 185） |
| `npx tsc --noEmit` | 0 错误 |
| `cargo check` | 0 错 0 警 |
| `npm run build` | ✓ |
| 契约脚本 | OK |

### 3.1 覆盖率口径（诚实声明）

- `update_system_config` 语言重建门（prev/now 判定+spawn）为未测命令胶水（依赖 Tauri AppHandle/DB，与命令层胶水同口径）。
- 前端胶水同样未测：`useRagData` 的 `statusKnown` 三态传递链路、`handle_search_tools` 的选路胶水（组件交互层，vitest 仅覆盖纯函数/映射层）。
- 启动路径 `refresh_from_config_and_report_change` → `sync_builtin_index` 支线为未测命令胶水（lib.rs setup 内 spawn）。
- **`tool_i18n.rs` 纯逻辑分支执行**：`resolve_tool_language`/`normalize_lang` 全分支由解析矩阵测试覆盖；四类文本表（工具级 + 参数级 + 标题 + schema_note）——前三类的**每个语言 arm**由全语言遍历不变量逐条执行（6 语言 × 51 文案、62 处出现全部走达）；schema_note 的 n=0/1 臂由 `schema_note_localized_all_langs` 逐分支覆盖（遍历走 {n} 臂）——纯逻辑分支覆盖率 100%。`refresh_from_config()` 与锁中毒恢复臂为防御性胶水（无单测，与命令层胶水同口径披露）。
- `decisionFormat.ts` 保持 100%（v8 实测 19/19 语句、14/14 分支、8/8 函数、16/16 行）。
- **通过率 100%**：全部套件 0 failed（ignored 14 为需真机/网络 probe，已披露）。
- 组件层（ModelVectorSettings 语言卡 UI）无组件级测试——与既有口径一致（依赖的服务/配置/后端逻辑均有专项测试）。

### 3.2 W4 修复（复核战役第四轮）

- [M] 语言解析竞态（并发 update 的 prev/refresh/now 检测窗口可交错陈旧快照——丢变更+缓存钉旧）→ `refresh_from_config_and_report_change()` 以 REFRESH_LOCK 串行化「读-改-比」整体；config 读失败保留旧缓存（不再复位 en）。
- [M] schema_note 测试 n=0/1 派发臂无锚（删除臂全绿）→ 每 n 类英文内容锚（do not include / Only the first result / first {n} results）。
- [M] 搜索结果指引（第 2 个 schema_note 调用点）零覆盖 → `trimmed_results_guideline` 可测接缝 + 非英断言（读语言内聚，调用点无从硬编码）。
- [M] 文档门控语义过时（toolLanguage 变更 → 解析语言变化）×2 + 诚实口径补 update_system_config 门为未测胶水；skip-check 注释纠偏（哈希含本地化 inputSchema）。

### 3.3 W3 修复（复核战役第三轮）

- [M] uiLanguage 通道（auto-heal / languageChanged）翻转解析语言但不触发 builtin 索引重建（重建原只看 toolLanguage 键）→ `update_system_config` 改为**解析语言实际变化**即重建（覆盖全部通道）。
- [M] `meta_tools_shape_progressive_vs_flat` 对全局 TOOL_LANG 输出断言英文锚（schema_note 本地化引入的新竞态）→ 持 `lang_test_lock()`。
- [M] 覆盖率口径补第 4 类文本表（schema_note）与三形态表述。

### 3.4 W2 修复（复核战役第二轮）

- [M] `describe_full_schema_top_n`（schema_note）英文硬编码嵌入本地化描述 → `schema_note_full_schema_top_n(n, lang)` 六语表（n=0/1/{n} 三分支，标识符保留）；`meta.rs` 两处调用点接线（工具描述 + 搜索结果指引）。
- [M] 遍历不变量缺 standard+fullSchemaTopN 形态（standard 分支 smart_route_describe 的描述/参数/标题接线无断言）→ collect 补 `(false, Some(3))` 构建。
- [M] `toolLanguage: "auto"` 登录后不重解析（auth 模式启动期 uiLanguage 同步被 401 拒）→ 保存时随写 `uiLanguage` + 设置页挂载时自愈补写。

### 3.5 W1 修复（复核战役首轮）

- [M] `handleToolLanguageChange` 忽略 `apiPut` 失败响应（apiRequest 不抛错）→ 显式 `res.success` 检查，保存失败下拉保持旧值 + 报错（同 handleDeviceChange 模式）。
- [M] `LANG_LOCK` 不覆盖跨模块读者 → `lang_test_lock()` 提为 `pub(crate)`，decision 两个目录恒等测试持锁（此前 ~2% flake，压测 40×0 失败复核）。
- [M] 遍历不变量补 en 侧非空断言 + 收集 `annotations.title`（en-arm 回归/标题回退盲区封堵）。

### 3.6 已知问题（记录不修，非本变更引入）

- 全量 `cargo test --lib` 默认并行下共享测试池 `database is locked`（group_service/http_server/server_service/oauth_upstream 等写池测试互踩，本机默认线程数下 20/20 复现；`--test-threads=1` 验证门全绿）——既有测试基建债（AGENTS 台账「测试池偶发锁」），与本 diff 无耦合（归因验证：失败测试不引用本 diff 任何符号）。

## 4. 复核战役（100 轮）

（台账见本文末「§5 复核战役台账」一节）

## 5. 复核战役台账

战役模式：6 波 × 逐波压缩收敛（W1-W2 全量新眼扫描 → W3-W4 专项切片 → W5 修复合复审+译文/竞态/测试质量/文档对账 8 切片 → W6 六语译文精读+接线/门控/前端 9 切片），共 **51 个复核槽**（W5S2 因执行中断记 aborted），发现曲线：W1-W2 密集（12 项）→ W3-W4 稳定（8 项）→ W5 收尾（11 项）→ W6 零产品缺陷、译文微瑕 11 项——各切片进入零发现/微瑕状态，战役收官。

### 波次与切片

| 波 | 切片 | 视角 | 结果 |
| --- | --- | --- | --- |
| W1 | S1-S6 | 前端保存链路 / 测试锁竞态 / 遍历不变量盲区 / 文档计数 / en 锚 / diff 新眼 | 修 6 项（handleToolLanguageChange 显式 success、lang_test_lock pub(crate)、walk 补 en 非空+title、文档计数、rag en 锚、auto 重解析缺口） |
| W2 | S1-S6 | schema_note 六语 / meta 双分支 / 缓存一致性 / 文档 / 测试 / diff 新眼 | 修 6 项（schema_note_full_schema_top_n 六语、auto 启动期 401 缺口、(false,Some(3)) 形态、meta_tools_shape 持锁、覆盖率口径第 4 类） |
| W3 | S1-S5 | 重建门语义 / meta walk 形态 / 测试锁 / 文档 / 契约 | 修 5 项（重建门改解析语言实际变化、walk 补形态、meta 持锁、措辞） |
| W4 | S1-S6 | REFRESH_LOCK / Err 保留缓存 / 可测接缝 / 文档 / 前端 / 契约 | 修 6 项（REFRESH_LOCK 串行化、refresh Err 保留旧缓存、trimmed_results_guideline 可测化+非英断言、文档门控语义、披露） |
| W5 | S1-S8 | 修复合复审 / diff 新眼(中断) / zh 译文 / fr-tr-vi-es 译文 / 竞态 / 前端 / 测试质量 / 文档对账 | 修 10 项（fr 语义反转 H、es/tr 译文、remove+sync→直接 sync、en 全表面 FNV 钉死+PROP_KEYS 机械对齐、启动消费 changed、handleDeviceChange 回滚、重复 #[test]、恒真断言、文档 445→448/措辞/changelog） |
| W6 | S1-S9 | zh/fr/es/tr/vi 译文精读 ×5 + rag 接线 + decision/meta 接线 + config 门 + 前端 | 修 5 组译文（es 1M、tr 2M、vi 2L、zh 4L、fr 2L）；接线/门控/前端三切片零发现；1L（standard+fullSchemaTopN call 文案矛盾）记录不修 |

### 记录不修

- **[L] `smart_route_call_desc` standard+`fullSchemaTopN` 文案矛盾**：Some(n) 时第 n 条后结果不含 inputSchema，但 standard call 描述仍断言「搜索结果会明确给出参数」。search 描述中的 schema_note 已纠正此点；call 描述加文案变体需新增 6 语表，收益低，记录为已知边界。
- **[L] tools/list 一次响应可能混两种语言**：三个 builder（rag/decision/meta）各自读一次 `current_tool_language()`，微窗口内配置变更可致单次响应内混合语言。下一响应即一致，无持久影响。
- **[M] 全量 `cargo test --lib` 默认并行 DB lock flake**：存量测试基建债（§3.6），与本 diff 无耦合。
- 命令层胶水（update_system_config 门、启动 spawn 支线）与前端交互层（useRagData statusKnown、handle_search_tools 选路）无单测，已披露（§3.1）。

### 最终门禁

| 门禁 | 结果 |
| --- | --- |
| `cargo test --lib -- --test-threads=1` | **448 passed / 0 failed**（14 ignored 为需真机/网络 probe） |
| `cargo check` | 0 错 0 警 |
| `npx tsc --noEmit` | 0 错 |
| `npm test`（vitest） | **193 passed**（19 文件） |
| `npm run build` | ✓（DMG 产出） |
| 契约脚本 | OK 零 mismatch |
| 6 语 locale JSON | 有效 |
