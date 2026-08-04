# RAG 功能开发计划

> **文档落位**：本计划经用户确认后，作为正式项目文档保存到 `doc/rag_feature_20260730.md`（沿用 `doc/skills_feature_20260727.md` 命名惯例）。本会话处于 plan mode，仅可编辑此计划文件；批准后第一步即落盘到 `doc/`。

## Context

mcphub-desktop 需要内置 RAG（检索增强生成）能力，让本地挂载的 MCP 接口（`/mcp`）能提供 `rag_search`（语义+关键词混合检索文档片段、返回片段与文章 id）与 `rag_get`（按文章 id 取全文）两个工具。

交互形态：左侧菜单新增「RAG」一级菜单（三本书堆叠图标），点击进入 RAG 页面。页面顶部标题右侧有一个**滑块开关**（默认关闭），开关旁有红色信息提示图标，悬浮提示「开启后将占用大量内存，建议在有足够内存的设备上开启」；开启时先检测内存是否充足，不足则提示并阻止。开启/关闭是一个**有状态的资源生命周期管理**：
- 开启 → 启动嵌入式向量模型（ort + tokenizers）、打开 lancedb 向量库连接、向 `/mcp` 挂载 `rag_search`/`rag_get` 两个工具。页面同步等待所有操作就绪（期间显示「开启中」且其他所有功能置灰）。
- 关闭 → 停止模型、释放资源、关闭向量库连接、从 `/mcp` 删除这两个工具。

页面右上角两个按钮：【文档上传】（多选纯文本文件，拷贝到 `rag/files`、分块向量化、写入向量库）、【搜索设置】（调整向量搜索与关键词搜索权重，默认各 0.5，范围 [0,1]）。中部为已上传文档列表，每行有【删除】（删库内文件 + 删向量库记录）、【查看】（弹窗展示全文）。

技术选型（用户指定）：模型推理 = `ort` + `tokenizers`（模型文件已存在于 `src-tauri/runtimes/rag/model`，含 `model.onnx` 175MB + tokenizer）；向量库 = `lancedb`（文件放应用 `rag` 目录）；RAG 配置存现有 sqlite 的 `system_config.config_json`（与向量库互不冲突）。

### 关键既有约定（复用，勿新造）

- 菜单：`frontend/src/components/layout/Sidebar.tsx` 的 `workspaceItems` 数组（lucide-react 图标，`<NavLink>`）。
- 路由：`frontend/src/App.tsx`（`lazy` 导入 + `<Route>`，置于 `<MainLayout/>` 内）。
- 面包屑：`frontend/src/components/layout/Header.tsx` 的 `useCrumbs()`。
- 页面范式：`frontend/src/pages/SkillsPage.tsx`（标题区 `flex items-end justify-between`、`hub-h1`/`hub-sub`、右上 `hub-btn primary`、`hub-card` 列表行、自包含内联 modal `fixed inset-0 bg-black/50`）。
- CSS 工具类：`frontend/src/index.css` 的 `hub-*`（`hub-btn`、`hub-icon-btn`、`hub-card`、`hub-switch on`、`hub-input`、`hub-tag`、`hub-h1`、`hub-sub`、`hub-mono`、`hub-num`）；开关组件 `frontend/src/components/ui/ToggleGroup.tsx` 的 `Switch`。
- i18n：`locales/{en,zh,fr,tr}.json`，`nav.*` / `pages.*`。
- 后端命令范式：`src-tauri/src/commands/*.rs`（`#[tauri::command]` 返回 `Result<T,String>`），注册于 `src-tauri/src/lib.rs` 的 `generate_handler!`。
- 配置读写：`src-tauri/src/services/config_service.rs`（`config_json` blob 的 `get`/`update`，嵌套取值如 `c.get("rag")...`）。
- 应用数据目录：`app.path().app_data_dir()?.join("rag")`（与 `db/mod.rs`、`skill_service.rs` 一致）；运行时模型已在 `src-tauri/runtimes/rag/model`。
- MCP 工具挂载点：`src-tauri/src/services/http_server.rs` 的 `dispatch_mcp`（`match method` 块，参照 `resources/list`、`resources/read` 的自定义方法处理；`tools/list` 聚合点约 803-852 行）。
- 迁移：`src-tauri/src/db/migration.rs`（`TARGET_VERSION`，现 14 → 升 15；`migrate_vN` + match 分支；`.sql` 文件仅兼容占位）。
- 构建注意：cargo 走本地代理 `127.0.0.1:7890`、cargo 不在 PATH（用 rustup toolchain）（见 memory `build-proxy`）。

### 默认决策（用户已答复的澄清点）

1. **Phase 1 范围 = 纯视觉页面，无后端**。所有状态用 React 本地 state 模拟（开关本地翻转、模拟「开启中」延迟、空列表），不接 Tauri 命令、不动 Rust。出页面交用户定稿后再做后端。
2. **RAG 关闭（默认）时，下方文档列表只读、操作禁用**（上传/搜索设置/删除/查看置灰；列表本身可见）。开启中（initializing）整页内容置灰、显示「开启中…」。开启后全部可用。

---

## 阶段划分（先 UI 定稿，再后端）

按用户要求**第一阶段只画页面**，确认无误后再开发后端。

### 阶段一：UI 定稿（仅前端，可运行可点，无后端）

> ✅ **已完成（2026-07-31）**：UI 经用户确认通过。RAG 菜单/Library 图标、RagPage（开关+红色 Info 悬浮提示+上传/搜索设置/向量搜索按钮+文件名模糊搜索+文档列表+上传/向量搜索/搜索设置/查看/删除弹窗）、Sidebar 文档数 badge（skill/rag 同 servers/groups）、4 语言 i18n 全部就绪。`npx tsc --noEmit` 0 新错、`npx vite build` 通过。向量搜索结果 mock 用于预览。

#### 新建文件 `frontend/src/pages/RagPage.tsx`（仿 `SkillsPage.tsx` + `hub-*` 样式）

**头部**：`hub-h1` 标题 + `hub-sub` 副标题；右侧开关组 =
- `<Switch>`（来自 `ToggleGroup.tsx`，`hub-switch`，`checked={enabled}`，`onCheckedChange` 触发本地 toggle）。
- 红色信息提示图标：lucide `Info`（内联 `color: var(--hub-danger)` 或红），Phase 1 用原生 `title` 属性展示提示文案「开启后将占用大量内存，建议在有足够内存的设备上开启」（后续阶段可换为悬浮卡）。
- 开关本地状态：`enabled`（默认 false）、`initializing`（bool）。开 → `initializing=true`，`setTimeout`~1.2s 后 `initializing=false`（纯视觉模拟「同步等待就绪」）。

**右上工具条**（标题下方 `flex items-center gap-2 ml-auto`）：
- 【文档上传】`hub-btn primary` + lucide `Upload`。
- 【搜索设置】`hub-btn` + lucide `SlidersHorizontal`。

**文档列表**（`hub-card overflow-hidden`，仿 Skills 行）：列 = 文件名、大小、上传时间、操作区（【删除】`hub-icon-btn`+`Trash2`、【查看】`hub-icon-btn`+`Eye`）。空状态 `hub-card p-10 text-center` 占位文案。

**弹窗（自包含内联 modal，仿 SkillsPage Dialog 模式）**：
- **上传弹窗**：`<input type="file" multiple accept="...">`（accept 由补全后的 `file_support.json` 生成，Phase 1 可硬编码常用纯文本集 `.txt,.md,.json,.js,.ts,.py,.rs,.sh,...`），提示「仅支持纯文本，不支持 pdf/word/xlsx」。底部「取消 / 确认上传」（确认仅关闭弹窗，不真实处理）。
- **搜索设置弹窗**：两个滑块（向量搜索权重、关键词搜索权重，各 `[0,1]`，默认 `0.5`；原生 `<input type="range">` 套 `hub-*` 样式），实时显示数值；底部「取消 / 保存」。
- **查看弹窗**：`<pre>`/`hub-mono` 可滚动展示文档全文 + 关闭按钮。

**禁用策略**（按默认决策 2）：
- `enabled===false`（关闭，默认）：列表可见只读，【上传】【搜索设置】、每行【删除】【查看】`disabled` 置灰（`opacity-50 pointer-events-none` 或按钮 `disabled` 样式）。
- `initializing===true`（开启中）：除标题与开关外整页置灰，标题区显示「开启中…」+ lucide `Loader2` 旋转。
- `enabled===true`：全部可操作。

#### 修改文件（前端）

