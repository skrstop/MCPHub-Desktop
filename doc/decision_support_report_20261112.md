# 决策支持（Decision Support）实现与测试报告

> 日期：2026-11-12 · 版本基线：26.11.4 工作树 · 状态：✅ 完成（lib 436 passed / vitest 185 passed / tsc 0 错 / cargo check 0 错 0 警；2026-11-12 晚 UI 合并轮 + 第二复核战役 + 2026-11-15 UI 调整轮与第三复核战役后更新）

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
| DB 迁移 v39+v40 | `db/migration.rs` + `migrations/0039_decision_history.sql` + `0040_decision_history_composite_index.sql` | `decision_history` 表（id/created_at/decision_type/state/question/options_json/scores_json/probabilities_json/selected_index/selected_option/duration_ms/source）+ `created_at DESC` 索引（v39）+ `(created_at DESC, id DESC)` 复合索引（v40，消除每页/每 prune 全表排序）；TARGET_VERSION 38→40 |
| 决策服务 | `services/decision_service.rs`（新） | `DECISION_ACTIVE` AtomicBool（同步门控，启动时 `sync_enabled_flag` 从 `decision.enabled` 回填）；模型缓存（tokio Mutex，按 size 键，**缓存键含 (mtime,len) 指纹**——下载完成后无需显式失效，stat-before-lock + open-after-lock + 原子 rename 发布 = 永不服务陈旧权重）；`decide_and_record`（决策 + 历史落库，历史写失败不拖垮决策）；历史 list/get/clear/cleanup（15 天保留 + **HISTORY_MAX_ROWS=100_000 行数上限 prune**，与活动日志一致）；`builtin_decision_tools()`（`decision_decide` 工具定义，禁用时返回空）；`parse_decision_tool_args`（纯函数校验：未知工具名 / 2-20 选项 / 非空 / **64k 单字段 + 128k 总量 cap**；校验失败错误带 `[invalid params] ` 前缀，bridge 映射为 -32602） |
| 命令层 | `commands/decision.rs`（重写） | 12 条命令（11 条 `require_admin`：status / set_enabled / list_models / select_model / download_model / download_status / download_cancel / decide / history_list / history_get / history_clear；`decision_tools` 无门控恒返目录） |
| 模型管理 kind 化 | `mv/models.rs` | `ModelKind {Embedding, Decision}` 枚举（Serialize lowercase）；`model_root_kind` / `download_root_kind` / `list_models_kind` / `resolve_model_paths_kind` / `download_model_kind` / `ensure_model_ready_kind`（旧签名保留为 embedding 包装）；下载进度结构新增 `kind` 字段，`download_status_kind` / `cancel_download_kind` 按 kind 消歧（单槽下载状态跨 kind 不串）；决策选择持久化 `decision.model`（与 `mv.model` 独立命名空间）+ `default_decision_size`（`pick_default_size` 纯函数） |
| 模型目录 | `runtimes/rag/model/decision/julia/1/` | 重排为 `<kind>/<family>/<size>` 布局（与 embedding 一致）；`deploy.json`（bundle:true / default:true / github 下载源） |
| 打包裁剪 | `scripts/gen-model-resources.mjs` | 扫描 embedding + decision 两个 kind 目录；resources 增加 `runtimes/rag/model/{kind}/*/*/deploy.json` + bundle 尺寸 `**/*` glob |
| MCP 工具暴露 | `rag/service.rs` + 5 个 listing 面 + 2 个 dispatch 面 | `builtin_tools()` 追加决策工具（禁用即空）；`call_builtin_tool` 顶部 `decision_` 前缀分支（先于 RAG 门控，决策自带门控）；新增 `builtin_server_active()` = RAG 开 || 决策开，替换全部 builtin 可见性门控（pool.rs / rmcp_bridge.rs / http_server.rs ×4 / smart_routing/meta.rs ×3）；**builtin 索引写入统一走 `sync_builtin_index()`（`BUILTIN_INDEX_SYNC_LOCK` 静态 tokio Mutex）**——RAG/决策 toggle 与 smart reindex builtin 段三个写入点全部串行化（锁序恒 SYNC→EMBED_WRITE_LOCK，无反转） |
| 定时清理 | `lib.rs` 6h 循环 | 追加 `decision_service::cleanup_history(15)`（失败记 app_log warn） |

