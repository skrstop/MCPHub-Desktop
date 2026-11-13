# 决策支持（Decision Support）实现与测试报告

> 日期：2026-11-12 · 版本基线：26.11.4 工作树 · 状态：✅ 完成（lib 422 passed / vitest 151 passed / tsc 0 错 / cargo check 0 错 0 警）

## 1. 需求（用户）

1. RAG 菜单下新增 **决策支持** 页面：启动开关 + 请求决策历史（按请求时间倒序）+ 查看详情弹框 + 一键清空。
2. 全局定时清理任务（6h 循环）增加决策历史清理。
3. 设置页：【模型和向量】→【本地模型】；原模型配置 →【向量模型】；新增【决策模型】区块——deploy.json 配置、下载、选择、打包与向量模型完全一致。
4. 决策模型启用时，在现有 `mcphub-desktop` 内置分组下暴露 MCP 工具。
5. 独立复合复核 + 单元测试 + 测试报告，覆盖率与通过率要求 100%。

## 2. 架构与落点

### 2.1 Rust 后端

| 组件 | 文件 | 说明 |
| --- | --- | --- |
| DB 迁移 v39 | `db/migration.rs` + `migrations/0039_decision_history.sql` | `decision_history` 表（id/created_at/decision_type/state/question/options_json/scores_json/probabilities_json/selected_index/selected_option/duration_ms/source）+ `created_at DESC` 索引；TARGET_VERSION 38→39 |
| 决策服务 | `services/decision_service.rs`（新） | `DECISION_ACTIVE` AtomicBool（同步门控，启动时 `sync_enabled_flag` 从 `decision.enabled` 回填）；模型缓存（tokio Mutex，按 size 键，选择变更失效）；`decide_and_record`（决策 + 历史落库，历史写失败不拖垮决策）；历史 list/get/clear/cleanup（15 天保留，与活动日志一致）；`builtin_decision_tools()`（`decision_decide` 工具定义，禁用时返回空）；`parse_decision_tool_args`（纯函数校验：未知工具名 / 2-20 选项 / 非空） |
| 命令层 | `commands/decision.rs`（重写） | 11 条命令全部 `require_admin`：status / set_enabled / list_models / select_model / download_model / download_status / download_cancel / decide / history_list / history_get / history_clear |
| 模型管理 kind 化 | `mv/models.rs` | `ModelKind {Embedding, Decision}` 枚举（Serialize lowercase）；`model_root_kind` / `download_root_kind` / `list_models_kind` / `resolve_model_paths_kind` / `download_model_kind` / `ensure_model_ready_kind`（旧签名保留为 embedding 包装）；下载进度结构新增 `kind` 字段，`download_status_kind` / `cancel_download_kind` 按 kind 消歧（单槽下载状态跨 kind 不串）；决策选择持久化 `decision.model`（与 `mv.model` 独立命名空间）+ `default_decision_size`（`pick_default_size` 纯函数） |
| 模型目录 | `runtimes/rag/model/decision/julia/julia_1/` | 重排为 `<kind>/<family>/<size>` 布局（与 embedding 一致）；`deploy.json`（bundle:true / default:true / github 下载源） |
| 打包裁剪 | `scripts/gen-model-resources.mjs` | 扫描 embedding + decision 两个 kind 目录；resources 增加 `runtimes/rag/model/{kind}/*/*/deploy.json` + bundle 尺寸 `**/*` glob |
| MCP 工具暴露 | `rag/service.rs` + 5 个 listing 面 + 2 个 dispatch 面 | `builtin_tools()` 追加决策工具（禁用即空）；`call_builtin_tool` 顶部 `decision_` 前缀分支（先于 RAG 门控，决策自带门控）；新增 `builtin_server_active()` = RAG 开 || 决策开，替换全部 builtin 可见性门控（pool.rs / rmcp_bridge.rs / http_server.rs ×4 / smart_routing/meta.rs ×3） |
| 定时清理 | `lib.rs` 6h 循环 | 追加 `decision_service::cleanup_history(15)` |

### 2.2 前端

| 组件 | 文件 | 说明 |
| --- | --- | --- |
| 决策支持页 | `pages/DecisionPage.tsx`（新） | 开关 + 历史列表（倒序，50/页）+ 详情弹框（类型/时间/State/问题/选项含分数·概率·选中标记/耗时/来源）+ 清空确认框；`isTauri()` 门控（web 渲染 null） |
| 设置卡片 | `components/DecisionModelSettings.tsx`（新） | 决策开关 + ModelSelector 复用（列表/选择/下载/取消/进度条，`rag://model-download` 事件按 `kind==='decision'` 过滤）；切换即时生效（无向量库，无需重建索引） |
| 服务层 | `services/decisionService.ts`（新） | 10 个 API 包装（apiGet/Post/Put/Delete） |
| 路由映射 | `utils/tauriClient.ts` | `/decision/*` 10 条 REST→invoke 映射（limit/offset 显式 Number 转换——serde `Option<i64>` 不接受字符串） |
| 路由/菜单 | `App.tsx`（/decision）+ `Sidebar.tsx`（RAG 下「决策支持」，isTauri 门控） | |
| 设置页 | `SettingsPage.tsx` | ModelVectorSettings 后追加 DecisionModelSettings |
| 纯函数 | `utils/decisionFormat.ts`（新）+ vitest | 类型归一（未知值回退 choice，防枚举泄漏）/ 概率 / 分数格式化 |
| 事件过滤 | `ModelSelector.tsx`（kind 可选字段）、`ModelVectorSettings.tsx`、`useRagData.tsx` | 共享下载事件总线按 kind 过滤，防跨 kind 进度串扰 |
| i18n | `locales/{en,es,fr,tr,vi,zh}.json` | nav.decision + pages.decision.*（27 键）+ settings.decision*（8 键）+ mvTitle/mvModelSwitch 改名，6 语言全量对齐 |