- `frontend/src/App.tsx`：`const RagPage = lazy(() => import('./pages/RagPage'));` + `<Route path="/rag" element={<RagPage/>} />`。
- `frontend/src/components/layout/Sidebar.tsx`：`workspaceItems`（resources 与 market 之间）加 `{ path: '/rag', label: t('nav.rag'), icon: <Books className="h-4 w-4" /> }`；从 `lucide-react` 导入 `Books`（三本堆叠书图标；lucide 0.552 已有 `Books`，若实际无则退 `Library`）。
- `frontend/src/components/layout/Header.tsx`：`useCrumbs` 加 `if (path.startsWith('/rag')) return [root, t('nav.rag')];`。
- `locales/{en,zh,fr,tr}.json`：`nav.rag = "RAG"`；`pages.rag.{title,subtitle,upload,searchSettings,memoryWarn,empty,delete,view,save,cancel,opening,vectorWeight,keywordWeight,uploadHint}` 等键。

#### 阶段一验收
- `cd frontend && npx vite build` + `npx tsc --noEmit` 无新增类型错误。
- `npm run tauri dev` 启动后：左侧出现「RAG」菜单（三本书图标），面包屑显示 RAG。
- 开关默认关闭 → 下方操作置灰、列表只读。
- 点开关 → 标题区「开启中…」+ 旋转图标、全内容置灰 → ~1.2s 后变为开启、操作可用。
- 三个弹窗可打开/关闭；搜索设置滑块联动数值。
- **交用户定稿 UI**，按反馈调整；定稿后进入阶段二。

---

### 阶段二：后端核心（开关与模型/向量库生命周期）

拆为子阶段，每步 `cargo check` + 局部联调通过后再进下一步：

- **2.1 基础设施**：`Cargo.toml` 加 `ort`、`tokenizers`、`lancedb`；`migration.rs` 升 `TARGET_VERSION=15` + `migrate_v15`（在 `config_json` 种子 `rag` 默认值：`{enabled:false, vectorWeight:0.5, keywordWeight:0.5, maxResults:20}`）；新建 `src-tauri/src/rag/` 模块（`mod.rs` 声明 + 全局 `OnceLock` 运行时句柄）+ `models/mod.rs`、`services/mod.rs`、`commands/mod.rs` 声明。验证：启动迁移无报错、`config_json.rag` 含默认值。
  > ✅ 完成：deps 加入并编译通过（tokenizers 关闭 esaxx C++ 特性以绕开 macOS CLT C++ 头缺失；ort 默认 `download-binaries`，用户 `tauri dev` 能下载预编译 ORT）。migration v15 落地。rag 模块骨架建好。
- **2.2 embedding**：`rag/embedding.rs`（ort 加载 `runtimes/rag/model/model.onnx` + `tokenizers` 加载 `tokenizer.json`，文本→向量；含内存预检：开启前读系统内存，不足提示并阻止）。
  > ✅ 完成：模型 I/O 名从 ONNX 实测确认（输入 `input_ids`/`attention_mask`、输出 `sentence_embedding`，768 维，图内已含 mean-pool + L2-normalize）。`embed(&mut self, text)`；`check_memory_sufficient()`（跨平台 free 内存探测，阈值 2 GiB）。cargo check 通过。
- **2.3 向量库**：`rag/vectordb.rs`（lancedb 连接 `app_data_dir/rag/lancedb`，建表/插入/查询/删除；`rag/files` 目录管理）。
  > ✅ 完成：表 `rag_chunk`，schema `{id,doc_id,doc_name,chunk_index,chunk_text,embedding(FixedSizeList<f32,768>)}`；`open/add_chunks/search/delete_by_doc/drop_table`。lancedb 0.33 API 全部核对。cargo check 通过。
- **2.4 service + 命令**：`rag/service.rs`（`start()`/`stop()` 同步生命周期 + 内存预检；`upload`/`delete`/`get_doc`/`list_docs`/`search`/`get_settings`/`save_settings`）；`commands/rag.rs` 包一层 `#[tauri::command]`；`lib.rs` 注册。前端 `RagPage` 接通真实命令替换阶段一模拟状态。
  > ✅ 完成:`rag/service.rs`(生命周期 `toggle`/`start`/`stop`/`status`,全局 `tokio::Mutex<Option<Runtime>>`;文档 `list_docs`/`get_doc`/`upload`/`delete_doc`/`open_file_location`,files_dir 存 `<id>`+`<id>.meta`;`search` 嵌入查询→lancedb 近邻→`score=1/(1+distance)`;`get/save_settings` 走 `config_service`)。`models/rag.rs` 7 结构体(camelCase 对齐前端)。`commands/rag.rs` 10 命令,`lib.rs` 已注册。前端 `ragService.ts` 改真实 `apiGet/apiPost/apiPut`,`tauriClient` 加 `/rag/*` 路由映射,`useRagData` 改真实 `ragToggle`/`ragStatus`/`search`(去掉 mock setTimeout)。前端 `tsc` 0 新错、`vite build` 通过;后端 `cargo check`(`ORT_SKIP_DOWNLOAD=1`)0 错 0 警。

### 阶段三：MCP 工具挂载

> ✅ 完成：`dispatch_mcp` 的 `tools/list` 在 `rag::service::is_enabled()` 时追加 `rag_search`/`rag_get`（无 server 前缀,app 级工具）；`tools/call` 在解析 server 前缀前拦截这两个名字 → 路由到 `rag::service::search` / `get_doc`(经 `mcp::progress::get_app_handle()` 取 AppHandle)。结果走 `strategy.shape_tool_call_result` 保持版本兼容。关闭时不返回、不路由。`mcp::progress` 加 `get_app_handle()` 公开 getter。

### 阶段四：文档处理与检索完善

> ✅ 完成：`file_support.json` 补全 60+ 纯文本扩展(markdown/代码/数据/配置);`upload_one` 用 `is_supported_text_file` 按扩展名过滤(拒绝 pdf/word/xlsx);上传分块(~800 字符,120 重叠)→向量化→写 lancedb;删除同步清 files + `db.delete_by_doc`;内存检测 `check_memory_sufficient` 在 `start()` 调用。
> **权重代入实际搜索(用户强调点)**:`service::search` 每次实时读 `rag.vectorWeight`/`rag.keywordWeight`/`maxResults`,向量通道 `db.search`(vec_score=1/(1+distance)) + 关键词通道 `db.keyword_search`(`only_if lower(chunk_text) LIKE`,kw_score=命中词数/总词数),按 `(doc_id,chunk_index)` 合并后 `final=vw*vec + kw*kw`,降序取 maxResults。权重为 0 跳过对应通道。设置页改权重→保存→下次检索即按新权重排序。

---

### 阶段五：标签(tag)系统 — 存储/索引/检索 + UI

> ✅ 完成（2026-07-31）。按 lancedb 三步法实现:
> 1. **存储**:`rag_chunk` 表新增 `tags` 列(`List<Utf8>`),每条 chunk 携带所属文档的标签;`DocMeta` 也存 `tags`(`list_docs` 离线可读)。schema 迁移:旧表无 `tags` 列时 drop+recreate。
> 2. **索引**:`ensure_index` 对 `tags` 建 `Index::LabelList` 标量索引(加速 `array_contains_any`)。
> 3. **查询**:`service::search(query, tags)` 接收可选标签,Rust 侧按标签交集过滤(`eq_ignore_ascii_case`),命中标签交集的 chunk 才计入;`vectorWeight`/`keywordWeight`/`maxResults` 仍实时读配置代入。
>
> **后端**:`models/rag.rs`(`RagFileInput`/`RagDocInfo`/`RagDoc` 加 `tags`);`vectordb.rs`(schema + `ChunkInput.tags` + `ListBuilder` 写入 + `SearchHit.tags` 读取 + `LabelList` 索引);`service.rs`(`upload_one` 存 tags、`reindex_doc` 复用于 `set_doc_tags`、`search(query,tags)` 过滤、`list_docs`/`get_doc` 返回 tags);`commands/rag.rs`(`rag_search_command(query,tags)` + `set_rag_tags(id,tags)`);MCP `rag_search` inputSchema 加可选 `tags`,从 args 解析。
> **前端**:`ragService`(`uploadRagDocs(files,tags)`/`searchRagDocs(query,tags)`/`setRagTags(id,tags)`)+ `tauriClient` 路由(`/rag/search` 传 tags、`/rag/docs/set-tags`→`set_rag_tags`)+ `useRagData`(`upload/search/setTags`);`RagPage`:列表多选 checkbox + 行内标签徽标 + 批量【添加/移除标签】按钮(`BatchTagsDialog`)、上传弹窗 `TagEditor` 指定本批标签、详情弹窗(`ViewDialog`)查看+编辑标签、向量搜索弹窗可选标签过滤。`TagEditor` 复用组件(回车/逗号添加、点 × 删除)。

---

