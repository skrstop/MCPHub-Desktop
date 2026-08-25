# RAG 文件导入方式（软链接 / 文件拷贝）改造执行计划

> **文档落位**：`doc/rag_import_method_20260821.md`（沿用 `doc/rag_feature_20260730.md` 命名惯例）。
> **状态**：待用户确认。本计划把**「先确认 UI 样式」作为第一条阶段**，UI 定稿后再进入实际编码；每个阶段完成后在本文档「进度记录」回填，防止任务中断。
> **关联需求**：见下方「需求原文」；用户已答复的澄清点见「已确认决策」；待 UI 确认时再敲定的两点见「待确认（UI 阶段一并确认）」。

---

## 需求原文（用户）

1. RAG 文件上传时可像 skill 一样选择「软链接」还是「文件拷贝」：
   - 软链接**不用真创建软链接**，只把文件地址记录下来，不真拷贝/不建软引用；**默认软链接**；
   - 导入方式同 skill 需要一个**提示按钮**，悬浮或点击展示导入方式的区别和不同；
   - 文件导入需**增加 md5 值的生成**，用于后续的更新检测。
2. 文件上传**内容处理、向量处理、查看处理都不变**。
3. **删除处理**：软链接方式**不能真删物理文件**，只能删我们自己的记录和向量。
4. **文件地址打开处理**：软链接则打开原始地址，文件拷贝则打开拷贝地址。
5. **文件更新操作**，弹出更新框：
   - a. 软链接文件：先检查原始文件是否存在且是否有更新，**不存在则提示「原始文件不存在，无法更新」且不能操作任何东西**；
   - b. 文件拷贝文件：先检查原始文件是否存在且是否有更新（**老版本没有 original_path 和 md5 属性**：无 md5 → 默认有更新；无 original_path → 跳过检测直接重新上传即现逻辑，但**新上传的文件需记录原始地址**，此处理仅为了兼容老版本）；如果原始文件存在，需**用户手动选择**：从原始文件更新 / 手动上传文件更新。
6. 列表右上角增加**批量文件更新按钮**：点击开启异步线程处理，按钮进入 **loading 状态**，且有**「查看进度」可点击字样**，并弹框处理进度（进度同上传更新文件的进度）；此弹框可被关闭，再次点击批量更新按钮可弹出此框。
7. **应用版本 +1**。

---

## 工作方式（用户要求）

整个改造按用户指定的三步推进，**每步需用户确认后才进入下一步**（串行 + 确认门）：

| 步骤 | 内容 | 对应产物 / 阶段 | 当前状态 |
|------|------|----------------|---------|
| 第 1 步 | 按需求用**假数据**画出页面与流程，由用户做 UI 与流程确认，反复调整直到确认完成 | 阶段 0（仅前端，无后端） | ⬜ 待开始 |
| 第 2 步 | 根据 UI 确认后的结果产出**详细改造计划**（前后端、按功能、按模块、按阶段拆分），由用户确认，文档放在 `doc/` 目录 | 本文件 `doc/rag_import_method_20260821.md` | 🔄 待用户确认 |
| 第 3 步 | 开始**实际编码**，每完成一个阶段必须把进度记录回执行计划文档（文末「进度记录」表），防止任务中断 | 阶段 1–11 | ⬜ 待开始 |

> 第 1 步产出 UI 定稿；第 2 步即本执行计划，待用户确认；第 3 步按本计划阶段逐个编码并回填进度。

---

## 已确认决策（用户已答复）

| # | 问题 | 用户答复 |
|---|------|---------|
| 1 | 批量文件更新（异步自动）扫描检测的范围 | **全部文档都检查**（软链接走原始文件 md5 检测；文件拷贝走 meta.md5 检测；无 original_path 的老版本跳过检测）。 |
| 2 | 软链接文档原始文件被移动/删除（向量仍在但内容读不回）的列表可视化标记 | **列表可视化标记**（行名旁加警告角标/灰化，「打开文件夹」点后提示原始不存在，「更新」置灰并 hover 提示原因）。 |
| 3 | 文件列表每行是否显示导入方式（软链接/文件拷贝）标识 | **每行显示徽章**（文件名行 fileType 标签旁加小徽章：软链接 Link 图标 / 文件拷贝 Copy 图标，hover 显示原始地址）。 |

## 待确认（在阶段 0 UI 评审时一并敲定）

> 用户已授权「按当前计划自动执行，无需逐阶段确认」，故以下两点**按推荐默认锁定**推进：

| # | 问题 | 锁定值 |
|---|------|--------|
| A | 软链接存储：`rag/files/` 下要不要留一份实体文件？ | **不留**（贴合需求原文「不用真创建软链接，只是把文件地址记录下来」）。软链接文档在 `rag/files/` 下**只存 `{id}.meta`**（含 `original_path` + `md5`），**无实体内容文件**；上传/索引/查看时直接从 `original_path` 读字节。文件拷贝文档保持现状（拷贝一份到 `rag/files/{id}.{ext}`）。 |
| B | md5 生成时机 | **上传/导入时即生成并写入 meta**；更新检测时重读源文件算 md5 与 meta 比对；更新成功后刷新 meta.md5。 |

---

## 既有代码锚点（改造前必读）

### 前端
| 文件 | 作用 | 关键位置 |
|------|------|---------|
| `frontend/src/pages/RagPage.tsx`（1973 行） | RAG 页面全部 UI | 头部右上按钮 `292-302`；批量工具条 `325-362`；列表行 `387-496`（操作按钮 `451-493`）；上传弹窗 `UploadDialog` `1083-1170`；上传进度浮层 `725-821`；`TagEditor` `1683`；`ViewDialog` `1741`；`BatchTagsDialog` `1918` |
| `frontend/src/hooks/useRagData.tsx` | RAG 共享 state + actions（context provider） | `upload` `454-499`；`updateDoc` `516-534`；`remove`/`removeMany` `536-554`；`openLocation` `591`；事件监听 `rag://upload-progress`/`rag://reindex-progress`/`rag://model-download` `199-269` |
| `frontend/src/services/ragService.ts` | HTTP API 封装 | `uploadRagDoc`(`POST /rag/docs/upload` `{filePath,tags}`) `69-72`；`updateRagDoc`(`POST /rag/docs/update` `{id,filePath}`) `83-87`；`deleteRagDoc` `75-78`；`openRagFileLocation`(`POST /rag/open-location` `{id}`) `139-142`；`pickRagFiles`(`POST /rag/docs/pick`) `57-61` |
| `frontend/src/types/index.ts` `250-374` | RAG 类型 | `RagDoc` `253`、`RagDocInfo` `266`（含 `version`/`fileName`/`fileType`）、`RagPickedFile` `371` |
| `frontend/src/pages/SkillsPage.tsx`（参考范式） | skill 导入方式 UI | `MethodHelpIcon`（悬浮/点击 popover 解释软链 vs 拷贝）`44-126`；`ExportDialog` radio 选择 `822-851`（`method: 'symlink'\|'copy'`，默认 symlink `692`）；`InstallDialog` per-agent toggle `1018-1054` |
| `frontend/src/App.tsx` | provider 包裹 | `RagDataProvider` `10`/`89` |