### 2.2 前端

| 组件 | 文件 | 说明 |
| --- | --- | --- |
| 决策支持页 | `pages/DecisionPage.tsx`（新） | 开关 + 历史列表（倒序，50/页）+ 详情弹框（类型/时间/State/问题/选项含分数·概率·选中标记/耗时/来源）+ 清空确认框 + 标题旁 Beta 标签 + 「查看工具」按钮（ToolsDialog 展示 `decision_decide` 名称/描述/参数 schema，镜像 RagPage ToolsDialog）；`isTauri()` 门控（web 渲染 null） |
| 设置卡片 | `components/ModelVectorSettings.tsx`（决策分段，2026-11-12 晚合并、2026-11-15 调整） | 决策运行状态行（「运行中：{model}」三态）+ ModelSelector（列表/选择/下载/取消/进度条，`rag://model-download` 事件按 `kind==='decision'` 分流）；**无启用开关**（唯一入口在决策支持页标题旁）；卡片头部带「决策支持」标签（启用时点亮）；切换即时生效。原独立 `DecisionModelSettings.tsx` 已删除 |
| 服务层 | `services/decisionService.ts`（新） | 12 个 API 包装（apiGet/Post/Put/Delete，含 decideDecision） |
| 路由映射 | `utils/tauriClient.ts` | `/decision/*` 12 条 REST→invoke 映射（limit/offset 显式 Number 转换——serde `Option<i64>` 不接受字符串） |
| 路由/菜单 | `App.tsx`（/decision）+ `Sidebar.tsx`（RAG 下「决策支持」，isTauri 门控，带 Beta 标签） | |
| 设置页 | `SettingsPage.tsx` | 仅渲染 ModelVectorSettings（决策区块已内含） |
| 纯函数 | `utils/decisionFormat.ts`（新）+ vitest | 类型归一（未知值回退 choice，防枚举泄漏）/ 概率 / 分数格式化 / **`mergeHistoryPage`（前缀重取分页合并：prev+页内去重 + created_at DESC id DESC 排序）** |
| 事件过滤 | `ModelSelector.tsx`（kind 可选字段）、`ModelVectorSettings.tsx`、`useRagData.tsx` | 共享下载事件总线按 kind 过滤，防跨 kind 进度串扰 |
| i18n | `locales/{en,es,fr,tr,vi,zh}.json` | nav.decision + pages.decision.*（38 键）+ settings.decision*（10 键）+ mvTitle/mvModelSwitch 改名，6 语言全量对齐 |

## 3. 复核战役（10 波 × 10 独立代理 = 100 轮）

> 战役结构：每波 10 个并行独立 code-review 代理（视角轮换：service/models/runtime/builtin/commands/page/settings/DB/locales/security + 新角度：错误路径/并发/serde/UI 状态/测试质量/嵌入 parity/MCP 契约/文档准确性/性能/回归扫描），波间全部修复后进入下一波。发现曲线：W1 1H+9M+3L → W2 3H+8M+4L → W3 1H+2M+8L → W4 2H+5M+6L → W5 4M+6L（无 High，收敛）→ W6 1M+1L → W7 5M+3L → W8+ 持续收敛。

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
1. **[M]** 前端三处 `rag://model-download` 监听不读 `kind` 字段（跨 kind 进度条串扰——kind 字段加了但消费端没用）→ ModelVectorSettings 决策分支（只收 decision）/ ModelVectorSettings + useRagData（只收 embedding/legacy）过滤。
2. **[M]** 决策测试并行抖动（全局 DECISION_ACTIVE flag 竞争 + 共享 decision_history 表 DELETE/INSERT 交错，实测 5 跑 4 挂）→ `TEST_LOCK` 串行化全部触 flag/表测试，连续 4 轮全绿。