### 阶段六：检索 bug 修复 + UI 微调（2026-07-31）

> ✅ 完成。三个用户反馈:
> 1. **相似度搜索恒返回空(根因)**:`service.rs::chunk_text` 按**字节偏移**切片(`as_bytes()[start..cut]`),再用 `from_utf8(...).unwrap_or("")` 兜底。CJK 文本 3 字节/字,字节边界几乎从不落在字符边界上 -> `from_utf8` 失败 -> chunk 被静默丢弃 -> 中文文档几乎**零 chunk 入库** -> 搜索永远空。**修复**:切片前后用 `str::is_char_boundary` 把 `end`/`start` 对齐到字符边界,保证每个 slice 天然合法 UTF-8;overlap 起点同样对齐,且 `next <= start` 时回退到 `end` 保证前进、不死循环。验证:12000 字节纯中文旧逻辑 0 chunk,新逻辑 18 chunk。(注:旧文档需**重新上传**才会按修复后逻辑重新分块入库。)
> 2. **`extract_tags` 恒返回空**:`vectordb.rs::extract_tags` 收的是 `ListArray::value(i)` 返回的单个 cell(对 `List<Utf8>` 即内层 `StringArray`),却 `downcast_ref::<ListArray>()` -> 必失败 -> 所有 hit 的 `tags` 为空 -> 标签过滤永远命中不了。**修复**:改为 downcast `StringArray` 直接读。同时 `service::search` 两路检索的 `unwrap_or_default()` 改为 `match` + `rag_log("warn", ...)`,失败不再静默吞掉(可在日志页按 server=rag 过滤查看)。
> 3. **标签输入框背景**:`RagPage` 的 `UploadDialog`/`VectorSearchDialog` 中 `TagEditor` 此前裸渲染(无背景,不显眼),与 `ViewDialog`/`BatchTagsDialog` 不一致。统一包一层 `<div className="hub-card" style={{padding:'6px 10px',background:'var(--hub-surface)'}}>`。
> 4. **按钮改名**:「向量搜索」按钮及弹窗标题改为「相似度搜索」(zh;en=Similarity Search;fr=Recherche par similarité;tr=Benzerlik Arama),i18n key `pages.rag.vectorSearch`/`vectorSearchDialogTitle`。
>
> 验证:前端 `tsc --noEmit`(RAG 相关 0 新错)+ `vite build` 通过;后端 `ORT_SKIP_DOWNLOAD=1 cargo check` 0 错(仅 1 个与本改动无关的 `mi_collect` extern 块 doc-comment 警告)。

### 阶段七：删除/改标签的向量清理闭环（2026-07-31）

> ✅ 完成。补删除文档时的 lancedb 数据清理两个缺口:
> 1. **物理空间回收**:`vectordb.rs` 新增 `optimize()`--lancedb 是 append-only 版本化存储,`delete` 只建新版本标记删除,被删向量物理上仍在磁盘(默认保留 7 天旧版本)。`optimize()` 调 `table.optimize(OptimizeAction::Prune { older_than: zero, delete_unverified: true })` 立即清掉除当前版本外的所有旧版本,真正释放被删 embedding 的磁盘空间。`delete_unverified=true` 安全:桌面单进程 + 持 runtime 锁,无并发事务。在 `delete_doc`、`set_doc_tags`(`rewrite_chunks_with_tags`)、`upload_one_path` 覆盖同名旧文档后各调一次。
> 2. **RAG 关闭时的边界保护(直接报错)**:`delete_doc` / `set_doc_tags` 原先 `if let Some(rt)` 在 runtime 为 `None`(RAG 关闭)时**静默跳过** lancedb 操作 -> 留下孤儿向量 / 陈旧标签 chunk。改为:runtime 为 `None` 时直接 `Err("RAG is not enabled - turn on RAG before ...")` 返回,**不删文件 / 不写 meta**,状态保持一致。`delete_doc` 先清 lancedb(`delete_by_doc` + `optimize`)再删文件,失败则文档完好;`set_doc_tags` 把 meta 写入挪到 runtime 检查之后(持锁内),runtime 关闭时 meta 不会被改。UI 在 RAG 关闭时已禁用删除/编辑按钮,此为代码层兜底,防 MCP/批量等其它调用路径漏删/漏改。
>
> 重构:`set_doc_tags` 抽出 `rewrite_chunks_with_tags(db, id, tags)` 辅助函数(读现有 chunks+embeddings -> delete -> 用新 tags 重新 add -> optimize),不重新跑 embedding 模型。
>
> 验证:`ORT_SKIP_DOWNLOAD=1 cargo check` 0 错。

### 阶段九：检索 UI + 设置 + 导入提速（2026-08-01）

> ✅ 完成。五项:
> 1. **相似度评分标签宽度**:`VectorSearchDialog` 结果行的 `相似度: 0.93` 标签加 `flex-shrink-0 whitespace-nowrap`,不再被标题挤压截断。
> 2. **相似度评分阈值**(新设置,默认 0):`RagSettings` 加 `scoreThreshold`(f32,0~1)。`service::search` 在加权打分后 `filter(score >= threshold)`,低于阈值的结果不返回。前端搜索设置弹窗用滑条(`WeightSlider`,0~1 step 0.05)配置。
> 3. **chunk_size**(新设置,默认 512 token):`RagSettings` 加 `chunkSize`(u32)。`reindex_doc` 每次实时读配置,传给 `chunk_text`。
> 4. **chunk_overlap**(新设置,默认 100 token):`RagSettings` 加 `chunkOverlap`(u32),同上。
> 5. **导入提速(根因)**:慢是两个叠加因素--
>    - `reindex_doc` 逐 chunk 调 `model.embed`(44 chunk = 44 次 `session.run`);
>    - `EmbeddingModel::load` 设 `with_intra_threads(1)`,每次推理单线程。
>    修复:`intra_threads` 改 `min(4, available_parallelism)`(4 线程并行,~3-4x);新增 `embed_batch(texts)` 把整批 chunk 填充(padding)到最长序列后**一次** `session.run`(按 16 分批控制峰值内存),`reindex_doc` 改用 `embed_batch`。
>
> **token 感知分块**:`chunk_text` 重写为按 **token** 切分(用 `EmbeddingModel::count_tokens` 量窗口,token 数达 `chunkSize` 即切,在空白处对齐,相邻 chunk 共享 `chunkOverlap` token)。旧版按字节切分对 CJK 过碎、对 ASCII 过粗,且与模型上下文窗口(2048 token)脱节。现在 chunk 大小直接对应模型预算。`count_tokens` 是纯 tokenizer 操作(不跑模型),对几百 KB 文本 <1s。
>
> **导入提速补丁(O(n) 分块)**:`chunk_text` 的 token 计数改为**一次 tokenize 全文**(`tokenize_offsets` 返回每个 token 的字节偏移)+ 按 token 索引切分,O(n)。上一版的 `count_tokens` 对每个字符位置重新编码前缀,是 O(n²) -- 28KB 文档光分块就要 ~7s(还没算 embedding)。新版 `EmbeddingModel::tokenize_offsets` 替换 `count_tokens`,`chunk_text` 按 `offsets[start].0 .. offsets[end].0` 切片(在 token 边界,不切分 token)。实测 README(2593 token)分块 <1ms。

### 阶段十：记录并展示文档实际 chunk 数（2026-08-01）

> ✅ 完成。
> - `DocMeta`/`RagDocInfo`/`RagDoc` 加 `chunk_count: u32`(`#[serde(default)]`,旧 meta 反序列化为 0,兼容)。`reindex_doc` 改返回 `Result<usize>`(chunk 数),`upload_one_path` 在 reindex 后把它写入 `.meta`(调整顺序:先 reindex 拿数,再写 meta)。`list_docs`/`get_doc` 从 meta 填 `chunk_count`。
> - 前端 `RagDocInfo`/`RagDoc` 类型加 `chunkCount`,文档列表新增「分块」列(宽 64,mono,显示 `doc.chunkCount`),表头同步。i18n `columnChunks`(zh=分块/en=Chunks/fr=Blocs/tr=Parça)。
> - 旧文档 chunk 数显示 0,重新上传后显示实际值。
> 验证:前端 tsc 0 rag 错 + vite build 通过 + 4 locale JSON 合法;后端 cargo check 见下。

### 阶段十二：分组「内置：RAG」工具 + 查看工具弹框 + chunk 单位回 token（2026-08-01）