### 后端
| 文件 | 作用 | 关键位置 |
|------|------|---------|
| `src-tauri/src/rag/service.rs`（2738 行） | RAG 核心 | `DocMeta` `1191-1215`（字段：id/name/title/tags/size/uploaded_at/chunk_count/version/file_type）；`content_path_for` `1671-1693`（三候选：`{id}.{ext}` / `{id}` / `{meta_name}`）；`upload_one_path_inner` `1539-1632`（读源文件→校验大小→`is_likely_text`→`decode_text`→新 uuid→`write_doc_and_index`）；`write_doc_and_index` `1416-1447`（写 content + reindex + 写 meta）；`create_doc_from_content` `1738-1828`（`rag_file_create` 工具）；`update_doc` `1836-1939`（`rag_file_update` 工具）；`update_doc_from_file` `1472-1537`（`update_rag_doc` 命令：读新文件→删旧 content→`write_doc_and_index`）；`list_docs` `1296-1336`；`get_doc` `1339-1360`；`delete_doc` `2331-2392`（删向量+删 content 候选+删 meta+tag stats）；`open_file_location` `2395-2411`（`reveal_in_file_manager`）；`MAX_UPLOAD_BYTES = 64MiB` `1942` |
| `src-tauri/src/models/rag.rs` | 序列化模型 | `RagDocInfo` `87-117`（id/name/size/uploaded_at/tags/chunk_count/file_type/version/file_name）；`RagDoc`（+content） |
| `src-tauri/src/commands/rag.rs` | Tauri 命令 | `upload_rag_doc(app, file_path, tags)`、`update_rag_doc(app, id, file_path)`、`delete_rag_doc(app, id)`、`open_rag_file_location(app, id)`、`list_rag_docs`、`get_rag_doc`、`pick_rag_files` 等 |
| `src-tauri/src/lib.rs` `472-493` | 命令注册 `generate_handler!` |
| `src-tauri/src/services/http_server.rs` `dispatch_mcp` ~`781`/`1009-1199` | MCP 工具分发（`rag_search`/`rag_get`/`rag_tag_search`/`rag_file_create`/`rag_file_update`/`rag_file_delete`） |
| `src-tauri/src/db/migration.rs` | 版本化迁移 | 当前 `TARGET_VERSION = 20`；`migrate_vN` + `apply_migration` match |
| `src-tauri/src/services/skill_service.rs`（参考） | skill 软链/拷贝 | `export_to_agents` `958`（`method=="copy"`→`copy_dir_recursive`；`=="symlink"`→`std::os::unix/windows::fs::symlink`）；`delete_skill` `1318`（软链必删、拷贝按 opt-in）；`remove_link` `281`（安全删软链）；DB `skill_exports.method` 列存 `'symlink'\|'copy'` |
| `src-tauri/Cargo.toml` | 依赖 | **无 md5/sha2 直接依赖**（`sha2` 仅作为 candle/tokenizers 传递依赖存在于 Cargo.lock）——需新增 `md-5`（或 `sha2`） |

### 存储说明（关键）
RAG 文档是**文件型存储**（不是 DB 表）：
- 实体内容文件：`<app_data>/rag/files/{id}.{ext}`（上传）/ `{name}.{ext}`（`rag_file_create`）/ 软链接则**无实体文件**（阶段 A 默认）。
- meta：`<app_data>/rag/files/{id}.meta`（`DocMeta` 序列化 JSON）。
- 向量：`<app_data>/rag/lancedb`（按 doc id 删/查）。
- **新增字段全部进 `DocMeta` JSON（`#[serde(default)]` 向后兼容），不进 DB 列**。skill 是 DB 表（`skill_exports.method` 列），RAG 不同——RAG 用 meta 文件，故 `method`/`original_path`/`md5` 写进 `DocMeta`。

### 版本号（四源同步，现 `1.0.30001`）
`src-tauri/tauri.conf.json` / `src-tauri/Cargo.toml` / 根 `package.json` / `frontend/package.json`（+ `Cargo.lock`）。需求要求版本 +1 → `1.0.30002`。同步 `doc/upgrade/1.0.30002.md`（CI 发布时作 `latest.json` notes）。

---

## 阶段划分

> **总原则**：阶段 0 先画假数据 UI 供用户确认；UI 定稿后，按「后端数据层 → 后端各功能 → 前端各功能 → i18n/版本/文档」顺序推进。每阶段完成后在文末「进度记录」回填 ✅ + 日期 + 验证结果。

---

### 阶段 0：UI 样式确认（仅前端，假数据，可运行可点，无后端）

**目标**：在 `RagPage.tsx` 用假数据画出全部新 UI 与流程，供用户确认 UI 与交互。**不接 Tauri 命令、不动 Rust、不动后端**。用本地 state 模拟异步。用 `MOCK_IMPORT_METHOD` 开关控制（`true` 时用 `MOCK_DOCS` 假数据 + 强制 `disabled=false` 便于逐按钮验证；`false` 恢复真实流程）。

**涉及文件**（仅前端）：
- `frontend/src/pages/RagPage.tsx`（主要改造）：新增 `RagMethodHelpIcon` / `UpdateDialog` / `BatchUpdateConfirmDialog` / `BatchUpdateDialog` / `TagSearchSelect` 组件 + mock 假数据 `MOCK_DOCS` + mock state/handlers。
- `frontend/src/index.css`：补 `.hub-icon-btn:disabled` 置灰样式。
- `locales/{en,zh,fr,tr}.json`：阶段 0 已先行落位的键（导入方式/更新弹框/批量确认+进度/标签搜索/丢失标记/「文档导入」重命名）——用 `t(key, 'fallback')` 内联兜底，待阶段 11 正式补齐缺失键。
- `frontend/src/types/index.ts`（**仅加 mock 用的类型字段**，后端阶段再正式落位）

**要画的 UI / 流程**（⚠️ 以下为 UI 评审迭代定稿后的最终版，区别于初版）：
1. **上传弹窗 `UploadDialog` 加「导入方式」选择**：**分段切换样式**（仿 skill `InstallDialog` 的 per-agent toggle，非 radio）——「软链接（`Link2`，默认选中）」/「文件拷贝（`Copy`）」两个并排按钮，选中态背景 `var(--hub-surface)` + 加粗。**「导入方式」标题 + `?` 帮助按钮 + 分段切换器放同一行**（`flex items-center gap-2 flex-wrap`，不换行）。帮助按钮 `RagMethodHelpIcon` 悬浮/点击弹 popover 解释软链 vs 拷贝区别。**「文档上传」文案统一改为「文档导入」**（按钮/弹框标题/选择/确认/失败/进度文案，四语言同步）。
2. **文件列表行加「导入方式徽章」**：文件名行 fileType 标签旁加小徽章（软链接 `Link2` / 文件拷贝 `Copy`），hover 显示原始地址。
3. **文件列表行加「原始文件丢失」标记**：⚠️ 标记 + 「原始丢失」文字（红色）。**丢失行不整行置灰**——仅**「查看」「打开文件夹」按钮置灰**（`hub-icon-btn:disabled` CSS 已补 `opacity:0.45`+`not-allowed`）；「分片查看」「更新」按钮**不受影响**可正常点（丢失仅无法自动更新）。查看弹框：mock 文档点查看弹占位内容弹框（真实内容待阶段 2/3）。
4. **单条「更新」弹框**（重写 `handleUpdate`，不再直接弹 OS picker）：
   - 先「正在检查原始文件…」（0.9s mock）→ 按分支：
     - **原始丢失**（软链/拷贝）：提示「原始文件不存在，无法从原始文件更新，可手动上传覆盖」+ **「手动上传文件更新」为主按钮**（「从原始文件更新」隐藏）。
     - **原始存在有更新**：提示「检测到原始文件有更新」+「从原始文件更新」按钮。
     - **原始存在无更新**：提示「原始文件无更新」+ 仍允许「手动上传文件更新」。
     - **老版本无 original_path（拷贝）**：提示「该文档为老版本…请手动选择文件更新，新文件将记录原始地址」+ 仅「手动上传」主按钮。
   - 「从原始/手动」点击后驱动更新进度（1.3s mock）→ 完成 toast。
5. **列表右上角「批量文件更新」按钮**：加在「相关度搜索 / 文档导入 / RAG设置」**同一行**。**未运行**显示「批量更新」（`RefreshCw`）→ 点击启动；**运行中**按钮文字变「查看进度」+ `Loader2` 旋转 → 点击只重开进度弹框、不重启任务（无独立「查看进度」按钮）。
6. **批量更新前置确认**：点「批量更新」**不直接执行**，先预扫描（0.6s mock）弹确认框：
   - 2×2 统计：**文档总数 / 待更新 / 无需更新 / 原始丢失**；
   - 待更新=0 → 显示「没有需要更新的文档」+「开始更新」按钮禁用（显示 ✓）；
   - 原始丢失>0 → ⚠️ 提示「N 个文档原始文件丢失，将跳过自动更新（可单独手动上传覆盖）」；
   - 用户点「开始更新」→ 真正启动后台任务 + 进度弹框。