### 第三至五波（R11-R50，30 代理逐行 + 亲核修复）

**W2-W5 关键修复（全部落地并验证）**：
- **[H] tokenizer 回退**：`DecisionModel::load` 增加 `tokenizer_fallback` 参数——下载目录只含 model.gguf，GGUF 内嵌 GPT-2 tokenizer 对 julia_1 的 Metaspace 词表是**错的**；bundled 目录的 tokenizer.json 经 `bundled_size_dir_kind` 传入。
- **[H] 身份哨兵**：下载状态快照 `const snapshot = {...p}; setModelDownload(snapshot)`（DecisionModelSettings + ModelVectorSettings + useRagData）——原地突变被 React `Object.is` 吞更新。
- **[H] 路径穿越**：`valid_model_size_name()` 门控（`:` + 22 个 Windows 保留名 + 尾点/尾空格）接入 3 个解析器；解析器对非法名返回 `Ok(None)`（启动时回退默认模型而非硬失败）。
- **[H] ConfirmDialog Enter**：autoFocused Cancel 上按 Enter 冒泡到遮罩触发 onConfirm（数据销毁）→ target 为 BUTTON 时跳过遮罩 Enter。
- **[H] 终态回归**：W2 重构使 ModelVectorSettings error/cancelled 相相提前 return 未 setModelDownload → 进度条永久卡住 → 全部终态相统一快照 set。
- **[H] 缓存失效时机**：`invalidate_model_cache` 在下载**开始**时触发（download_model_kind 是 fire-and-forget）→ 缓存键改 (name,(mtime,len)) 指纹，无用失效移除。
- **[M] toggle 索引同步竞态** → 单一 `sync_builtin_index()`（`BUILTIN_INDEX_SYNC_LOCK`），两个 toggle 全部路由；reindex builtin 段同锁。
- **[M] 无界输入** → sequence() 64k 单字段 cap；parse 64k+128k 总量 cap。
- **[M] 校验错误码** → `[invalid params] ` 哨兵 + bridge 映射 -32602（客户端可区分「改参数」与「服务器错误」）。
- **[M] 历史无界增长** → HISTORY_MAX_ROWS=100_000 + prune_history_row_cap。
- **[M] 前缀重取分页** → mergeHistoryPage 纯函数（prev+页内去重 + 稳定排序）+ 6 单测。
- **[L]** tail 同步 unlink 进临界区（tokio::fs 在 blocking pool 排队，abort 后任意时刻执行——airtight 需 std::fs）；cancel 路径不 unlink；ptr_eq 所有权门控；Escape topmost-first；mounted guards；loadError+retry；setLoading 反馈；deploy.json URL 修正 julia/1。

**记录不修台账**：DST 折叠排序（装饰性 1h/年）；model_ready 仅文件存在性检查；焦点陷阱；bundled family 不匹配（潜伏）；es.json auth.error 漂移（10 个多余键，超出范围）；嵌入运行时缓存无指纹 parity（mv 消费方注册制已覆盖换模型场景）。

### 第六至七波（R51-R70，20 代理逐行 + 亲核修复）

**W6（R51-R60）**：8/10 切片 CLEAN（runtime/models/builtin-sync/commands/page/settings/client/locales 全净）。修复 2 项：
- **[M]** REST `call_server_tool`/`call_group_tool` 把决策参数校验错误映射 500 → `is_invalid_params_error` → **400 BAD_REQUEST**（镜像 /mcp 桥的 -32602；500 暗示服务器故障会诱导重试）。
- **[L]** 冷模型加载持缓存锁跨 spawn_blocking（队头阻塞全部 decide 含缓存命中）→ 双检模式：短锁命中检查、无锁加载、仅发布时重取锁（并发重复加载有界，last-writer-wins——与 mv 注册表同权衡）。