> ✅ 完成。三项:
> 1. **分组「内置：RAG」工具(方案 A,完整 builtinTools 模式)**:仿 builtin_prompts/resources,新增 `groups.builtin_tools` 列(v17 迁移:`ALTER TABLE groups ADD COLUMN builtin_tools TEXT`,NULL=全暴露回退兼容、[]=都不暴露、[...]=白名单)。`Group`/`GroupPayload` 加 `builtin_tools`;`group_service` create/update/row_to_group/SELECT_COLS 读写;`http_server` `scope_builtin_selection` 返回三元组 `(prompts, resources, tools)`,`tools/list` 对 group scope 按 `builtin_tools` 过滤 RAG 工具,`tools/call` 对 RAG 工具校验白名单(不在白名单返回 -32602)。全局/单 server scope 仍全暴露。前端 `Group`/`GroupFormData` 加 `builtinTools`;`useGroupData` + `tauriClient` 传 `builtinTools`;`AddGroupForm`/`EditGroupForm` formData + 传给 `ServerToolConfig`;`ServerToolConfig` 工具 tab 加 `BuiltinSelectionCard`(「内置工具」,RAG 开启且有 RAG 工具时显示),RAG 工具从 `getRagTools()` 拉,`ragStatus()` 探测开关。i18n `groups.builtinTools/allBuiltinTools/builtinToolsSelected`(zh/en/fr/tr)。
> 2. **RAG 开关旁「查看工具」按钮**:弹框显示 RAG 的 3 个 MCP 工具(rag_search/rag_get/rag_tag_search)的 name/description/inputSchema。后端 `tool_definitions()` 单一数据源 + `rag_tools` 命令;`http_server` tools/list 改用 `tool_definitions()`。前端 `ToolsDialog`。
> 3. **chunk 单位改回 token**:`chunk_text` 改回 token 切分(恢复 `tokenize_offsets` O(n)),默认 512/100 token,UI 标注 token,无字↔token 转换。
> 验证:前端 tsc 0 rag/group 新错 + vite build 通过 + 4 locale JSON 合法;后端 cargo check 0 错 0 警。

### 阶段十三：导入提速（CoreML + 全核 + arena + batch64）（2026-08-01）

> 用户反馈:几十 KB 文档 ~10s、10MB 文档 ~10min,不可接受。要求 100KB≤1s / 1MB≤5s / 10MB≤30s。
> **根因**:导入耗时几乎全在 `embed_batch` 的 `session.run`(Gemma3 Q4 模型 CPU 推理)。原配置 `intra_threads=1`(后改 4)、arena 关、batch=16,导致每批开销大、并行度低、张量分配无复用。
> **优化**(`embedding.rs::load` + `embed_batch`):
> 1. **CoreML EP**(macOS):`Cargo.toml` 在 `[target.'cfg(target_os = "macos")']` 下给 ort 加 `coreml` feature(ort-sys 的 coreml 是空 flag,预编译 dylib 已含 CoreML EP,仅解锁 Rust 绑定)。load 时 CoreML EP 优先注册,Apple Silicon 上走 ANE/GPU,不支持的操作自动回落 CPU。非 macOS 仅 CPU。
> 2. **全核**:`intra_threads` 从 `min(4, n)` 改为 `n`(全部逻辑核),8/10/12 核机器此前只用 4 核,浪费严重。
> 3. **arena 开**:CPU EP `with_arena_allocator(true)`,跨多次 batch session.run 复用张量分配(主要吞吐收益)。RSS 在 disable 仍回收:Session drop 释放 arena+模型,`stop()` 调 `mi_collect(true)`。
> 4. **batch64**:`MAX_BATCH` 16→64,session.run 调用数 ~1/4,per-call 开销与 CPU 利用率显著改善。
> 验证:`ORT_SKIP_DOWNLOAD=1 cargo check` 0 错 0 警。
> **说明**:是否达成 100KB≤1s/1MB≤5s/10MB≤30s 取决于 CoreML/ANE 是否真正接管。Q4 量化模型上 CoreML 可能只接管部分算子(其余回落 CPU),若实测仍不达标,下一步可换 `model_fp16.onnx`(CoreML 全精度支持更好)或预编译 CoreML 模型。CPU-only 场景(无 CoreML 加速)难以达到 10MB/30s,需要硬件加速。

### 阶段十四：单文档字符进度条 + 单文件处理日志 + 提速（2026-08-02）

> ✅ 完成。三项用户反馈:
> 1. **单文档字符进度条**(`embedding.rs` + `service.rs` + 前端):上传浮层原来只有「文件级」进度条(`current/total` 文件数)。新增「单文档级」进度条(按**字符**百分比),渲染在文件级条下方。后端在 `reindex_doc` 把分块按 `EMBED_BATCH_SIZE` 分批 embed,每批 `session.run` 完成后 emit `rag://upload-progress` 事件(`{name, charsDone, charsTotal}`,累加每 chunk `chars().count()`,`emit_upload_progress` 把 `charsDone` 钳到 `charsTotal` 防止 overlap 双计导致 >100%)。前端 `useRagData` 挂 `listen('rag://upload-progress')` → `charProgress` state;`RagPage` 上传浮层在 `charProgress.name === uploadProgress.name && charsTotal>0` 时渲染第二条(accent 色)+ 百分比 + `charsDone/charsTotal 字符`。每个文件开始/上传结束重置。
> 2. **单文件处理日志**(`service.rs`):`decode_text` 改返回 `(String, &'static str)`(文本 + 编码名),`upload_one_path_inner` 计时 `read_ms`/`total_ms`,`reindex_doc` 计时 `embedMs`,并在文件处理结束输出一行结构化摘要 `indexed '<name>' done: size=XB chars=X encoding=X chunks=X readMs=X totalMs=X`(配套既有 decode 行的 encoding/bytes/convertMs + index 行的 chunks/embedMs)。日志页按 server=rag 过滤即可逐文件看导入成本。`skill_service` 的 `decode_text` 调用改为 `.0`。
> 3. **提速**(`embedding.rs`):
>    - **并行 tokenize**:`embed_batch` 的逐 chunk `tokenizer.encode` 串行循环改为 `tokenizer.encode_batch`(内部 rayon,`TOKENIZERS_PARALLELISM` 默认开),把 tokenize 摊到全部核——大文档(几千 chunk)的逐 chunk tokenize 占可观墙钟时间。
>    - **batch 64→128**:暴露 `pub const EMBED_BATCH_SIZE: usize = 128`,`reindex_doc` 用同一常量切分进度批次(每批恰好一次 `session.run`,不再二次切分)。`session.run` 调用数减半,CoreML/CPU EP 利用率更好;arena 复用张量,峰值激活内存对典型 chunk(~200 token,chunk_size=512 字)可控。
>    - 预分配 `ids`/`mask`/`out` 的精确容量。
> 验证:后端 `ORT_SKIP_DOWNLOAD=1 cargo check` 0 错;前端 `tsc` rag 相关 0 新错 + `vite build` 通过 + 4 locale JSON 合法。
> **说明**:主导成本仍是 `session.run`(Gemma3 Q4 前向)。并行 tokenize + batch128 主要减少 tokenize 串行开销与 per-call 开销;若仍不达标,根因在 CoreML/ANE 是否真正接管(见阶段十三说明),下一步仍是换 fp16 模型或预编译 CoreML。

### 阶段十五：CoreML 提速尝试 → 失败回退 + 根因结论（2026-08-02）