7. **批量更新进度弹框**（上传浮层同款样式，**只显示文件进度 + 向量进度，不逐条列出文件状态**）：
   - 居中小卡片（`max-w-sm`）：顶部 spinner/完成 Check + 文案 + 文件进度条（current/total）+ 向量进度条（reindex 子进度%）+ 底部「关闭弹框不中断任务」提示；
   - **可关闭**（右上 X），关闭只关弹框、不中断后台线程；**批量运行中再点「查看进度」按钮重开弹框**（不重启任务）。
8. **文件列表搜索增加标签搜索**（新增）：文件名搜索框右侧加 `TagSearchSelect`——**可搜索的多选下拉框**：
   - 触发按钮 `minWidth:240`：`Tag` 图标 + 「按标签筛选…」（未选）/「N 个标签」（已选）+ 下拉箭头；
   - 下拉面板：搜索输入框（实时过滤）+ 已选 chip 区（点 chip 取消，「清除」一键清空）+ 复选项列表（带勾选框）；
   - **多选 OR 语义**：文档含任一选中标签即命中，与文件名搜索 AND 联合；
   - 选项列表 `maxHeight:200` + `overflow-y-auto` + `overscrollBehavior:'contain'`（标签多时滚轮只滚列表不带动页面）；
   - 选项来源：当前文档聚合去重排序的所有标签。

**假数据**（6 条覆盖各分支）：
- `设计文档.md` 软链接·存在有更新；`架构说明.md` 软链接·存在无更新；`已删除源文件.txt` 软链接·**丢失**；
- `接口规范.java` 拷贝·存在有更新；`老版本笔记.txt` 拷贝·**老版本无 original_path**；`丢失拷贝.py` 拷贝·**丢失**。

**验收**：用户跑 `npm run dev` 进入 RAG 页，逐一点击上传弹窗（分段切换+帮助+同行）/列表徽章/丢失标记（仅查看+打开置灰）/单条更新各分支/批量更新按钮（前置确认→开始更新→进度弹框关闭与重开）/标签多选搜索，确认 UI 与流程无误。

> ⚠️ 阶段 0 完成后**必须由用户书面确认 UI 定稿**，再进入阶段 1。确认时一并敲定「待确认 A/B」两点。

---

### 阶段 1：后端 — 数据模型 + MD5 + 迁移

**目标**：给 `DocMeta` 加 `method`/`original_path`/`md5` 字段（serde 向后兼容），加 md5 计算 crate，无破坏性。

**涉及文件**：
- `src-tauri/Cargo.toml`：新增 `md-5 = "0.10"`（轻量，选 md-5 贴合需求「md5」；若团队偏好 sha256 则改 `sha2`）。
- `src-tauri/src/rag/service.rs`：
  - `DocMeta` 加 `#[serde(default)] method: Option<String>`（`"symlink"`/`"copy"`，None 视为老版本/拷贝兜底）、`#[serde(default)] original_path: Option<String>`、`#[serde(default)] md5: Option<String>`。
  - 新增 helper `fn compute_md5(bytes: &[u8]) -> String`（`Md5::digest` → hex）。
  - `content_path_for` 增加软链接分支：`meta.method == Some("symlink")` 时直接返回 `Path::new(&original_path?)`（不再查 `rag/files` 候选）。
- `src-tauri/src/models/rag.rs`：
  - `RagDocInfo` 加 `method: Option<String>` / `original_path: Option<String>` / `md5: Option<String>` / `lost_original: bool`（`#[serde(default)]`，序列化 camelCase）。
  - `RagDoc` 同加（查看需知 method 以决定读源）。
- `src-tauri/src/db/migration.rs`：**无需 DB 迁移**（RAG 文档不进 DB 表，字段在 meta JSON）。仅若后续要把 `method` 索引化才需迁移——本期不做。**`TARGET_VERSION` 不变**（保持 20），除非有其它同期迁移。> 注：如团队要求把 lost_original 持久化，仍只在 meta，不动 DB。

**验收**：`ORT_SKIP_DOWNLOAD=1 cargo check` 通过；旧 `.meta`（无新字段）反序列化不报错（serde default 生效）。

**进度记录**：`[ ] 待开始`

---

### 阶段 2：后端 — 上传 / 导入方式

**目标**：`upload_rag_doc` 增加 `method` 参数；软链接不拷贝实体、只记 `original_path`+`md5` 并直接从原路径读字节索引；文件拷贝维持现状并补记 `original_path`+`md5`。

**涉及文件**：
- `src-tauri/src/commands/rag.rs`：`upload_rag_doc(app, file_path, tags, method: Option<String>)`（`method` 缺省默认 `"symlink"`，贴合需求「默认软链接」）。
- `src-tauri/src/rag/service.rs`：
  - `upload_one_path` / `upload_one_path_inner` 增加 `method` 入参，分两支：
    - **`symlink`**：不 `std::fs::write` content 文件；直接用读到的 `raw` 走 `reindex_doc`（chunk+embed）；`DocMeta` 写 `method=Some("symlink")`、`original_path=Some(file_path)`、`md5=Some(compute_md5(&raw))`、`size=raw.len()`；**不落 `{id}.{ext}` 实体文件**。`file_stem` 传空/占位（`content_path_for` 软链分支不查它）。
    - **`copy`**：维持现 `write_doc_and_index`（写 `{id}.{ext}`），`DocMeta` 额外写 `method=Some("copy")`、`original_path=Some(file_path)`、`md5=Some(compute_md5(&raw))`。
  - `write_doc_and_index` 签名增 `method`/`original_path`/`md5` 透传到 `DocMeta`（或上传分支在写 meta 时补字段——二选一，倾向后者减少签名改动）。
  - `list_docs`/`get_doc`：读 meta 后，若 `method==symlink`，**不**走 `content_path_for` 的 `rag/files` 候选，直接用 `original_path`（已在阶段 1 的 `content_path_for` 改造里）；`get_doc` 读 content 从 `original_path` 读字节再 `decode_text`（与上传同一解码路径，保证「查看处理不变」）。计算 `lost_original = method==symlink && original_path 不存在`。
  - `RagDocInfo`/`RagDoc` 回填 `method`/`original_path`/`md5`/`lost_original`。

**验收**：上传一个 md 文件选「软链接」→ `rag/files/` 下只有 `{id}.meta`、无实体文件、向量已写入；选「文件拷贝」→ 有 `{id}.{ext}` 实体 + meta 带 original_path+md5。`list_rag_docs` 返回带 method/original_path/lost_original。

**进度记录**：`[ ] 待开始`

---

### 阶段 3：后端 — 删除 / 打开 / 查看 分支 + 丢失检测

**目标**：删除软链接不碰物理文件；打开按 method 开原始/拷贝地址；查看软链接从原始路径读。

**涉及文件**：
- `src-tauri/src/rag/service.rs`：
  - `delete_doc`：开头读 meta，若 `method==symlink` → **跳过删 content 文件候选**（只删 meta + 向量 + tag stats）；若 `copy` → 维持现状（删 content + meta + 向量）。
  - `open_file_location`：读 meta，`method==symlink` 且 `original_path` 存在 → `reveal_in_file_manager(original_path)`；不存在 → 返回 `Err("原始文件不存在")`（前端阶段 0 已 toast）；`copy` → 维持现状 reveal `rag/files` 拷贝。
  - `get_doc`：软链接从 `original_path` 读（阶段 2 已含）；`lost_original` 时返回 meta 但 content 置空/标记（前端阶段 0 已灰化，查看可提示）。
  - 抽 `fn original_exists(meta) -> bool`（`method==symlink` 且 `original_path` 文件存在）供删除/打开/更新/批量复用。

**验收**：删软链接文档 → 原始文件仍在、meta/向量已清；删拷贝 → 拷贝+meta+向量清。打开软链接 → Finder/Explorer 定位原始文件；原始丢失 → 报错。

**进度记录**：`[ ] 待开始`

---

### 阶段 4：后端 — 单条更新弹框逻辑

**目标**：新增「检查更新」命令 + 改造更新命令，支撑弹框的软链接/拷贝/老版本/丢失三分支（丢失分支在前端不阻断，强制走手动上传）。