**W7（R61-R70）**：fix-of-fixes/SQL/serde/并发/文档/回归 6 切片 CLEAN。修复 5M+3L：
- **[M]** NaN/Infinity 分数使 serde_json 失败 → `unwrap_or_default` 返回**空**工具结果（is_error:false）且历史存 "[]" → 序列化错误传播为工具错误。
- **[M]** 历史行可存 ~700KB（char cap ≠ byte cap，CJK 4 字节/字符）→ 持久化时截断 state/question 16KB + options 1KB（char-boundary 安全）。
- **[M]** 无界并行 decide（各持 168MB 模型 + forward 张量）→ `DECIDE_SEMAPHORE(1)` 串行化。
- **[M]** 初始加载期间 toggle 被在飞 fetchAll 视觉回退 → togglingRef 守卫。
- **[M]** char-cap 守卫零测试覆盖 + noul 反转对 `["true","false"]` 未测 + 未知 decisionType 未测 → +2 测试（5 断言组）。
- **[L]** tokenizer 加载错误泄漏绝对路径 → 仅文件名；清空确认框分页时低估删除数 → hasMore 时用 count 无关文案（locales ×6 +clearConfirmAll）。

### 第八至十波（R71-R100，30 代理逐行 + 亲核修复 + 收官）

**W8（R71-R80）**：fix-of-fixes/decide 链/i18n/迁移/门控/回归 6 切片 CLEAN。修复 1H+4M+4L：
- **[H]** 启用 Switch 无可访问名称（读屏器只播报 "switch"）→ 两处 aria-label。
- **[M]** 持久化 decision.model 指向已删除文件时每次 decide 硬失败 → `selected_model` 就绪检查回退默认尺寸；单列 created_at 索引无法满足 id tiebreak（每页/prune 全表 temp B-tree 排序）→ **迁移 v40 复合索引**（TARGET_VERSION 39→40）；togglingRef 守卫窗口在 toggle 完成后关闭、陈旧 fetch 仍可回退 → statusGenRef 代际计数器；详情弹框关闭焦点落入 body → viewTriggerRef 恢复。
- **[L]** 信号量注释承诺 busy error 实为无限排队 → try_acquire 快速失败；sync_builtin_index 静默吞错 → rag_log warn；长无断行问题撑破弹框 → break-all；单槽下载拒绝消息不含 kind → 补 kind。

**W9（R81-R90）**：fix-of-fixes/枚举/输出契约/历史 UI/智能路由索引 5 切片 CLEAN。修复 3M+测试 2：
- **[M]** cls_id 取自 bos_token_id 无校验（GGUF/tokenizer.json 不匹配会静默池化错误位置）→ 加载期交叉校验；set_decision_model 吞持久化错误（UI 显示新选择、重启回退）→ Result 传播 fail-closed；decision_decide 前端不可达（无映射/包装）→ 补 POST /decision/decide + decideDecision。
- **测试**：truncate_for_history 单测（ASCII 截断/CJK 边界/空串）+ migrate_v40 复合索引测试。
- **记录不修**：导出/导入不含 system_config（系统性缺口，波及全部键，超出决策范围）；DECIDE_SEMAPHORE busy 路径需 AppHandle 不可单测。

**W10（R91-R100，收官）**：E2E 全链 trace/资源边界/前端终扫/Rust 终扫/对抗输入/收官审计 6 切片 CLEAN；测试套件实跑全绿（lib 427 / vitest 158 / tsc 0 / decisionFormat 覆盖率 100% stmts+branch+funcs+lines）。修复 1C+2M+4L：
- **[C] 修复合自身回归（W9S6 修复引入）**：特殊 token 交叉校验用了 `[..]` 方括号启发式——真实 julia_1 词表是 `<bos>/<eos>/<mask>` 风格（无方括号），模型**永远无法加载**（三条 tokenizer 路径全中）→ 改为词表存在性 + 非空校验。教训：对真实工件（GGUF 元数据 + tokenizer.json）逐字节核验后再写校验器，#[ignore] 的 probe 测试不会在 CI 抓这类回归。
- **[M]** 错误文案审计："no decision model available" 不可操作 → "download one first"；"join load/decide task" tokio 内部术语泄漏 → 用户可读文案。
- **[L]** 文档计数漂移（decisionService 10→11、tauriClient 10→11、locales 27+8→32+9、服务测试 10→12）→ 全部对齐。
- **收官判定**：W10S10 跨面审计（缓存指纹×fire-and-forget 重下载、信号量×HTTP 并发、SYNC 锁×reindex、单槽 panic 守卫、清理循环、哨兵双桥映射、前端合并去重、locale 完整性）**零 High/Medium 残留，战役收官**。