> 用户实测 `test.md`(28899B/12739 chars/20 chunks) embedMs=7259,`test_1.md`(2019B/2 chunks) embedMs=1030。日志全程 `BFCArena for Cpu`。
> **模型实测**(`config.json` + 文件大小):hidden=768、layers=24、heads=3、max_pos=2048;权重 `model_q4f16.onnx_data`=167MB → ~334M 参数的 Q4(int4 权重 + fp16 激活)句子嵌入模型。
> **尝试 CoreML MLProgram**(15.1):把 `CoreML::default()` 改 `with_model_format(MLProgram) + with_compute_units(All)`,期望接管 transformer matmul 到 GPU/ANE。
> **结果:反而更慢** —— `test_1.md`(2 chunks)从 1030ms 飙到 8781ms(8x 变慢)。
> **根因**:CoreML MLProgram 对**每个不同的输入 batch shape 都重新编译/特化**模型图;每次上传的 chunk 数不同 → 每个文件都付几秒的重新编译。叠加 Q4 量化算子把图切碎、CoreML↔CPU 边界频繁切回(thrash)。→ 对动态 batch 的 Q4 模型,MLProgram 是净负。
> **回退**(15.2):恢复 `CoreML::default()`(NeuralNetwork 默认)。对该 Q4 模型 CoreML 不接管(整图回落 CPU MLAS),即「CoreML 注册等于 no-op,纯 CPU」,这是当前最快的可用配置。
>
> **根因结论(为什么 CPU 这么慢)**:ort 的 CPU EP(MLAS)**不支持原生 int4 GEMM**,Q4 权重在运行时被 dequant 成 fp16 再 upcast 成 fp32 跑 fp32 matmul —— 量化的加速收益全丢,等于一个 fp32 的 334M 模型在 CPU 上的原始速度(~360ms/chunk,batch≈200 token)。这不是代码低效,是 ort MLAS 的 int4 支持缺口 + CoreML 对动态 batch Q4 的不适配。
>
> **真正能提速的路径(都需要换模型文件,非代码层)**:
> 1. **fp16 ONNX 模型**:CoreML MLProgram 对 fp16 transformer 全支持且不再因量化切碎图;固定/缓存 batch shape 后可上 GPU/ANE。预期 ~5-10x。需提供 `model_fp16.onnx` + data,代码层把 `model_dir/model.onnx` 指过去即可。
> 2. **更小的嵌入模型**:如 all-MiniLM-L6-v2(23M/384 维,~90MB fp16)或 bge-small(33M),CPU 上 ~10-15x 快。代价:embedding 维度变 384 → 需重建 lancedb 表 schema(`FixedSizeList<f32,384>`)+ 重新入库;检索质量略降。
> 3. **预编译 CoreML `.mlpackage`**:绕过 ort 的 CoreML EP 适配问题,直接用 CoreML 原生 int4 调色板权重,但需脱离 ort 另起推理路径,工作量大。
>
> 验证:`ORT_SKIP_DOWNLOAD=1 cargo check` 0 错。回退后 `test_1.md` 应回到 ~1s 量级(基线)。提速需求待用户选定上述路径之一。

### 阶段十六：GPU 优先 + 多模型格式支持(q4/q4f16/f16/f32)+ 维度变化重载（2026-08-02）

> 用户要求:(1) 优先用 GPU,无/不支持则退化 CPU,尽可能利用两者;(2) 代码层支持 q4f16/f16/f32/q4 等格式,随时切换模型;维度变化时自动弹框(同导入弹框)重新载入所有文档,维度不变则不动。

#### EP 策略 — 全平台「quantized→CPU;否则 GPU 优先 + CPU 兜底」(`embedding.rs` + `Cargo.toml`)
- **格式探测**:`is_contrib_quantized(model.onnx)` 扫 graph 文件字节,查 `MatMulNBits`/`MatMulFpQ4`/`MatMul4Bits`/`MatMatMul4Bits`(com.microsoft contrib int4 matmul 算子)。
- **contrib 量化模型(q4/q4f16)→ 固定纯 CPU**:CoreML/DirectML/CUDA EP 都不认这些 contrib 算子,重型 matmul 必落 CPU;注册 GPU EP 只会 per-shape 重编译 + 边界 thrash(阶段十五实测 8x 变慢)。所以不注册 GPU,只用 CPU(arena)。
- **其他模型(f16/f32 等所有尺寸)→ 全平台优先 GPU**:`register_gpu_ep(&mut builder)` 按平台注册 GPU EP,**先于** CPU 注册,ort 按注册顺序给每个算子分配到第一个支持的 EP——GPU 优先、不支持的算子自动回落 CPU(最大化两者)。
  - macOS → **CoreML**(MLProgram,All units:ANE > GPU > CPU)。
  - Windows → **DirectML**(DirectX 12;覆盖 NVIDIA/AMD/Intel)。
  - Linux → **CUDA**(NVIDIA;无 N 卡/CUDA 运行时则注册失败→CPU)。
  - GPU EP 通过 per-EP `register()` 注册(失败 warn 后跳过),而非 `with_execution_providers([gpu,cpu])`——后者任一 EP 失败会整体 abort session 创建。CPU EP 最后 append 作兜底。
- **Cargo.toml** 按平台开 ort 特性:macOS `coreml`、Windows `directml`、Linux `cuda`(各 `[target.'cfg(...)']` 块;ort-sys 会下载对应 GPU prebuilt)。非 macOS check 不受 win/linux 块影响。
- Load 时 `log::info` 输出 `embed_dim={} max_context={} contrib_quant={} ep={CoreML+CPU|DirectML+CPU|CUDA+CPU|CPU}`。

#### 动态 embedding 维度(`embedding.rs` + `vectordb.rs` + `service.rs`)
- 移除硬编码 `pub const EMBED_DIM=768`;`EmbeddingModel` 加 `embed_dim: usize` 字段,load 时从 session 输出张量形状读(`session.outputs()[0].dtype().tensor_shape().last()`,Shape: Deref<Target=[i64]>)。`embed()`/`embed_batch()` 行切片用 `self.embed_dim`。
- `VectorDb::open(dir, embed_dim)`:`VectorDb { conn, embed_dim, recreated }`。`chunk_schema(embed_dim)`。`ensure_table` 新增第 3 种重建情形:既有表 `embedding` FixedSizeList 宽 ≠ 模型 embed_dim → drop+recreate;返回 `recreated: bool`。
- `service::start`:`VectorDb::open(dir, model.embed_dim())`;若 `db.recreated()` → `zero_all_chunk_counts`(把所有 `.meta` 的 chunk_count 归零,UI 不显示陈旧数)+ 置 `NEEDS_REINDEX` 静态标志。`RagStatus` 加 `needs_reindex: bool`(`status()` 返回,disable 清零)。

#### 维度变化重载 — 复用导入弹框(`service.rs` + 前端)
- **后端** `reindex_all(app)`:遍历所有 `.meta`,读 `files/<id>` 内容,调 `reindex_doc`(内部已 emit `rag://upload-progress` 字符进度),rewrite meta chunk_count;每文档 emit `rag://reindex-progress` `{current,total,name}`(文件级进度);完成清 `NEEDS_REINDEX`。内容文件不动,只重生 embeddings;tags/title/name 保留。命令 `rag_reindex_all`,注册于 `lib.rs`,路由 `/rag/reindex-all`(tauriClient)。
- **前端** `useRagData`:挂 `rag://reindex-progress` 监听 → 复用 `uploadProgress` state(同导入 overlay);`reindexing` state 切换标题文案(`reindexingFile`/`reindexingDone`)。`toggleEnabled` enable 后若 `st.needsReindex` → 自动 `reindexAll()`,弹框与导入时完全一致(文件级 + 字符级双进度条)。维度不变则 `needsReindex=false`,什么都不做。
- i18n:`reindexingFile`/`reindexingDone`/`reindexFailed`(zh/en/fr/tr)。

#### 模型文件加载(无需改代码)
- 仍 `commit_from_file(model.onnx)`;ort 按 protobuf 内嵌的外部数据文件名解析权重。q4f16(`model_q4f16.onnx_data`)、f16/f32(单文件或各自 data 文件)、q4 等都能直接换文件即用——把新模型的 `model.onnx`(+ data 文件 + `config.json` + `tokenizer.json`)放进 `runtimes/rag/model/`,重启/重 enable 即按新格式选 EP、读新维度。

> 验证:`ORT_SKIP_DOWNLOAD=1 cargo check` 0 错;前端 `tsc` rag 0 新错 + `vite build` 通过 + 4 locale JSON 合法。
> **提速结论**:contrib 量化(q4/q4f16)经 ort 只能 CPU(阶段十五已证);f16/f32 标准算子模型 → CoreML 接管 GPU/ANE,预期 ~5-10x。用户换 fp16 模型即获加速,代码自动适配。


### 阶段十七：多模型尺寸选择 + 下载 + 自动重启（2026-08-02）