**涉及文件**：
- `src-tauri/src/commands/rag.rs`：
  - 新增 `check_rag_update(app, id) -> RagUpdateCheck`：读 meta，返回 `{ method, hasOriginalPath, originalExists, hasMd5, originalChanged }`。
    - `originalChanged`：`originalExists && compute_md5(读原始文件) != meta.md5`；`!hasMd5`（老版本无 md5）→ 视为 `true`（有更新）；`!hasOriginalPath`（老版本无 original_path）→ `originalExists=false`、走兼容分支；`lostOriginal` → `originalExists=false`（前端走手动上传为主按钮）。
  - 改造 `update_rag_doc`：增 `mode: "original" | "file"` + `file_path: Option<String>`：
    - `mode="original"`：从 `meta.original_path` 读字节重新索引（软链接/拷贝通用）；更新 meta.md5=新 md5、`copy` 还要重写拷贝文件、version+1。
    - `mode="file"`：维持现 `update_doc_from_file`（读新 file_path），**额外记录 `original_path=Some(file_path)` + `md5`**（兼容老版本「新上传需记录原始地址」）；method 沿用 meta 现有值（若老版本 method 为 None，按拷贝语义补 `method=Some("copy")`，因为手传了新文件=产生了拷贝）。
- `src-tauri/src/models/rag.rs`：加 `RagUpdateCheck` 结构。
- `src-tauri/src/lib.rs`：注册 `check_rag_update`。

**验收**：软链接 + 原始丢失 → `check` 返回 `originalExists=false`（前端「从原始」隐藏、手动上传为主按钮）；软链接 + 原始有更新 → `originalChanged=true`（前端显示「从原始文件更新」）；拷贝 + 老版本无 original_path → `hasOriginalPath=false`（前端走手动上传单按钮，上传后补 original_path+md5）。

**进度记录**：`[ ] 待开始`

---

### 阶段 5：后端 — 批量更新异步任务 + 进度事件

**目标**：新增异步批量更新命令（**带前置预扫描**），逐条检查全部文档、对有更新的自动重新索引，发进度事件；按钮 loading + 「查看进度」+ 可关闭/重开弹框由前端消费。

**涉及文件**：
- `src-tauri/src/commands/rag.rs`：
  - 新增 `preview_batch_update(app) -> BatchPreview`：同步快速扫描全部文档，返回 `{ total, toUpdate, skipped, lost }`（供前端确认框展示文件更新数量）。
  - 新增 `batch_update_rag_docs(app) -> ()`：`tauri::async_runtime::spawn` 后台遍历 `list_docs` 全部文档，逐条：
    1. 发 `rag://batch-update-progress` 事件 `{ phase: "checking" | "reindexing" | "done", current, total, name, docId, reindexPct? }`（**不逐条发 skipped/lost/failed 状态**，前端进度弹框只显示文件进度 + 向量进度）；
    2. 软链接/拷贝：`original_exists`? 否 → 跳过；是 → 算 md5 比 meta.md5：变 → 重新索引（复用阶段 4 `mode="original"`）发 `reindexing`+`done`；不变 → 跳过；
    3. 文件拷贝老版本无 original_path → 跳过（留给单条手动上传补地址）。
  - 用全局任务句柄防止并发重复启动（已 loading 时再点只重开弹框，不重启任务——前端阶段 0 已体现，后端用 `OnceLock<Mutex<Option<JoinHandle>>>` 或原子 flag 守卫）。
- `src-tauri/src/rag/service.rs`：复用 `reindex_doc` + md5 helper；可能抽 `fn reindex_from_original(app, id) -> Result<()>`。
- `src-tauri/src/lib.rs`：注册 `preview_batch_update`、`batch_update_rag_docs`。
- 复用现有 `rag://upload-progress` 的 char 级事件（`reindex_doc` 内已发），前端进度弹框同上传更新进度样式。

**验收**：点批量更新 → 前端先调 `preview_batch_update` 弹确认框（展示统计）→ 用户确认 → 调 `batch_update_rag_docs` 后台跑、逐条发事件；原始丢失的跳过不崩；并发再点不重启。

**进度记录**：`[ ] 待开始`

---

### 阶段 6：前端 — 类型 / 服务层 / hook

**目标**：类型对齐后端、服务层加 method/检查/预览/批量命令、hook 加 state 与事件监听。

**涉及文件**：
- `frontend/src/types/index.ts`：`RagDocInfo`/`RagDoc` 加 `method?`/`originalPath?`/`md5?`/`lostOriginal?`；新增 `RagUpdateCheck`、`BatchPreview`；`RagPickedFile` 不变。
- `frontend/src/services/ragService.ts`：
  - `uploadRagDoc(filePath, tags, method)`；
  - 新增 `checkRagUpdate(id) -> RagUpdateCheck`（`POST /rag/docs/check-update`）；
  - `updateRagDoc(id, { mode, filePath? })`（`POST /rag/docs/update` 改 body）；
  - 新增 `previewBatchUpdate() -> BatchPreview`（`POST /rag/docs/batch-preview`）；
  - 新增 `batchUpdateRagDocs()`（`POST /rag/docs/batch-update`）。
- `frontend/src/hooks/useRagData.tsx`：
  - `upload(files, tags, method)` 透传 method；
  - 新增 `checkUpdate(id)`、`updateDoc(id, {mode, filePath})`、`previewBatchUpdate()`、`batchUpdate()` action + `batchUpdateState`（`idle/running/done`）、`batchProgress`（current/total/name/phase/reindexPct）、监听 `rag://batch-update-progress` 事件（仿现有 `rag://upload-progress` 监听 `199-269`）。
  - `RagDocInfo` 列表字段透传 method/originalPath/lostOriginal。
- `frontend/src/utils/tauriClient.ts`：新增 `mapRestToCommand` 分支 `/rag/docs/check-update`、`/rag/docs/batch-preview`、`/rag/docs/batch-update`，`/rag/docs/upload` 带 `method`、`/rag/docs/update` 改 `{id, mode, filePath}`。

**验收**：`npx tsc --noEmit` 无新错（项目用 vite build，类型不阻断但保持干净）。

**进度记录**：`[ ] 待开始`

---

### 阶段 7：前端 — 上传弹窗导入方式 + 帮助按钮

**目标**：把阶段 0 定稿的「导入方式分段切换 + 帮助按钮（同行）」接到真实后端。

**涉及文件**：
- `frontend/src/pages/RagPage.tsx`：`UploadDialog` 接 `method` state（默认 `symlink`）+ `RagMethodHelpIcon`（文案 RAG 语境，**标题+帮助+切换器同行**）；`handleUploadConfirm` 传 method 给 `upload`。

**验收**：选软链接上传 → 后端无实体文件；选拷贝 → 有拷贝。

**进度记录**：`[ ] 待开始`

---

### 阶段 8：前端 — 列表行徽章 + 丢失标记（仅置灰查看/打开）

**目标**：每行显示导入方式徽章 + 原始丢失仅置灰「查看」「打开文件夹」按钮（其余不受影响）。

**涉及文件**：
- `frontend/src/pages/RagPage.tsx` 列表行：fileType 标签旁加 method 徽章（`Link2`/`Copy`，hover 显示 `originalPath`）；`lostOriginal` 时加 `AlertTriangle` + 「原始丢失」标记，**不整行置灰**，仅「查看」「打开文件夹」按钮 `disabled={lost}`（`hub-icon-btn:disabled` CSS 已支持置灰）；「分片查看」「更新」不受影响。
- `frontend/src/index.css`：`hub-icon-btn:disabled` 已补（阶段 0 已加）。

**验收**：列表能一眼区分软链/拷贝/丢失；丢失行仅查看+打开置灰，分片/更新可点。

**进度记录**：`[ ] 待开始`

---

### 阶段 9：前端 — 单条更新弹框

**目标**：把阶段 0 定稿的更新弹框（软链/拷贝/老版本/丢失分支 + 从原始/手动上传两动作 + 进度浮层）接到 `checkUpdate`+`updateDoc`。丢失分支：手动上传为主按钮。

**涉及文件**：
- `frontend/src/pages/RagPage.tsx`：`UpdateDialog`（替换现 `handleUpdate` 直接弹 picker 的逻辑）；`handleUpdate(doc)` 改为打开 `UpdateDialog`；弹框内调 `checkUpdate` 后按分支渲染；「从原始文件更新」→ `updateDoc(id,{mode:'original'})` + 复用上传进度浮层；「手动上传」→ `pickFiles` + `updateDoc(id,{mode:'file',filePath})`；丢失分支 `originalExists=false` → 隐藏「从原始」、手动上传为主按钮。