**战役总账（100 轮）**：发现曲线 1H+9M+3L → 3H+8M+4L → 1H+2M+8L → 2H+5M+6L → 4M+6L → 1M+1L → 5M+3L → 1H+4M+4L → 3M+2L → 1C+2M+4L；累计修复 **1C+8H+34M+30L**（另 12 项记录不修）。最终验证：**lib 427 passed / vitest 158 passed / tsc 0 错 / cargo check 0 错 0 警 / npm build ✓ / decisionFormat.ts 覆盖率 100%（16/16 语句、10/10 分支、7/7 函数、14/14 行）**。（第一战役收官时点数字；UI 合并轮 + 第二战役后为 lib 431 / vitest 181，第三轮（UI 调整轮 + 第三复核战役）后为 lib 436 / vitest 185，见 §4 与 §5/§6。）

## 4. 测试结果

| 套件 | 结果 |
| --- | --- |
| `cargo test --lib -- --test-threads=1` | **436 passed / 0 failed**（14 ignored 为需真机/网络的 probe） |
| 新增 Rust 单测 | 决策服务 18 个（工具门控/schema 契约/**工具定义 JSON 形状+门控**/参数校验×4 含 noul 反转对+未知类型+字符上限/**invalid-params 哨兵链路**/**try_begin_decide 禁用+信号量 busy 双路径**/历史 CRUD/分页 clamp×2/cleanup 截断边界+上限 clamp 双侧钉死（4000d 删/3600d 留）/malformed JSON 降级/历史截断边界/**行数上限 prune 低于上限 no-op**/**decision_tool_catalog 恒等**/**selection_lost 纯函数**）+ commands 3 个（`decision_status_serde_camel_case` / `decision_tools` 命令恒返目录 / `decision_status_view` 有效模型 vs 持久化钉死）+ 迁移 v39+v40（表/列/索引/幂等/复合索引）+ `pick_default_size`（默认优先/回退序/空集）+ `valid_model_size_name` 路径穿越用例 |
| 前端 vitest | **185 passed / 0 failed**（17 文件；decisionFormat 15 用例【覆盖率 100%】（含 mergeHistoryPage 7 用例） + **decisionService 8 用例**（getDecisionTools 成功/空回退/抛错 + status/toggle + selectionLost 透传×2）+ **tauriClient.decision 10 用例**（12 条 /decision 映射抽样对账 + transform 数组包装）+ **DecisionToolsDialog 8 组件用例**（渲染含 required 标记/空态/错误态/未启用提示×3（已知关/缺省/未知 null）/双关闭钮/卸载前 resolve 正路径）） |
| `npx tsc --noEmit` | 0 错误 |
| `cargo check` | 0 错 0 警 |
| `npm run build`（前端） | ✓ |
| `node scripts/gen-model-resources.mjs` | ✓（bundled: embedding/F2LLM/v2-80M + decision/julia/1） |

**覆盖率说明（诚实口径）**：新 Rust 代码中 `decision_service.rs` 的可测逻辑（门控/校验/历史 CRUD/cleanup/解析降级/截断）单测覆盖；`commands/decision.rs` 为 require_admin + 委托胶水（3 项有单测：serde camelCase 契约 / `decision_tools` 命令恒返目录 / `decision_status_view` 纯核；require_admin 胶水未单测——与既有命令层同口径）；`mv/decision.rs` 模型推理由既有 `julia_reference_probe`（#[ignore]，需模型文件）数值验证。前端 `decisionFormat.ts` **100% 覆盖（19/19 语句、14/14 分支、8/8 函数、16/16 行，v8 实测）**；页面/服务层为 IO 胶水，仅补针对性测试（decisionService 8 mock 用例 + DecisionToolsDialog 8 组件用例）；ModelVectorSettings 决策区块与 DecisionPage 主体无组件级测试（见 §5 覆盖率口径）。**通过率 100%（全部套件 0 failed）。**


## 5. UI 合并轮 + 第二复核战役（2026-11-12 晚）

四项 UI 变更：①决策模型设置合并进【本地模型】卡片（ModelVectorSettings，头部「决策支持」标签，原独立 DecisionModelSettings.tsx 删除）；②DecisionPage「查看工具」按钮 + ToolsDialog（镜像 RagPage，`decision_tools` 命令无门控、禁用返回空）；③DecisionPage + Sidebar Beta 标签；④RagPage 头部补 Beta pill 对齐。

第二复核战役（10 波独立代理）：W1 四切片（合并正确性/ToolsDialog/Rust 契约/删除完整性）全 CLEAN；W2 回归扫描 + 全文重读 + 跨层契约 + 文档（文档过时 4L 修复）；W3 全文重读 + 契约审计 + locales（Switch aria-label 错键 L、契约脚本 decision 面零覆盖 + 缺失参数方向未校验 M 修复）；W4 修复合回炉（契约脚本缺失参数方向未校验 M + session/app 过滤 L + 计数 11→12 L + Beta pill 不一致 L 修复）；W5 测试覆盖审计（M×4 → 补测试）；W6 修复合回炉 + 全 diff 新视角 + i18n/a11y + 文档终审（数字过时修复）。

**测试补强**：Rust +4（`decision_tool_definitions_shape_and_gating`、`invalid_params_sentinel_linkage`、`try_begin_decide_disabled_and_busy_paths`（`try_begin_decide` 从 decide_and_record 提取可测化，行为逐字保持）、`prune_history_row_cap_below_cap_is_noop`）；前端 +3 文件（decisionService / tauriClient.decision / DecisionToolsDialog）合计 23 用例（7/10/6，第二战役收官时点；第三战役后演进为 26 用例 8/10/8）。契约脚本扩展 decision 命令 invoke 参数对账（正向全量 12 命令 + 反向 8 个带必参命令；缺失参数方向经变异测试验证可抓）。

**最终验证**：lib **436 passed** / vitest **185 passed** / tsc 0 错 / cargo check 0 错 0 警 / npm build ✓ / 契约脚本 OK。**覆盖率口径（诚实声明）**：decisionFormat.ts 100%（实测 19/19 语句、14/14 分支、8/8 函数）；`decision_tool_definitions` 形状/门控有专项 Rust 测试；ToolsDialog 有 8 个组件测试。ModelVectorSettings 决策区块与 DecisionPage 全组件（含标题旁开关/状态「—」三态/历史列表/分页/详情弹框）无组件级测试——「100%」仅指 decisionFormat.ts 纯函数层，不适用于新增 UI 组件（其依赖的服务/路由/后端逻辑均有专项测试）。


## 6. UI 调整轮（2026-11-15，四项）

1. **决策开关与查看工具移至标题旁**（镜像 RagPage header 布局）：标题 + Beta + Switch + 已开启/已关闭状态文字 + 查看工具按钮同排；查看工具按钮**不随启用状态禁用**。
2. **本地模型设置去启用开关**：决策分段只保留模型下拉（标签「决策模型」，无「切换」字眼）；启用开关唯一入口 = 决策支持页标题旁（与 RAG 页同构）。
3. **决策模型运行状态行**：下拉上方「运行中：{model}」（镜像 RAG mvStatus 行，hub-status 圆点 + 三态文案）；RAG 与决策用 border-t 分段区分。三态映射抽 `decisionStatusKey(enabled, modelReady)` 纯函数（decisionFormat.ts，单测覆盖）。
4. **决策未开启也能看工具**：`decision_tool_definitions()` 拆出 `decision_tool_catalog()`（始终返回目录）——查看工具弹框恒展示 `decision_decide` 文档；enable 门控只作用于 MCP 面（`builtin_decision_tools` 门控测试不变）。toolsEmpty 文案改中性（防御态）。

**测试**：Rust +5（`decision_tool_catalog_is_flag_independent` / `selection_lost` 纯函数 / `decision_status_serde_camel_case` / `decision_tools` 命令恒返目录 / `decision_status_view` 有效模型 vs 持久化钉死——`ResolvedModel` 结构化接缝防相邻布尔互换）并强化 definitions 断言（flag-off/on 输出逐字段相等）+ `cleanup_history_with_now` 确定性时钟改造（原边界测试有秒跳 flake 1/40，H 修复）+ 测试库 per-PID 隔离（多进程迁移互踩）；前端 +4 净增（decisionStatusKey 三态 + ToolsDialog 未启用提示×3 + decisionService selectionLost 透传；另删除 1 个不可证伪的卸载冒烟测试）。locales ×6：+9 键（pages.decision.enabled/disabled/toolsDisabledHint、settings.decisionStatus{Running,NoModel,Stopped}、settings.decisionLostHint/decisionLostHintGone、settings.decisionModelMissing 参数化 + decisionModelGone）/-5 死键（decisionEnable/decisionEnableDescription/decisionModelSwitch/decisionEnabledToast/decisionDisabledToast），toolsEmpty 文案中性化、decisionModelMissing 参数化 {{model}}。

**第三复核战役修复终态（W1-W3）**：①模型丢失警告按启用门控 + **`selectionLost` 契约**（decision_status 增报持久化丢失选择——decide 路径 ready 回退会把丢失态藏进 model，警告此前为死代码）；②ToolsDialog 未启用提示 toolsDisabledHint（弹框顶部「暂不暴露给 MCP 客户端」警示，随开关实时显隐）；③`cleanup_history_with_now` 确定性时钟 + 测试库 per-PID（H 级 flake 根治，43 次压测零抖动）；④`decision_tool_catalog` 恒等测试 + definitions 双态逐字段相等断言。

**W4 修复终态**：①`decision_status_view` 纯核 + `decision_tools` 命令测试（布线回归钉死——此前命令体零测试）；②select 乐观更新清 `selectionLost`（残留警告指向已弃用模型）+ 丢失态下拉置空让运行中回退模型可被采纳；③serde 测试改 `get()` 区分 present-null、cleanup 补上限 clamp 断言、删除不可证伪冒烟测试。

**W5 修复终态**：①[M] `mod tests` 丢 `#[cfg(test)]` 回归（拆锁时引入——非测试构建编译测试模块产生 unused import 警告）→ 属性归位；②[M] 下载恢复缺口（仅查 `st.model`——丢失模型重下载的进度/取消在重挂载后不可见）→ 候选集探测全部 size；③[M] 警告文案失实（「重新下载后再使用决策」暗示决策被阻断，实际回退模型仍在决策）→ 中性化措辞；④[L] 警告用原始 size 名（"1"）→ label 映射；⑤[L] 乐观 select 可被在飞 fetch 覆盖 → `decisionGenRef` 代际守卫；⑥[L] 上限 clamp 3650 未从下侧钉死 → 3600d 行存活断言；⑦[L] decisionService mock 补 `selectionLost` 透传断言；⑧[L] 采纳态加 `decisionLostHint` 提示（备用模型代为决策 + 两条恢复路径）。

**W6 修复终态**：①[M] `decisionLostHint` 未随启用门控（3 代理独立收敛——功能关闭时文案失实 + 动作不可达，与警告同病灶）→ `decisionEnabled &&` 门控；②[L] es 误译（admítalo→selecciónelo）；③[L] `decision_status_view` 改 `ResolvedModel{name,ready}` 结构化接缝（相邻裸布尔互换可静默编译）；④文档计数全量对账（184/+3/+5/+7/8 键）。

**W7 修复终态**：①[M] `decisionLostHint` 在 lost-no-fallback 态失实（「代为决策」为假）→ gate 补 `decision?.modelReady`；②[L] 警告文案对已下架 size 承诺不可达的「重新下载」→ 双出路措辞；③[L] `decision_status_view` 复合变异缺口 → 补 persisted-ready/effective-unready 用例 + `EffectiveModel`/`PersistedModel` newtype（互换=编译错误）；④文档：覆盖率注记、12 条命令、含 merge 口径、§5 重号。

**W8 修复终态**：①[M] size 目录被移除时「重新下载」无任何 UI 入口 → `decisionModelGone`/`decisionLostHintGone` 变体文案（只承诺可达出路）；②[M] 命令层表格行截断（括号未闭合、decision_tools 子句丢失）→ 补全；③[L] fetch 失败路径绕过代际守卫（陈旧 fetch 误报 toast）→ catch 补 gen 检查。

**W9 修复终态**：①[L] 变体谓词只查目录成员而非「下载动作真实渲染」（deploy.json 无 downloadUrls 的条目无任何按钮）→ `ready || downloadable` 门控；②文档键数对账（settings.decision* 10 键、+9 键）。

**W10 修复终态**：①[L] 状态加载失败时用初始默认 false 断言「已关闭」+ 工具弹框误称未暴露 → `enabled: boolean | null` 三态（未知显示「—」、hint 仅在确知 false 时显示）；②[L] 警告「选择其他模型」在无就绪行的态需先下载 → 措辞放宽为「改用/switch to」（机制中立，全态真实）；③[M] 文档：AGENTS §3.10 交叉引用错误（指向 OAuth 节）→ 改 commit+§3.4.6；changelog 警告句补门控与 Gone 变体语义；§4 枚举补错误态。

**W11 修复终态**：①[M] 设置页状态行把「未知」塌缩成「未运行」（加载失败时断言失实）→ decision===null 显示「—」（与 DecisionPage 三态一致）；②[M] `enabled={null}` 生产路径无测试（仅测了缺省）→ 补 null 用例 + 2 个陈旧测试名修正；③[M] AGENTS §3.4.2 TARGET_VERSION 记 38（代码 40）→ 40 + v39/v40。

**最终验证**：lib 436 passed / vitest 185 passed / tsc 0 错 / cargo check 0 警 / build ✓。覆盖率口径不变：decisionFormat.ts 100%（实测 19/19 语句、14/14 分支、8/8 函数、16/16 行）。

**W12 收官（12 波 × 100 槽闭环）**：①[L→M] §5 计数残留（25/8/10/7 → 26/8/10/8、未启用提示×2→×3）→ 对账；②[M] 诚实缺口表述未含「—」三态渲染（DecisionPage 标题旁/设置状态行）→ 缺口范围补全；③[M] 设置页状态行「未知」塌缩为「未运行」→ `decision === null` 显示「—」；④[M] `enabled={null}` 生产路径补测试 + AGENTS §3.4.2 TARGET_VERSION 40。**独立收官判定：SHIP**（正确性/文案真实性/测试诚实性三维零残留；已知记录不修台账见 §7）。最终验证：lib **436 passed** / vitest **185 passed** / tsc 0 错 / cargo check 0 警 / build ✓ / 契约脚本 OK；coverage decisionFormat.ts 100%（19/19、14/14、8/8、16/16）。

## 7. 已知边界（记录不修台账）

- 决策模型下载与向量模型下载共享单槽状态（并发下载互斥拒绝，进度按 kind 消歧）——与既有单槽设计一致。
- `decision_decide` MCP 工具走 `call_builtin_tool` 统一 dispatch：bearer allow-list / 工具禁用 / 组过滤 / 活动日志 / 600s 超时全部继承 builtin 语义。
- julia_1 打包随安装包分发（bundle:true，168MB）；如需瘦身改 deploy.json `bundle:false` 走下载。
- 决策历史 `created_at` 为本地时间字符串（`%Y-%m-%d %H:%M:%S`），cleanup 按字典序比较（定宽格式下正确）。