> 用户要求:(1) GPU>CPU 优先不变(阶段十六 register_gpu_ep 保留);(2) 模型目录改 `rag/model/<family>/<size>/`,通用文件(tokenizer/config)在 family 目录,尺寸目录放 model.onnx+data 或 download.url;(3) 开关旁加模型下拉框(值 `model_<尺寸>`),download.url 的尺寸用子按钮下载+进度,下载完成前不可选;(4) 重选模型自动重启 RAG。
>
> **目录结构**(用户已落地):`runtimes/rag/model/embeddinggemma/{tokenizer.json,config.json,...}` + `embeddinggemma/q4/{model.onnx,model_q4.onnx_data}`(bundled) + `embeddinggemma/f16/download.url`(下载型)。
>
> **后端**(`service.rs`+`commands/rag.rs`):`model_root`/`download_root`(`<app_data>/rag/models`,可写)/`family_dir`;`list_models` 扫尺寸判定 ready/downloadable 返回 `RagModelInfo{size,label:"model_<size>",status,ready,downloadable}`;`resolve_model_paths(app,size)->(common_dir,size_dir)`;`current_model`/`set_current_model` 持久化 `config_json.rag.model`;`select_model` 校验 ready->持久化->若已开则 stop+start(自动重启);`download_model` 读 download.url -> reqwest 流式下载 .zip(emit `rag://model-download` 进度)-> spawn_blocking 用 `zip` crate 解压到 `<app_data>/rag/models/<family>/<size>/` -> 校验 model.onnx;`EmbeddingModel::load(common_dir,size_dir)`;`start` 读 current_model(无则首个 ready 作默认)。命令 `rag_list_models`/`rag_current_model`/`rag_select_model`/`rag_download_model`,注册 lib.rs,路由 `/rag/models|/rag/model|/rag/select-model|/rag/download-model`。`zip` crate 从 Windows-only 移到通用 `[dependencies]`。
>
> **前端**(`useRagData.tsx`+`RagPage.tsx`):`models`/`currentModel`/`modelDownload` state + `fetchModels` + `rag://model-download` 监听(done/error 刷新列表);`selectModel`(ready 才切,needsReindex 触发 reindexAll 复用导入弹框);`downloadModel`;`ModelSelector` 组件(开关旁):`<select>` ready 尺寸可切 + downloadable 尺寸 disabled 标「下载」,每个 downloadable 一个下载子按钮(Loader2+百分比/提取中)。i18n 8 键 zh/en/fr/tr。
>
> 维度变化重载与模型切换联动:换不同维度模型 -> select_model 重启 -> start 检测 dim 不匹配 -> needs_reindex -> 弹框重新载入所有文档。同维度换模型则不重载(用户既定)。
>
> 验证:`ORT_SKIP_DOWNLOAD=1 cargo check` 0 错;前端 `tsc` rag 0 新错(tauriClient:826 activities 报错为既有)+ `vite build` 通过 + 4 locale JSON 合法。
> **download.url 内容**:**两行 URL**--第 1 行 = model.onnx(graph),第 2 行 = onnx_data(权重数据文件)。`download_model` 直接下载这两个文件到 `<app_data>/rag/models/<family>/<size>/`(model.onnx 存为 `model.onnx`,数据文件存为第 2 行 URL 的 basename,须与 model.onnx 内部引用的外部数据名一致--HF resolve URL 天然满足)。**无 zip 解压**;`zip` crate 仍只 Windows 用(Node 归档解压),非 Windows 不编译。
> **默认模型**:首次无选择时,优先取名为 `quantized` 的 ready 尺寸(用户的 embeddinggemma/quantized),否则取首个 ready。

### 阶段十一：chunk 单位改为「字」+ 模型上下文动态上限（2026-08-01）

> ✅ 完成。
> - **chunk_size / chunk_overlap 单位改为「字」(字符)**:`chunk_text` 从 token 切分改回字符切分(用 `char_indices` 字节偏移,UTF-8 安全,不切多字节字符;O(n),不需要 model 参数)。`RagSettings.chunk_size`/`chunk_overlap` 语义从 token 改为字符,默认仍 512/100。移除 `EmbeddingModel::tokenize_offsets`(不再用)。
> - **模型上下文上限动态获取(不写死)**:`EmbeddingModel` load 时从 `config.json` 的 `max_position_embeddings` 读 `max_context` 存为字段;新增独立 `embedding::read_max_context(model_dir)`(读文件,不依赖 runtime)。`service::model_max_context(app)` 暴露。新命令 `rag_model_limits` 返回 `{ maxContext }`,注册于 `lib.rs`。前端 `useRagData` 挂载时拉取,设置弹窗 `chunk_size` input `max={maxContext}`(动态)、`min=64`;`chunk_overlap` `min=0`、`max=chunkSize-1`。
> - **i18n**:chunkSize/chunkOverlap 标签单位改「字」(zh/en/fr/tr),`chunkSizeHint` 带 `{{max}}` 插值显示当前模型上限。
> - 前端 `RagModelLimits` 类型 + `getRagModelLimits` service + `tauriClient` `/rag/model-limits` 路由。
>
> 说明:字数 vs token 不完全对应(英文 ~2.2 字/token、中文 ~1-2 字/token),用字符数 + 模型 token 上限作 max 是保守的(中文 max 字数 ≈ token 数顶满,默认 512 远低于上限,安全)。
> 验证:前端 tsc 0 rag 新错 + vite build 通过 + 4 locale JSON 合法;后端 cargo check 见下。
>
> **设置持久化**:`get_settings`/`save_settings` 读写 `scoreThreshold`/`chunkSize`/`chunkOverlap` 到 `config_json.rag`。前端 `RagSettings` 类型、`ragService` 默认值、`useRagData` 初始 state 同步。i18n: `scoreThreshold`/`scoreThresholdHint`/`chunkingSection`/`chunkSize`/`chunkSizeHint`/`chunkOverlap`/`chunkOverlapHint`(zh/en/fr/tr)。chunking 设置改后需重新上传文档才生效(已在 hint 注明)。

### 阶段八：tags 列 schema 修复（2026-07-31）

> ✅ 完成。上传报错 `build record batch: Invalid argument error: column types must match schema types, expected List (non-null Utf8) but found List(Utf8) at column index 6`。
> **根因**:`chunk_schema()` 里 `tags` 列的内层 field 定义为 `nullable=false`(non-null Utf8),但 `add_chunks` 用 `ListBuilder::new(StringBuilder::new())` 构建的 `ListArray` 内层 field 是 `nullable=true`。两者不一致 -> `RecordBatch::try_new` 拒绝。此 bug 一直存在,只是被 `chunk_text` 的中文 bug 掩盖(中文产出 0 chunks,`add_chunks` 直接返回没走到 `try_new`);`chunk_text` 修复后才暴露。
> **修复**:`chunk_schema()` 的 `tags` 内层 field 改为 `nullable=true`(与 `ListBuilder` 默认一致)。`ensure_table` schema 迁移增加第二种重建情形:检测到旧表 `tags` 内层 non-null 时 drop+recreate(上传此前一直失败,表实际为空,重建无损失)。`embedding` 列不受影响(它用 `FixedSizeListArray::new` 显式传 non-null field,与 schema 一致)。

---

## 阶段三：MCP 工具挂载（原计划）

- `services/http_server.rs` 的 `dispatch_mcp`：RAG 开启时，`tools/list` 返回追加 `rag_search`、`rag_get`；`tools/call` 对这两个名字路由到 `rag::service`；关闭时不返回、不路由。
- `rag_search(query)` → **混合检索**：同时跑向量检索（lancedb 向量近邻）与关键词检索（BM25/全文匹配），各自归一化得分后按权重加权求和：
  `final_score = vectorWeight * vector_score + keywordWeight * keyword_score`。
  **权重必须实时从配置读取代入实际打分**：每次 `rag_search` 调用时读 `config_service::get()` 的 `rag.vectorWeight`/`rag.keywordWeight`（默认各 0.5，范围 [0,1]），用户在【搜索设置】改完保存后,下一次检索立即生效。返回 Top-K 命中片段 + 文章 id。当某权重为 0 时跳过对应检索通道（纯向量或纯关键词）。
- `rag_get(doc_id)` → 返回全文。

> **要求点（用户强调）**：搜索设置的权重参数**不是仅存配置,必须在实际搜索时代入使用**——`rag_search` 的混合打分公式直接消费这两个权重,设置页改权重 → 保存 → 下次检索即按新权重排序。

### 阶段四：文档处理与检索完善（原计划）

- 上传：拷贝文件到 `app_data_dir/rag/files`，按补全后的 `file_support.json` 过滤纯文本，分块→向量化→写 lancedb。
- 删除：删文件 + 删 lancedb 中对应向量。
- 检索打分：混合检索公式与权重代入见阶段三（`rag_search` 每次实时读 `rag.vectorWeight`/`rag.keywordWeight`）。
- 内存检测落地。

---

## 打包配置（`tauri.conf.json` 的 `bundle.resources`）

最终打包时需把 `runtimes/rag` 和 `runtimes/skill` 一起打包进产物，使应用运行期能读到模型与 skill catalog：

```json
"resources": [
  "runtimes/node",
  "runtimes/uv",
  "runtimes/skill",
  "runtimes/rag"
]
```

> 注：`runtimes/skill/install.json` 当前经 `include_str!` 编译进二进制，本可直接用；但 `runtimes/rag`（含 175MB ONNX 模型与 tokenizer）必须走 `resources` 打包。两者一并加入，保证运行时资源完整。

---

## 补全 `src-tauri/runtimes/rag/file_support.json`（阶段四执行）