**验收**：各分支流程与阶段 0 一致。

**进度记录**：`[ ] 待开始`

---

### 阶段 10：前端 — 批量更新按钮 + 前置确认 + 进度弹框

**目标**：头部右上批量按钮（文字变「查看进度」+ loading）+ 前置确认弹框（展示文件更新数量）+ 可关闭/重开进度弹框（上传同款：只文件进度+向量进度）。

**涉及文件**：
- `frontend/src/pages/RagPage.tsx`：头部右上加「批量文件更新」按钮（`RefreshCw`/`Loader2`）；未运行显示「批量更新」→ 点击调 `previewBatchUpdate` 弹 `BatchUpdateConfirmDialog`（2×2 统计 total/toUpdate/skipped/lost + 「开始更新」）；运行中文字变「查看进度」+ spinner → 点击只重开进度弹框；`BatchUpdateDialog` 复用上传浮层样式（文件进度条 + 向量进度条，不逐条列出状态）；弹框关闭只 `setShowBatchUpdateDialog(false)`、不中断任务；后端阶段 5 守卫 + 前端 state 双保险防双触发。
- `frontend/src/hooks/useRagData.tsx`：暴露 `previewBatchUpdate`/`batchUpdate`/`batchUpdateState`/`batchProgress`/`showBatchUpdateDialog` 控制。

**验收**：点批量 → loading + 弹框；关闭弹框 → 任务继续；再点按钮 → 重开弹框不重启；扫完 → loading 结束。

**进度记录**：`[ ] 待开始`

---

### 阶段 11：i18n + 版本 +1 + 文档同步

**目标**：补全 4 语言 i18n；版本四源 `1.0.30001 → 1.0.30002`；同步 AGENTS.md 与 upgrade notes。

**涉及文件**：
- `locales/{en,zh,fr,tr}.json` `pages.rag` 下新增键：
  - `importMethod` / `importMethodHelp` / `symlink` / `fileCopy` / `symlinkHelp` / `fileCopyHelp`（RAG 语境，区别于 skill）；
  - `updateDialogTitle` / `updateChecking` / `updateOriginalLost` / `updateOriginalChanged` / `updateNoChange` / `updateFromOriginal` / `updateManualUpload` / `updateManualUploadLegacyHint` / `updateRecheck`（兼容老版本提示「将记录新的原始地址」）；
  - `batchUpdate` / `batchUpdateHint` / `batchViewProgress` / `batchViewProgressHint` / `batchScanChecking` / `batchConfirmSummary` / `batchScanTotal` / `batchScanToUpdate` / `batchScanSkipped` / `batchScanLost` / `batchScanLostHint` / `batchScanNone` / `batchConfirmStart` / `batchProgressTitle` / `batchCheckingFile` / `batchPhaseDone` / `batchCloseDialog` / `batchCloseHint` / `batchUpdateDone` / `originalLostView`；
  - `methodSymlink` / `methodFileCopy` / `methodSymlinkTip` / `methodFileCopyTip`（列表徽章 hover） / `originalLost` / `originalLostTag`（列表丢失标记）；
  - `tagSearchPlaceholder` / `tagSearchInputPlaceholder` / `tagSearchCount` / `tagSearchClear` / `tagSearchEmpty` / `tagSearchFilter`（标签多选搜索）；
  - `openOriginalHint` / `openCopyHint`（打开地址提示） / `mockViewContent`（阶段 0 占位，接后端后删）；
  - **「文档上传」→「文档导入」重命名**：`upload` / `uploadDialogTitle` / `uploadConfirm` / `uploadFailedFile` / `uploadAllFailed` / `uploadPartialFailed` / `uploadingFile` / `uploadingDone`（四语言已改）。
- 版本四源 + `Cargo.lock`：`1.0.30001 → 1.0.30002`。
- `doc/upgrade/1.0.30002.md`：新建（CI 作 latest.json notes）。
- `AGENTS.md`：在「桌面端本地自定义功能」加一节（仿 §3.10 RAG 工具格式），记录软链接/拷贝导入方式、md5、批量更新（前置确认+进度）、标签搜索、DocMeta 新字段、命令清单。
- 本文档「进度记录」全部回填 ✅。

**验收**：`npm run build` 通过；`ORT_SKIP_DOWNLOAD=1 cargo check` 通过；`JSON.parse` 4 locale 通过；版本四源一致。

**进度记录**：`[ ] 待开始`

---

### 阶段 12：全量复查（3 轮，逐轮检查 + 修复）

**目标**：阶段 1–11 全部编码完成后，对**当前所有修改**做 **3 轮**独立全量复查，每轮都要从头审视全部前后端改动逻辑是否有问题、疏漏、bug，并**当场修复**发现的问题。每轮独立、不可合并——前一轮修复后引入的新问题可能在后一轮被发现。

**检查范围**（每轮都全量覆盖）：
- **后端**：`rag/service.rs`（DocMeta 字段 / `content_path_for` / 上传 / 删除 / 打开 / 查看 / 单条更新 / 批量更新 / `reindex_doc` / md5 helper）、`commands/rag.rs`（`upload_rag_doc`/`update_rag_doc`/`check_rag_update`/`preview_batch_update`/`batch_update_rag_docs` 等签名 + 注册）、`models/rag.rs`（`RagDocInfo`/`RagDoc`/`RagUpdateCheck`/`BatchPreview` serde + camelCase）、`db/migration.rs`（如有迁移）、`lib.rs`（命令注册）、`http_server.rs`（MCP 工具分发是否受影响）、`Cargo.toml`（md5 依赖）。
- **前端**：`RagPage.tsx`（导入方式分段切换 / 列表徽章 / 丢失置灰仅查看+打开 / 更新弹框三分支 / 批量前置确认 / 进度弹框上传同款 / 标签多选搜索 / 「文档导入」文案）、`useRagData.tsx`（state/actions/事件监听）、`ragService.ts`（API 封装）、`tauriClient.ts`（路由映射 + 响应转换）、`types/index.ts`（类型字段）。
- **i18n/版本/文档**：4 语言 JSON 键齐全且 `JSON.parse` 通过；版本四源一致；AGENTS.md 与 upgrade notes 同步。
- **mock 清理**：`MOCK_IMPORT_METHOD` 开关最终置 `false`（或删除 mock 分支/假数据/占位文案 `mockViewContent`），恢复纯真实流程。
- **跨切面**：老版本兼容（无 method/original_path/md5 的旧 meta）、并发守卫（批量再点不重启）、进度 state 竞争、错误路径（原始丢失/读源失败/重索引失败）、Windows 路径、空文档列表边界。

**每轮产出**：在下方「进度记录」回填该轮检查清单（发现的问题 + 修复点 + 验证结果）。

**验证**：每轮结束时 `cd frontend && npm run build` + `cd src-tauri && ORT_SKIP_DOWNLOAD=1 cargo check` 均通过；3 轮全部完成后做一次完整手动验证（软链/拷贝上传、单条更新各分支、批量更新前置确认+进度、丢失文档各按钮、标签搜索）。

**进度记录**：`[ ] 待开始`

---

## 进度记录