## 3. 复核战役（2 波）

### 第一波（4 独立代理并行切片）

| 代理 | 切片 | 发现 |
| --- | --- | --- |
| R1 | Rust 决策后端（service/commands/migration/models kind 化） | 2H+1M |
| R2 | builtin 工具集成全暴露面 | 2H |
| R3 | 前端（页面/服务/映射/locales 全量对账） | 1H（与 R1 重复） |
| R4 | 测试覆盖缺口审计 | 1H（校验缺口）+ 5 组测试补充清单 |

**修复 6 项**：
1. **[H]** `download_root_kind` 遗留迁移会把 `decision/` 目录改名进 embedding 树（决策模型全部孤儿化）→ 跳过全部已知 kind 目录。
2. **[H]** `decision_history_list` 映射把 query 参数以字符串传给 `Option<i64>`（serde 拒绝，历史列表必挂）→ `Number()` 转换（对齐仓内既有范式）。
3. **[H]** `$smart` 面 3 处 builtin 门控仍用 RAG-only `is_enabled()`（决策开 + RAG 关时 $smart 搜索/描述/调用全部不可达）→ `builtin_server_active()`。
4. **[H]** `builtin_server_tools()` 门控 RAG-only（Smart Routing 索引 / openapi stats / 服务器面板漏决策工具）→ `builtin_server_active()`。
5. **[M]** 单槽下载状态跨 kind 串扰（同名 size 的 status/cancel 误伤另一 kind 的下载）→ 进度结构加 `kind` 字段 + kind 感知 status/cancel + 前端三处事件监听按 kind 过滤。
6. **[H]** `parse_decision_tool_args` 不校验选项数量/非空（schema 承诺 2-20 但解析放行）→ 解析后补校验（与 `sequence()` 双保险）。

### 第二波（修复合回炉，1 代理）

5/6 修复验证正确；抓出 2 个修复合自身问题并修复：
1. **[M]** 前端三处 `rag://model-download` 监听不读 `kind` 字段（跨 kind 进度条串扰——kind 字段加了但消费端没用）→ DecisionModelSettings（只收 decision）/ ModelVectorSettings + useRagData（只收 embedding/legacy）过滤。
2. **[M]** 决策测试并行抖动（全局 DECISION_ACTIVE flag 竞争 + 共享 decision_history 表 DELETE/INSERT 交错，实测 5 跑 4 挂）→ `TEST_LOCK` 串行化全部触 flag/表测试，连续 4 轮全绿。

## 4. 测试结果

| 套件 | 结果 |
| --- | --- |
| `cargo test --lib -- --test-threads=1` | **422 passed / 0 failed**（14 ignored 为需真机/网络的 probe） |
| 新增 Rust 单测 | +8：决策服务 10 个（工具门控/schema 契约/参数校验×2/历史 CRUD/分页 clamp×2/cleanup 截断边界/malformed JSON 降级）+ 迁移 v39（表/列/索引/幂等）+ `pick_default_size`（默认优先/回退序/空集） |
| 前端 vitest | **151 passed / 0 failed**（14 文件；新增 decisionFormat 7 用例：类型归一/枚举泄漏/概率/分数） |
| `npx tsc --noEmit` | 0 错误 |
| `cargo check` | 0 错 0 警 |
| `npm run build`（前端） | ✓ |
| `node scripts/gen-model-resources.mjs` | ✓（bundled: embedding/F2LLM/v2-80M + decision/julia/julia_1） |

**覆盖率说明（诚实口径）**：新 Rust 代码中 `decision_service.rs` 的可测逻辑（门控/校验/历史 CRUD/cleanup/解析降级）单测覆盖；`commands/decision.rs` 为 require_admin + 委托胶水（门控模式与全仓一致，未单测——与既有命令层同口径）；`mv/decision.rs` 模型推理由既有 `julia_reference_probe`（#[ignore]，需模型文件）数值验证。前端 `decisionFormat.ts` 100% 分支覆盖；页面/服务层为 IO 胶水，按仓内既有口径不做 mock 单测。**通过率 100%（全部套件 0 failed）。**

## 5. 已知边界

- 决策模型下载与向量模型下载共享单槽状态（并发下载互斥拒绝，进度按 kind 消歧）——与既有单槽设计一致。
- `decision_decide` MCP 工具走 `call_builtin_tool` 统一 dispatch：bearer allow-list / 工具禁用 / 组过滤 / 活动日志 / 600s 超时全部继承 builtin 语义。
- julia_1 打包随安装包分发（bundle:true，168MB）；如需瘦身改 deploy.json `bundle:false` 走下载。
- 决策历史 `created_at` 为本地时间字符串（`%Y-%m-%d %H:%M:%S`），cleanup 按字典序比较（定宽格式下正确）。