补全常用纯文本格式：markdown(`.md`、`.markdown`)、代码(`.js/.ts/.tsx/.jsx/.py/.rs/.go/.java/.c/.cpp/.h/.cs/.rb/.php/.swift/.kt/.scala/.lua/.r/.sql/.yaml/.yml/.toml/.ini/.cfg/.conf/.xml/.html/.css/.scss/.less/.vue/.svelte`)、数据(`.json/.csv/.tsv`)、文档(`.log/.tex/.rst/.org`)。**不含** `.pdf/.docx/.xlsx/.pptx` 等非纯文本。前端 accept 串与此同步。

---

## 端到端验证（最终）

- `cargo build`（带本地代理 `127.0.0.1:7890`、rustup toolchain）通过；`cd frontend && npx vite build` + `npx tsc --noEmit` 无新增错误。
- 开关开启 → 「开启中」→ 就绪；`/mcp` `tools/list` 含 `rag_search`/`rag_get`；上传 `.md/.txt` 后列表出现；`rag_search` 返回片段+文章 id；`rag_get` 返回全文；删除后检索无结果；关闭后 `tools/list` 不含两工具且页面列表只读。
- 不修改 `mcphub-origin/`（核心约束）。
- 更新 `doc/rag_feature_20260730.md` 记录联调迭代（按 AGENTS.md 约束）。

---

## 阶段十八：GGUF 模型支持（candle）+ 策略模式 + download.url JSON + 下拉框重构（2026-08-02）

> 用户要求：ONNX 有弊端（阶段十五结论：ort MLAS 不支持原生 int4 GEMM，Q4 量化运行时 dequant 成 fp32 跑 fp32 matmul，量化加速全丢；CoreML 对动态 batch Q4 不适配），增加 GGUF 支持。GGUF 是 ggml/candle 生态原生量化格式，candle quantized QMatMul 在 CPU/Metal 有原生 int4 kernel，预期真正拿到量化加速。
>
> **已确认的两个关键决策**（plan mode 问询）：
> 1. GGUF 推理 crate = **candle-transformers**（candle-core/nn/transformers，metal/cuda feature，GPU 优先 + CPU 兜底，与 ort 同策略）。
> 2. 架构用**策略模式**（便后续扩展 BERT/Llama 等），当前实现 **Gemma/Gemma3**（decoder，双向 attention + mean-pool + L2-norm，复刻 ONNX 图内逻辑）。

### 1. 策略模式 - Embedder trait（新建 `rag/embedder.rs`）
- `pub trait Embedder: Send`：`embed(&mut self,&str)->Result<Vec<f32>>`、`embed_batch(&mut self,&[&str])->Result<Vec<Vec<f32>>>`、`embed_dim()->usize`、`max_context()->u32`、`tokenize_offsets(&self,&str)->Vec<(usize,usize)>`、`ep_label()->&str`、`backend()->&str`（"onnx"/"gguf"）。ort 专属的 `quantized/quant_markers` 诊断折进 `ep_label()` 单字符串（service 启用日志用 `backend()`+`ep_label()`）。
- 工厂 `load_embedder(size_dir)->Result<Box<dyn Embedder>>`：按文件探测--`model.onnx` 在->`OrtEmbedder`；`model.gguf` 在->`GgufEmbedder`。
- 共享 helper `check_memory_sufficient`/`free_memory_bytes`/`process_rss_mib` 从 `embedding.rs` 挪到 `embedder.rs`（ort/gguf 共用）。
- `rag/mod.rs` 声明 `embedder`/`gguf`/`gguf_gemma` 新模块。

### 2. OrtEmbedder（`rag/embedding.rs` 重构）
- `EmbeddingModel` 重命名 `OrtEmbedder`，impl `Embedder`。`load(common_dir,size_dir)` 改 `load(size_dir)`（tokenizer/config 都在 size_dir，见 §5）。contrib 量化探测、`register_gpu_ep`、arena、`embed_batch`（batch128 + encode_batch）逻辑全部不变，仅包裹进 trait。

### 3. GgufEmbedder（新建 `rag/gguf.rs`）+ Gemma3 架构策略（新建 `rag/gguf_gemma.rs`）
- **设备策略** `pick_device()`：macOS 优先 `Device::new_metal(0)`，Linux 优先 `Device::new_cuda(0)`，失败/无 GPU -> `Device::Cpu`（与 ort `register_gpu_ep` 同逻辑）。`ep_label()` 返回 "Metal+CPU"/"CUDA+CPU"/"CPU"。
- **加载**：`candle_transformers::quantized::gguf::Content` 读 `size_dir/model.gguf`，取张量 + metadata（`embedding_length`->embed_dim、`gemma3.context_length`/`max_position_embeddings`->max_context）。tokenizer 用 `size_dir/tokenizer.json`（tokenizers crate，与 ort 共用，GGUF 内嵌 tokenizer 冗余但不依赖）。
- **架构策略** `trait GgufArch: Send`：`forward_embed(&mut self,input_ids:&[u32],attention_mask:&[u32])->Result<Tensor>`（[seq,hidden]）、`hidden_dim()->usize`。按 GGUF metadata `architecture` 分发；当前实现 `gemma3`/`gemma2` -> `Gemma3EmbedArch`。
- **Gemma3EmbedArch**：基于 candle_transformers 的 quantized Gemma 改造前向--**bidirectional attention**（去 causal mask，用 attention_mask 屏蔽 padding；保留 sliding_window 分层模式，config `layer_types`/`sliding_window=512`）+ RMSNorm + RoPE。终层 hidden -> **mean-pool**（按 attention_mask 加权求和/求和）-> **L2-normalize**（复刻 ONNX 图内 mean-pool+L2-norm）。`embed_dim=hidden_size`（768）。
- `embed`/`embed_batch`：tokenize->pad->forward->pool+norm->切行。batch 同 ort 思路（pad 到最长，一次 forward；超 `EMBED_BATCH_SIZE` 分批）。
- **本阶段最大工作量/风险点**：candle_transformers 的 quantized Gemma 是 causal 生成用；embedding 变体的双向 attention + sliding + pooling 需自定义前向。

### 4. Cargo.toml
- 加 `candle-core`/`candle-nn`/`candle-transformers`（同版本，0.8 系列，按编译结果微调）。
- macOS `[target.cfg(target_os="macos")]`：candle-core/nn 加 `metal` feature。
- Linux `[target.cfg(all(unix,not(target_os="macos")))]`：加 `cuda` feature。
- Windows：CPU only（candle 无 DirectML，后续可加）。
- ort/lancedb/tokenizers 不变。

### 5. 目录结构（需求 3）-- ONNX 文件进尺寸目录，取消 family 通用目录
- `embeddinggemma/{tokenizer.json,config.json,special_tokens_map.json,tokenizer_config.json}` 拷进每个 ONNX 尺寸目录（`q4/`、`quantized/`、`f16/`），family 目录不再放通用文件（因其他尺寸可能是 GGUF）。
- 每个尺寸目录自包含：ONNX 尺寸=`model.onnx`+`*_onnx_data`+`tokenizer.json`+`config.json`+...；GGUF 尺寸=`model.gguf`+`tokenizer.json`+`config.json`。
- 代码：删 `family_dir`；`resolve_model_paths(app,size)` 返回 `Option<PathBuf>`（仅 size_dir，downloaded 优先于 bundled）；`Embedder::load(size_dir)`；`model_max_context` 改为解析 current/default size_dir 读 `config.json`（GGUF 可从 metadata 读）。

### 6. download.url 改 JSON（需求 4）
- 格式：onnx `{"type":"onnx","modelUrl":[".../model.onnx",".../model_data.onnx"]}`；gguf `{"type":"gguf","modelUrl":[".../model.gguf"]}`。
- `download_model`：读 download.url 作 JSON，取 `type`+`modelUrl` 数组，逐文件流式下载到 size_dir：onnx 第一个 URL 存 `model.onnx`（graph），其余存 URL basename（须与 protobuf 内嵌外部数据名一致，HF resolve URL 天然满足）；gguf 单文件存 `model.gguf`。
- `list_models` 探测：`model.onnx` 在=onnx ready；`model.gguf` 在=gguf ready；仅 download.url 在=downloadable（`type` 字段决定将下载的格式，也用于 UI badge）。

### 7. 默认模型 = embeddinggemma/default（需求 5）
- `start()` 无 current_model 时：优先 `size=="default" && ready`，否则首个 ready（当前是 "quantized"，改 "default"）。
- 新建 `embeddinggemma/default/`：放 `tokenizer.json`+`config.json`+`download.url`（gguf JSON）--default 作 GGUF 默认。用户下载/放入 `model.gguf` 后即 ready（沙箱无法代为下载 GGUF 文件，由用户提供）。
- 既有 q4/quantized（onnx bundled）保留作 fallback。

### 8. RagModelInfo 扩展（`models/rag.rs`）
- 加 `format:String`（"onnx"/"gguf"）、`file_size:u64`（ready=模型文件 on-disk 字节数和；downloadable=0/未知）。