| 阶段 | 状态 | 完成日期 | 验证结果 |
|------|------|---------|---------|
| 阶段 0 UI 确认 | 🔄 待用户确认 UI | 2026-08-21（假数据画完） | 前端 `npx vite build` 通过（928ms，无报错）；待用户运行 `npm run dev` 逐一点击确认 |
| 阶段 1 后端数据模型+MD5+迁移 | ✅ 完成 | 2026-08-22 | `DocMeta` 加 method/original_path/md5（serde default）；`RagDocInfo`/`RagDoc` 加 method/original_path/md5/lostOriginal；新增 `RagUpdateCheck`/`BatchPreview`；Cargo.toml 加 `md-5`；`compute_md5`/`md5_of_file`/`classify_original` helper |
| 阶段 2 后端上传/导入方式 | ✅ 完成 | 2026-08-22 | `upload_one_path` 增 method 参数；symlink 不写拷贝、记 original_path+md5；copy 维持现状并记 original_path+md5；`write_doc_and_index` 签名增 method/original_path/md5；`list_docs`/`get_doc` 按 method 读源 |
| 阶段 3 后端删除/打开/查看+丢失检测 | ✅ 完成 | 2026-08-22 | `delete_doc` symlink 不删物理文件（只删 meta+向量）；`open_file_location` symlink 开原始地址、丢失报错；`get_doc` symlink 从 original_path 读、lost 返回空 content；`list_docs`/`get_doc` 计算 lost_original |
| 阶段 4 后端单条更新弹框逻辑 | ✅ 完成 | 2026-08-22 | 新增 `update_doc_from_original`（mode=original 从 original_path 读重索引+刷新 md5）；`update_doc_from_file`（mode=file 手传，记新 original_path+md5，老版本补 method=copy）；`check_rag_update`/`update_doc`/`rag_file_create`/`rag_file_update` 均 md5 刷新 |
| 阶段 5 后端批量更新异步任务 | ✅ 完成 | 2026-08-22 | 新增 `preview_batch_update`（分类统计）+ `batch_update_rag_docs`（spawn 后台 + CAS 守卫防双触发）+ `run_batch_update`（逐条 classify→changed 则 update_doc_from_original）；发 `rag://batch-update-progress` 事件；命令注册到 lib.rs |
| 阶段 6 前端类型/服务/hook | ✅ 完成 | 2026-08-22 | types 加 method/originalPath/md5/lostOriginal/RagUpdateCheck/BatchPreview；ragService 加 checkRagUpdate/previewBatchUpdate/batchUpdateRagDocs + uploadRagDoc(method)/updateRagDoc(mode)；tauriClient 加 check-update/batch-preview/batch-update 路由 + upload 带 method + update 改 mode/filePath；useRagData 加 checkUpdate/getBatchPreview/batchUpdate/batchUpdateRunning/batchProgress + 监听 rag://batch-update-progress + upload 透传 method + updateDoc 改 mode |
| 阶段 7 前端上传弹窗导入方式 | ✅ 完成 | 2026-08-22 | UploadDialog 始终传 method（默认 symlink）+ 分段切换 + 帮助按钮同行；handleUploadConfirm 传 method 给 upload |
| 阶段 8 前端列表行徽章+丢失标记 | ✅ 完成 | 2026-08-22 | 列表行 method 徽章（Link/Copy）+ hover 原始地址；lostOriginal ⚠️ 标记；仅「查看」「打开文件夹」置灰，分片/更新不受影响 |
| 阶段 9 前端单条更新弹框 | ✅ 完成 | 2026-08-22 | UpdateDialog 接 checkUpdate（runUpdateCheck 调后端）；runUpdateAction 调 updateDoc(mode: original\|file)；丢失分支手动上传为主按钮；reindexing/done/error 阶段 |
| 阶段 10 前端批量更新按钮+进度弹框 | ✅ 完成 | 2026-08-22 | 头部右上批量按钮（未运行「批量更新」→ 调 getBatchPreview 弹确认框；运行中「查看进度」+ spinner → 重开弹框）；BatchUpdateConfirmDialog（2×2 统计）；BatchUpdateDialog（上传同款双进度条）；batchProgress 来自 useRagData 事件 |
| 阶段 11 i18n+版本+1+文档 | ✅ 完成 | 2026-08-22 | 4 语言补 47 个新 i18n 键（JSON.parse 通过）；版本四源 +1.0.30002；新建 doc/upgrade/1.0.30002.md；AGENTS.md 加 §3.12 |
| 阶段 12 全量复查第 1 轮 | ✅ 完成 | 2026-08-22 | 检查全部前后端改动；清理阶段0注释；确认 mock 残留无害；cargo check + build 通过 |
| 阶段 12 全量复查第 2 轮 | ✅ 完成 | 2026-08-22 | 修复 lost_original 口径不一致：`classify_original`/`list_docs`/`get_doc` 的 lost_original 放宽为 `has_original_path && !original_exists`（不限 symlink），copy 丢失文档也标 lost，preview/run/UI 口径一致；cargo check + build 通过 |
| 阶段 12 全量复查第 3 轮 | ✅ 完成 | 2026-08-22 | 检查前后端契约 + 状态机健壮性：update mode/filePath 契约 OK；batchUpdate running 清零路径完整（done 事件 + CAS 守卫）；无新问题；cargo check + build + JSON.parse 通过 |
| 追加详细复查第 1 轮（后端逻辑） | ✅ 完成 | 2026-08-22 | 发现并修复 4 个：lost_original 误伤 copy 查看置灰（加 content_available 字段拆分语义）；CAS 守卫不防 panic（RAII guard）；早期 ? 不发 done 致前端假死（Err 补发 done）；丢失 symlink 改 tags 清空向量（跳过重索引） |
| 追加详细复查第 2 轮（前端契约） | ✅ 完成 | 2026-08-22 | 发现并修复 2 个：BatchUpdateDialog 向量进度条永不渲染（reindexPct 后端从不发，改复用 charProgress）；查看按钮丢失 RAG 关闭判断（改 disabled \|\| noContent）；filePath 空串契约确认功能一致 |
| 追加详细复查第 3 轮（i18n/边界/集成） | ✅ 完成 | 2026-08-22 | 发现并修复 2 个：rag_file_update 对 symlink 写内容覆盖用户原始文件（拒绝，MCP 只读）；nameExists fr/tr 缺失（补齐，4 语言 176 键统一）；版本五源/47 键 i18n/边界/MCP 路径全部确认无问题 |
| 追加复核第 4 轮（修复质量审查） | ✅ 完成 | 2026-08-22 | 修复 3 个：Err 补发 done 误导（改发 error phase + 前端 error 渲染）；update_from_original 覆写 MCP 改名（保留 meta.name，顺带修复子进度条 name 匹配）；copy 拷贝丢失 title 语义不准（通用文案） |
| 追加复核第 5 轮（端到端数据流） | ✅ 完成 | 2026-08-22 | 8 条完整用户流程逐字段追踪全部通过（camelCase/serde/类型对齐/事件链/兼容分支）；无新问题 |
| 追加复核第 6 轮（并发/边界/回归） | ✅ 完成 | 2026-08-22 | 修复 3 个：list_docs 硬失败（改 skip 语义）；meta 读改写无锁竞争（META_LOCK 顺序锁 meta->runtime + write_meta_atomic 原子写，根治交错写与删除复活）；其余 12 项（Windows/特殊字符/空文件/MCP schema/外部调用者）确认无问题 |

### 阶段 12 复查明细（每轮回填）

**第 1 轮**：
- 检查清单：
  - 后端：DocMeta 字段 serde 兼容性、content_path_for symlink 分支、upload symlink 不写拷贝、delete_doc symlink 不删物理、open_file_location symlink 开原始、get_doc/list_docs 读源 + lost_original 计算、update_doc_from_original/file、check_rag_update、preview_batch_update、batch_update_rag_docs CAS 守卫 + 事件、rag_file_create/update md5 刷新、reindex_all symlink 读源、命令注册、Cargo.toml md-5 依赖。✅
  - 前端：types 字段、ragService 签名、tauriClient 路由 + method/mode 映射、useRagData state/actions/事件监听、RagPage mock 清理（MOCK_IMPORT_METHOD=false、假数据清空、mock 分支删除）、UploadDialog 分段切换+帮助、列表徽章+丢失置灰仅查看/打开、UpdateDialog 三分支接 checkUpdate、批量按钮+确认框+进度弹框、TagSearchSelect、文案导入、index.css disabled 样式。✅
- 发现问题：
  1. `MOCK_IMPORT_METHOD` 置 false 后，部分 mock 分支代码（handleOpenFolder/handleView/handleUploadConfirm 内的 `if (MOCK_IMPORT_METHOD)`）因 `MOCK_IMPORT_METHOD` 是非字面量 boolean 未被 TS 判死代码，但运行时永远走真实分支——已确认无逻辑错误。
  2. `MockDocInfo`/`MockUpdateCheck` 改为 `RagDocInfo`/`RagUpdateCheck` 的类型别名（阶段 0 遗留），dialog 组件签名复用无误。
  3. `startBatchUpdate` 原名重名问题已确认不存在（startBatchUpdate 内调 batchUpdate action）。
- 修复点：清理 `// 阶段0` 注释为正式注释；确认 mock 残留代码无害；前端 `batchProgress` 来自 hook（事件驱动），`BatchUpdateDialog` 用 `batchProgressTyped`。
- 验证结果：`cargo check` 通过（md-5 v0.10.6，9m33s，无 error/warning）；`npm run build` 通过（831ms）；4 语言 JSON.parse 通过；版本四源 1.0.30002 一致。

**第 2 轮**：
- 检查清单：聚焦跨函数交互与计数口径。
  - `classify_original.lost_original` 仅对 `symlink && has_original_path && !original_exists` 为 true；copy 文档原始丢失时 `lost_original=false`，会被 `run_batch_update` 当作 `original_changed=false` 跳过（行为正确：不重索引丢失源），但 `preview_batch_update` 会把它计入 `skipped` 而非 `lost`——**前端确认框「原始丢失」数不含 copy 丢失文档**，与用户语义预期不符。
- 发现问题：
  1. preview 与 run_batch_update 的 lost 口径不一致；copy 丢失文档在统计里不显示为「丢失」。
- 修复点：将 `classify_original.lost_original` 放宽为 `has_original_path && !original_exists`（不限 symlink），copy 丢失文档也标 lost，preview/run 口径一致，前端「原始丢失」数含全部丢失文档。
- 验证结果：`cargo check` 通过；`npm run build` 通过。

**第 3 轮**：
- 检查清单：聚焦前后端契约 + 状态机健壮性。
  - `tauriClient` `/rag/docs/update` 映射 `update_rag_doc(id, mode, filePath: Option<String>)`；mode='original' 忽略 filePath，mode='file' 空 filePath → 后端返回 Err。✅
  - `batchUpdate` 设 running=true 后调 `batchUpdateRagDocs`（返回 Ok 永不 throw）；running 清零完全靠 `phase='done'` 事件。后端 `run_batch_update` 末尾必发 done 事件（中途 update 失败也继续，最后发 done），spawn task 末尾 `BATCH_UPDATE_RUNNING.store(false)`——路径完整，无泄漏。
- 发现问题：无（第 2 轮修复后无新问题；状态机路径完整）。
- 修复点：无。
- 验证结果：`cargo check` 通过；`npm run build` 通过；4 语言 JSON.parse 通过；版本四源一致。3 轮复查全部完成。

---

## 追加三轮详细复查（2026-08-22，用户要求）

> 用户要求「再进行 3 轮详细的检查复核」。三个独立复核视角（后端逻辑正确性 / 前端逻辑与契约 / i18n+版本+边界+集成）并行执行，汇总发现并统一修复。

**追加第 1 轮（后端逻辑正确性 + 数据一致性）**：
- 检查清单：DocMeta serde 兼容、symlink 上传孤儿文件、delete/open/update-from-original/get_doc 各分支、preview/run 口径、reindex_all 丢失文档、CAS 守卫。
- 发现问题：
  1. 【中】`lost_original` 放宽后误伤 copy 丢失文档：copy 拷贝还在、内容可读，但前端据此置灰「查看/打开」——违反「丢失只影响自动更新」语义（已确认决策 #2 只针对软链接）。
  2. 【中】`BATCH_UPDATE_RUNNING` CAS 守卫不防 panic：spawn task panic 时 `store(false)` 被跳过 -> 后端批量更新永久失效（重启才能恢复）。
  3. 【中】`run_batch_update` 早期 `?`（files_dir/read_dir 失败）直接返回 Err 且不发 done 事件 -> 前端 `batchUpdateRunning` 永久假死（按钮停在「查看进度」）。
  4. 【低】`rag_file_update` MCP 工具对丢失 symlink 文档仅改 tags 时，读空 content 重索引 -> 原有向量被清空（不可逆数据丢失）。
- 修复点：
  1. 拆分语义：后端 `RagDocInfo`/`RagDoc` 新增 `content_available` 字段（symlink=原始存在；copy=拷贝存在）；`lost_original` 保持放宽口径（⚠️徽章 + 自动更新跳过，两种 method 都算丢失）；前端「查看/打开」置灰改用 `contentAvailable === false`。
  2. spawn task 内改用 RAII guard（`RunningGuard` Drop 时 `store(false)`），panic 也复位。
  3. spawn task 的 Err 分支补发 `done` 事件（`emit_batch_update_progress(app, 0, 0, "", "done")`），前端可靠清零。
  4. `update_doc` 对丢失 symlink 的 tag-only 更新跳过重索引（只持久化 meta.tags，chunks 保留旧 tags/embeddings），防向量清空。

**追加第 2 轮（前端逻辑 + 前后端契约 + 状态机）**：
- 检查清单：update mode/filePath 契约、batchUpdate 状态机、UpdateDialog 分支、handleView 兼容、TagSearchSelect、mock 残留、「文档导入」文案、BatchUpdateDialog 进度条。
- 发现问题：
  1. 【高】`BatchUpdateDialog` 向量进度条永不渲染：后端 `rag://batch-update-progress` 事件只发 `{current,total,name,phase}`，从不发 `reindexPct`；前端却用 `progress.reindexPct` 画向量进度条且用它判定 reindexing 状态 -> 向量进度条整块恒不显示，reindexing 阶段标题文案也错（显示「正在检查」而非「重新载入」）。
  2. 【中】列表行「查看」按钮 `disabled={lost}` 丢失 RAG 关闭判断（原为 `disabled={disabled}`），RAG 关闭时与兄弟按钮视觉不一致。
  3. 【低】tauriClient `filePath: '' ?? null` 结果是 `''`（非 null），后端 `Some("")` -> unwrap_or_default 为空串 -> mode='file' 返回 Err「file_path is required」——契约功能一致（非 bug，记录差异）。
- 修复点：
  1. `BatchUpdateDialog` 增 `charProgress` prop，向量进度条复用 `rag://upload-progress` 事件驱动的 `charProgress`（按 name 匹配当前文档，charsDone/charsTotal 算百分比，与上传浮层同范式）；`reindexing` 判定改为仅看 `phase === 'reindexing'`（去掉 reindexPct 门槛）；`BatchProgress` 类型删掉 `reindexPct` 字段。
  2. 查看按钮改回 `disabled={disabled || noContent}`（结合追加第 1 轮的 contentAvailable）。
  3. 无需修复（功能一致）。

**追加第 3 轮（i18n 完整性 + 版本一致性 + 边界场景 + 集成回归）**：
- 检查清单：47 个新增 i18n key 四语言齐全性、「文档导入」重命名、版本五源、AGENTS.md §3.12、空列表/全丢失/老版本边界、MCP 工具路径、Sidebar badge。
- 发现问题：
  1. 【中】`rag_file_update` MCP 工具对 symlink 文档带 `docContent` 调用时，会把新内容**写覆盖用户的原始文件**（symlink 无拷贝，content_path 解析到 original_path）——外部 AI agent 可借此静默覆盖用户只希望 RAG「链接」的文件，与删除保护语义不对称。
  2. 【低】`nameExists` 在 fr/tr 缺失（基线遗留，非本次引入；fr/tr 175 vs en/zh 176 的根源）。
  3. 【低】copy 文档拷贝被外部删除 + 原始还在：查看显示空内容且无丢失反馈——追加第 1 轮的 `content_available` 已顺带覆盖（拷贝不存在 -> content_available=false -> 按钮置灰 + toast 提示）。
- 修复点：
  1. `update_doc` 对 `method==symlink && content.is_some()` 直接返回 Err「symlink documents are read-only via rag_file_update...」（MCP 工具只读；UI 更新走 update_rag_doc 命令不受影响）。
  2. fr/tr 补 `nameExists` 翻译，4 语言 rag keys 统一为 176。
  3. 无需额外修复（content_available 已覆盖）。

**追加三轮验证结果**：
- `cargo check` 通过（增量 4.44s，无 error/warning）
- `npm run build` 通过（855ms）
- 4 语言 locale JSON.parse 通过，rag keys 均 176，`nameExists` 补齐
- 版本五源 1.0.30002 一致（tauri.conf.json / Cargo.toml / 根 package.json / frontend package.json / Cargo.lock）