### 9. 下载进度扩展（`service.rs::RagModelDownloadProgress`）
- 加 `speed:u64`（B/s）、`eta:u64`（剩余秒）、`file_current:u32`、`file_total:u32`。phase 去 "extracting"（无 zip 了，保留 "downloading"/"done"/"error"）。
- `download_model` 滑动窗口算 speed、`eta=(total-downloaded)/speed`、逐文件计数 file_current/file_total，累计 downloaded/total（各文件 content-length 之和）。

### 10. 前端下拉框重构（需求 6）-- `RagPage.tsx::ModelSelector`
- 弃用 `<select>`+外部下载按钮，改自定义 popover 下拉：触发按钮（当前模型 label+chevron）+ 面板（每项一行复合 div）。
- 每行：模型名 + 文件大小（格式化）+ 格式 badge（ONNX/GGUF）+（ready：点击选中，当前项打勾；downloadable：下载按钮+进度条）。
- 下载进度条信息：总进度% + 速度（MB/s）+ 剩余时间 + 文件数 `fileCurrent/fileTotal`。
- 保留需求 7 行为：RAG 关闭时下拉可选（仅 initializing 时禁用）；选中即持久化（关时）/自动重启（开时）--后端 `select_model` 已具备，UI 不额外禁用。`disabled` 仍传 `initializing`（不传页面级 `disabled`）。

### 11. 前端类型/状态/i18n
- `types/index.ts`：`RagModelInfo` 加 `format`/`fileSize`；`modelDownload` 加 `speed`/`eta`/`fileCurrent`/`fileTotal`。
- `useRagData.tsx`：`modelDownload` state 扩展字段透传（监听 `rag://model-download` 已有，payload 加新字段）。
- `locales/{en,zh,fr,tr}.json`：新增键（文件大小、速度、剩余时间、文件数、ONNX/GGUF badge、下载/下载中/下载完成 等）。

### 验证
- 后端：`ORT_SKIP_DOWNLOAD=1 cargo check`（asdf cargo 1.96 + 代理 `127.0.0.1:7890`）0 错。candle 无下载步骤；metal feature 在 macOS 编译 Metal shader（本机有 Metal framework）。
- 前端：`tsc --noEmit` rag 0 新错 + `vite build` 通过 + 4 locale JSON 合法。
- 真实构建/联调由用户跑（tauri dev/build）。GGUF Gemma3 前向为高风险点，可能需按实测调双向 attention/sliding/pooling。

### 风险/说明
- **Gemma3 量化双向前向**是本阶段主风险：candle_transformers 的 quantized Gemma 是 causal 生成用，需改双向 + sliding + mean-pool + L2-norm。若 candle 版本不支持 Gemma3 量化（仅 Gemma2），需用 Gemma2 GGUF 或自实现 Gemma3 量化层。
- **candle 版本/API** 需按实际 crates.io 核对（沙箱网络受限，无法实时查；用 0.8 系列，按编译结果微调）。
- **GGUF 模型文件**由用户提供（沙箱无法下载）；default/ 初始为 downloadable，用户下载后 ready。
- ort 路径完全保留（阶段十六 EP 策略不变），仅包裹进 OrtEmbedder；现有 onnx 尺寸（q4/quantized）继续可用。

#### 落地状态（2026-08-02）
> ✅ 完成（编译通过）。后端 `ORT_SKIP_DOWNLOAD=1 cargo check` 0 错 0 警（asdf cargo 1.96 + 代理 + candle 0.11.0 metal feature）。前端 `tsc --noEmit` RAG 0 新错 + `vite build` 通过 + 4 locale JSON 合法。
> - **crate 升级**：candle-core/nn/transformers 0.8 → 0.11.0（用户要求）。0.11 的 lib 名由 `candle` 改为 `candle_core`（`candle_nn`/`candle_transformers` 同），代码用 `candle_core::`/`candle_nn::`。
> - **关键 API 校准**（读 candle-core-0.11.0 源码核对）：`quantized` 在 `candle_core::quantized`（非 candle_transformers）；`gguf_file::Content::read(&mut R)` + `Content::tensor(&mut R, name, device)` + `Value::to_u32()/to_f32()`（`Result`）；`QMatMul::from_qtensor(QTensor)` + `Module::forward`；`candle_nn::RmsNorm::new(weight, eps)`；`candle_nn::ops::softmax_last_dim`；`Tensor::{embedding, arange, from_vec, narrow, reshape, dims2/3/4, broadcast_mul/sub/div/add, gelu, sqr, sqrt, tanh, abs, cos, sin, cat, where_cond, le(scalar), affine(f64,f64), repeat, transpose, contiguous, flatten_all, to_vec1}` 全部核对存在。
> - **目录**：`embeddinggemma/default/{Q4_0.gguf, tokenizer.json, config.json, ...}`（用户已放 Q4_0.gguf 277MB，我补了 tokenizer/config）；`quantized/` 已自包含（model.onnx + tokenizer + config）。`detect_format`/`find_gguf` 用 `*.gguf` 通配，兼容 Q4_0.gguf 命名。
> - **风险残留**：GgufEmbedder/Gemma3 双向前向已编译通过，但未用真实 GGUF 跑出 embedding 验证 cosine。Gemma3 的 sliding-window 分层模式（`i%6==5` full）、q_norm/k_norm、bidirectional attention、mean-pool+L2-norm 是否与 ONNX 参考一致需实测；若 embedding 质量异常优先核对这些。`.gelu()` 用 candle 默认（erf 还是 tanh 待定，Gemma3 用 `gelu_pytorch_tanh`，必要时改 `gelu_erf` 或手写）。
> - **`Embedder: Send + Sync`** + `GgufArch: Send + Sync`（Box<dyn Trait> 跨 await 必须 Sync，否则 future not Send）。candle_core::Error 经 `?` 转 anyhow（thiserror 派生 `std::error::Error`，From 自动生效）——仅末尾返回 candle Result 的 helper 用 `Ok(...?)` 包。

### 构建/编译约定（治本：避免每次 tauri dev 全量重编）

> candle + ort + lancedb 是巨型依赖，首次全量编译十几分钟。**Cargo 本身就缓存**（`src-tauri/target/debug/deps/`，约 70GB），但缓存是否复用取决于**构建环境指纹是否稳定**。下面三条是硬约定，违反任一都会让 ort-sys/candle 重编一份。

1. **`tauri dev` 绝不带 `ORT_SKIP_DOWNLOAD=1`** —— 那是无网沙箱（CI/受限环境）专用，会让 ort-sys 走"跳过下载"分支。真实开发机能正常下载 ort，带它反而让 build-script 环境与正常构建不一致 → ort-sys 重编一份。沙箱验证用 `ORT_SKIP_DOWNLOAD=1 cargo check`，**真实 `tauri dev`/`tauri build` 永远不带**。
2. **不反复切换 env**（代理 `HTTPS_PROXY`、`RUSTFLAGS`、`CARGO_*`）—— env 变化会改变 build-script 的 `rerun-if-env-changed` 指纹，连带 ort-sys/candle 及其下游全部重编。固定一套 env，ort-sys/candle 只编一份、稳定缓存。
3. **统一工具链**：cargo 走 asdf shim（`PATH` 含 `~/.asdf/shims`，cargo 1.96.0），不混用 rustup/系统 cargo。

#### dev profile 优化（`Cargo.toml`，2026-08-03）
默认 dev profile 是 `debug=2`（完整符号），70GB 体积里绝大多数是调试符号，**链接 mcphub 二进制极慢**（哪怕只改一行也要重链 70GB 符号）。已加：
```toml
[profile.dev]
debug = 1                      # 行号可用、去变量符号，减体积
split-debuginfo = "unpacked"  # macOS 调试符号外置 .dSYM，二进制小链接快
[profile.dev.package."*"]     # 依赖单独提速
opt-level = 1
debug = 1
```
- **一次性代价**：profile 变 → 所有依赖按新参数重编一次（~15min），之后增量构建明显变快（链接不再扫 70GB 符号）。
- **不影响 release**：release profile 独立，updater 符号等约束不变。
- 排查"每次都编译"时，先看 `ls src-tauri/target/debug/build/ | grep ort-sys` 是否有多个 hash 目录——有即 env 漂移证据，`rm -rf src-tauri/target/debug/build/ort-sys-*` 清掉、用固定 env 重跑一次即可。

#### 可选进一步增强
- `brew install sccache` + `.cargo/config.toml` 加 `[build] rustc-wrapper = "sccache"`：防 `cargo clean`/CI 后重来。
- macOS 装 `sold`（mold 的 mac 分支）做链接器，进一步加速。