---

## 追加三轮详细复核·第二批（2026-08-22，用户要求）

> 三个独立复核视角并行：第 4 轮（上一批修复本身的质量审查）/ 第 5 轮（端到端数据流追踪）/ 第 6 轮（并发竞争 + 深度边界 + 回归影响）。

**第 4 轮（上一批修复的质量审查）**：
- 检查清单：content_available 端到端布线、RAII guard、rag_file_update symlink 守卫与 if/else 结构、BatchUpdateDialog charProgress 改造、文案、锁定模式（4 处 meta 顺序锁）、mock 残留二次扫描。
- 发现问题：
  1. 【中】Err 分支补发 done 复用「完成」语义：批量任务早期失败（files_dir/read_dir 错误）时前端显示绿色 ✓「批量更新完成」--对失败的误导性确认。
  2. 【低】`update_doc_from_original` 用 file_name(original_path) 覆写 display_name -> MCP 改名（rag_file_update 的 docName）被静默回退；同时 reindex_doc 发出的 charProgress.name 与 batchProgress.name（meta.name）不匹配 -> 子进度条显示 0%。
  3. 【低】copy 拷贝被外部删除（原始还在）时「查看」置灰 title 提示「原始文件不存在」语义不准。
- 修复点：
  1. 后端 Err 分支改发 `phase="error"`；前端 useRagData 监听器接受 error phase 并清 running（不 refetch）；BatchUpdateDialog 渲染 error 状态（⚠️ 图标 + 红字「批量更新失败，请查看日志」）；`BatchProgress` 类型与 useRagData state 加 error phase；新增 i18n 键 `batchUpdateFailed`。
  2. `update_doc_from_original` 保留 `old_meta.name` 作为 display_name（不覆写）；charProgress.name 与 batchProgress.name 同源（均为 meta.name）-> 子进度条匹配恢复。
  3. 「查看/打开」置灰 title 改用语义准确的通用文案（`contentUnavailableView`「无法读取文档内容」/ `contentUnavailableOpen`「无法打开文件位置」），覆盖 symlink 原始丢失与 copy 拷贝丢失两种成因。

**第 5 轮（端到端数据流追踪）**：
- 检查清单：8 条完整用户流程逐字段追踪（软链接上传->列表->查看->更新->删除；文件拷贝上传->批量更新；老版本文档手动上传升级；丢失 symlink 各分支；标签编辑；rag_file_create/update MCP 路径；批量前置确认->进度->完成->列表刷新）。
- 发现问题：无。所有流程的字段名（snake_case->camelCase）、serde rename、类型对齐均正确；进度事件链完整（预览->确认->启动->checking/reindexing/done->refetch）；老版本兼容分支先于一切判断。
- 修复点：无。
- 边缘场景记录：后端 spawn panic 时前端 running 无兜底（依赖 done 事件）-> RAII guard + Err 补发已覆盖 Err 路径；纯 panic（极罕见）下重启恢复，接受。

**第 6 轮（并发竞争 + 深度边界 + 回归影响）**：
- 检查清单：批量+单条更新并发、批量+删除并发、批量+上传并发、事件乱序、Windows 路径、特殊字符文件名、0 字节文件、重复软链接导入、original_path 指向目录、外部 HTTP 调用者、MCP rag_get 响应 schema、其它 RagDocInfo 构造点。
- 发现问题：
  1. 【中】meta 读-改-写无锁：批量+单条更新同一文档可交错（窗口极窄）；**放大器**：`list_docs` 是唯一硬失败扫描器（`?` 传播）--一条坏 meta -> 整个列表清空（其余扫描器全部 skip 语义）。
  2. 【中】批量+删除同一文档：删除的向量清理在 runtime lock 下会等批量 reindex 完成，但随后的 meta 删除与批量的 meta 写无锁并发 -> 极窄窗口下已删文档被复活（meta 重写、无向量僵尸条目）。
  3. 【低】共享 charProgress：批量后台跑 + 单条更新并发时子进度条在 0% 与真实值间闪烁（name 不匹配守卫保证不显示错误数据，仅闪烁）。
- 修复点：
  1. `list_docs` 改 skip 语义（`let Ok(..) else { continue }`），与其余全部扫描器一致--一条坏 meta 不再拖垮整个列表。
  2. 新增 `static META_LOCK: OnceLock<Mutex<()>>` + `meta_lock()` helper：`update_doc_from_original`/`update_doc_from_file`/`update_doc`(rag_file_update)/`delete_doc`/`set_doc_tags` 全程持锁（锁序恒为 meta -> runtime，无 ABBA）；同根修复交错写与删除复活两个竞争。
  3. 新增 `write_meta_atomic()`（写 `{id}.meta.tmp` + rename）：meta 写原子化，崩溃/抢占不可能留下半截 JSON；`.meta.tmp` 扩展名为 tmp，所有扫描器天然跳过。`write_doc_and_index`/`update_doc`/`set_doc_tags`/`reindex_all` 的 meta 写全部切换。
  4. 闪烁问题（低）：记录不修（无数据危害；需事件来源标识或前端分流，复杂度不成比例）。
- 确认无问题（12 项）：批量+上传并发（快照外，无冲突）；tauri 事件同通道 FIFO 不乱序 + name 匹配守卫；Windows 路径（原生 Path 处理，无 Unix 假设）；特殊字符文件名（UTF-8 全链路安全）；0 字节文件（is_likely_text 空=文本，md5 d41d8...，0 chunk，前端分片按钮置灰）；重复软链接导入（不同 uuid 独立）；original_path 指向目录（picker 只选文件，不可达）；外部 HTTP 调用者（无 REST 路由，仅 Tauri command + MCP，无「不带 method 默认 symlink」外部路径）；MCP rag_get 只返回 content 文本块（新字段不泄漏进 schema）；其它 RagDocInfo/RagDoc 构造点仅 list_docs/get_doc（已补全）；rag_file_create 归入 skipped 正确；rag_file_update symlink 守卫不影响 copy 路径。

**第二批三轮验证结果**：
- `cargo check` 通过（修复 meta_lock await 后 5.14s，无 error/warning）
- `npm run build` 通过（913ms）
- 4 语言 locale JSON.parse 通过，rag keys 均 179（+batchUpdateFailed/contentUnavailableView/contentUnavailableOpen）

> 图例：⬜ 待开始 / 🔄 进行中 / ✅ 完成 / ⚠️ 阻塞

---

## 风险与注意事项

1. **老版本兼容**（需求 5b）：旧 `.meta` 无 `method`/`original_path`/`md5`。全部新字段 `#[serde(default)]`。`check` 命令对「无 md5 → 有更新」「无 original_path → 走手传兼容分支」必须先于一切判断。单条手动上传后**必须回写 original_path+md5**，否则下次仍是老版本语义。
2. **软链接存储选择**（待确认 A）：若用户在阶段 0 改选「拷贝一份到 rag/files」，则阶段 2/3 的「无实体文件」分支全部改为「有拷贝但 meta 标 method=symlink、删除不删拷贝/或删拷贝不删原始」——届时回滚阶段 2/3 设计。
3. **md5 crate 选型**（待确认 B）：默认 `md-5`；若团队统一用 `sha2`，阶段 1 改依赖即可，meta 字段名建议仍叫 `md5`（语义=「内容指纹」）或改 `content_hash`。
4. **批量更新并发**：后端必须守卫「已 running 时再触发不重启」；前端按钮再点只重开弹框。两者都要做，避免双触发。
5. **进度浮层复用**：单条更新与批量更新都复用 `uploadProgress`/`charProgress` 浮层，注意 state 竞争——批量跑时单条更新按钮建议禁用，反之亦然。
6. **不碰 origin**：所有改动仅在 `frontend/`、`src-tauri/`、`locales/`，遵守 AGENTS.md 核心约束。
7. **构建**：cargo 走本地代理 `127.0.0.1:7890`、cargo 不在 PATH（用 rustup/asdf toolchain），`ORT_SKIP_DOWNLOAD=1` 跳过模型下载做 check（见 memory `build-proxy`/`rag-build-setup`）。
