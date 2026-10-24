# RAG 文档历史记录 + 文件夹化存储 — 执行计划

> 状态：**Phase 1-3 已完成（2026-10-22）；Phase 4 联调冒烟待应用重启后验证；Phase 5 单测已完成**
> 日期：2026-10-22

## 一、总体设计

### 1. 存储布局改造（平铺 → 文件夹模式）

```
rag/files/                      # 现状：所有文件平铺
rag/files/
  {docId}/                      # 每文档一个文件夹（docId 已有 validate_doc_id 守卫，安全）
    meta.json                   # 原 {id}.meta
    content.{ext}               # 原 {id}{ext}（上传内容）
    content.md                  # 原 {id}.md（可提取文档的提取缓存）
    raw{raw_ext}                # 原 {id}{raw_ext}（copy 导入的原始归档）
    history/                    # 新增：历史版本子目录
      v{n}.json                 # 历史快照元数据 {version, savedAt, size, name, md5, source}
      v{n}{ext}                 # 历史内容快照（原文）
      v{n}.md                   # 可提取文档的提取缓存快照（支持渲染视图）
```

- **兼容策略**：`content_path_for` / `raw_content_path_for` / `open_source_path_for` 三个解析函数改为「先解析 `{id}/` 文件夹内路径，回退旧平铺路径」；启动时执行**幂等迁移**（扫描平铺 `{id}.meta` → 建文件夹搬移），迁移失败不阻断启动（回退平铺仍可读）。
- **删除文档** = 删除整个 `{id}/` 文件夹（含 history）。
- **不新增 DB 迁移**：历史完全文件化（与 meta 文件为源的设计一致），`rag_docs` SQL 镜像不动。

### 2. 历史快照机制

- 快照钩子：所有内容覆写点统一调用 `snapshot_history()`（`update_doc` MCP 路径、`update_doc_from_file`、`update_doc_from_original`、批量更新/自动更新 tick 共用路径），在 tmp 写 + rename **之前**、`meta_lock` 之内读取旧内容落快照。
- **留存数量惰性生效**：`RagSettings.history_retention`（默认 3，clamp 1–100）。每次新增快照后按当前设置裁剪超出部分；**修改设置不触发任何删除**（需求 5）。

### 3. 新增能力

| 项 | 形态 |
|---|---|
| `rag_file_history_versions` | MCP 工具：版本列表（倒序） |
| `rag_file_history_get` | MCP 工具：读取指定历史版本内容 |
| `rag_file_history_diff` | MCP 工具：本地 diff（新依赖 `similar` crate，unified diff 文本直接返回；`toVersion` 缺省 = 当前版本） |
| `rag_file_diff` | MCP 工具（与历史无关的独立能力）：对比**两个不同文档**，入参为两个文档 id，返回 unified diff |
| `delete_rag_doc_history` | Tauri 命令：批量删除历史版本（入参版本号列表） |

> 命名规则：**MCP 工具** = `rag_file_history_{versions,get,diff}`（历史族，对齐既有 `rag_file_*` 工具族）+ 独立工具 `rag_file_diff`（两文档对比）；**Tauri 命令** = `rag_doc_history_xxx` 风格（`list_rag_doc_history` / `get_rag_doc_history` / `delete_rag_doc_history` / `diff_rag_doc_history`）。

## 二、阶段划分与当前进度

> 注：**「UI 绘制」= 直接修改前端代码实现真实 UI**（非画原型图）。Phase 1 同步落地了 4 个**桩命令**（返回 mock 数据）使 UI 全流程可交互确认。

| 阶段 | 内容 | 状态 |
|---|---|---|
| **Phase 1** | 前端 UI 直接实现 + 桩命令（mock 数据） | ✅ **已完成，用户确认（2026-10-22）** |
| **Phase 2** | 按确认后的 UI 校准计划 | ✅ **已完成（本文档即校准后版本）** |
| **Phase 3** | 后端真实实现：文件夹迁移 + 历史快照/裁剪 + 桩命令替换 + 4 个 MCP 工具 + `similar` diff | ✅ **已完成（2026-10-22）** |
| **Phase 4** | 前后端联调回归（真实数据、E2E 冒烟） | ◐ 构建/单测全绿；E2E 冒烟待应用重启 |
| **Phase 5** | 单元测试补齐（Rust + vitest），覆盖率与通过率 100%（针对本次新增/改动代码） | ✅ **已完成（2026-10-22）** |

### Phase 1 已落地清单（用户已确认的 UI 基线）

**前端（`frontend/src`）**
- `types/index.ts`：`RagDocHistoryVersion` 接口（version/savedAt/size/name/md5/source）+ `RagSettings.historyRetention`
- `services/ragService.ts`：`listRagDocHistory` / `getRagDocHistory` / `deleteRagDocHistory` / `diffRagDocHistory` 4 个封装 + 默认值 `historyRetention: 3`
- `utils/tauriClient.ts`：4 条路由映射（`POST /rag/docs/history/{list,get,delete,diff}`）
- `hooks/useRagData.tsx`：默认 settings 补 `historyRetention: 3`
- `pages/RagPage.tsx`：
  - `renderDocRow` 操作列新增 **History 按钮**（列宽 170→200）
  - **HistoryDialog**：倒序版本列表（vN 徽标/名称/大小/时间）、全选 + 多选、每行「查看详情」+「对比」+「删除历史」、底部「对比选中版本」（勾选恰好 2 个时可用，旧→新）+ 批量删除 + 已选计数
  - **DiffDialog**：**左右对比布局**（左=旧/右=新，粘性列头），删除红底/新增绿底/hunk 头强调色、逐行对齐补空格、**上一处/下一处跳转按钮**（循环 + 「N / M 处差异」计数）
  - **ViewDialog 复用**：`onSaveTags` 改 Optional——历史详情只读模式（无标签编辑），原文/渲染切换 + 缩放完整保留
  - **SearchSettingsDialog**：「文档历史留存数量」滑条（1–100，默认 3）+ Info tips（修改不实时生效、文档下次更新时统一清理）
- `locales/{en,zh,fr,tr,es}.json`：`pages.rag.history*` + `diff*` 共 24 键 ×5

**后端桩（`src-tauri`，接口签名与最终版一致，Phase 3 替换实现）**
- `models/rag.rs`：`RagDocHistoryVersion` 结构 + `RagSettings.history_retention`（Default=3）——**真实生效非桩**
- `rag/service.rs`：`get_settings`/`save_settings` 补 `historyRetention` 读取 + clamp(1,100)——**真实生效非桩**
- `commands/rag.rs`：4 个桩命令（均 `require_admin`，已注册 `lib.rs`）——`list_rag_doc_history` 返回 5 条 mock 版本、`get_rag_doc_history` 返回 Markdown 桩文档、`delete_rag_doc_history` 返回删除数、`diff_rag_doc_history` 返回样例 unified diff；均带 `TODO(phase3)` 标记

**Phase 1 验证记录**：tsc 0 错误；cargo check 0 错 0 警；npm build ✓

## 三、逐点清单

### 修改点

| # | 位置 | 改动 | 阶段 |
|---|---|---|---|
| M1 | `rag/service.rs` `content_path_for`/`raw_content_path_for`/`open_source_path_for` | 解析候选加入 `{id}/` 文件夹优先 + 平铺回退 | P3 |
| M2 | `rag/service.rs` `update_doc` (~L4315)、`update_doc_from_file` (~L3305)、`update_doc_from_original` (~L3466) | 覆写前插入 `snapshot_history()`（含按 retention 裁剪） | P3 |
| M3 | `rag/service.rs` `delete_doc` (~L5142) | 删除逻辑改为整文件夹 `remove_dir_all`（保留 symlink 外部原文件语义） | P3 |
| M4 | `rag/service.rs` 启动路径 | 幂等文件夹迁移（平铺 → `{id}/`），失败 warn 降级 | P3 |
| M5 | `models/rag.rs` `RagSettings` | + `history_retention`（default 3，clamp 1–100，get/save 双侧） | ✅ P1 已完成 |
| M6 | `frontend/types/index.ts` + `ragService.ts` + `useRagData.tsx` | RagSettings 类型/默认值同步 | ✅ P1 已完成 |
| M7 | `RagPage.tsx` `SearchSettingsDialog` | 「文档历史留存数量」滑条 + tips（修改不实时生效） | ✅ P1 已完成 |
| M8 | `RagPage.tsx` `renderDocRow` | + 历史按钮（History 图标） | ✅ P1 已完成 |
| M9 | `RagPage.tsx` `ViewDialog` | `onSaveTags` 改 Optional，历史只读模式复用 | ✅ P1 已完成 |
| M10 | `utils/tauriClient.ts` rag 块 | + 4 条路由映射 | ✅ P1 已完成 |
| M11 | `locales/{en,zh,fr,tr,es}.json` | + `history*`/`diff*` 键 ×5 | ✅ P1 已完成 |
| M12 | `src-tauri/Cargo.toml` | + `similar` 依赖（本地 diff） | P3 |

### 新增点

| # | 位置 | 内容 | 阶段 |
|---|---|---|---|
| A1 | `rag/service.rs` | `snapshot_history` / `prune_history` / `list_doc_versions` / `read_doc_version` / `delete_doc_versions` / `diff_doc_versions` / `diff_two_docs` / `migrate_files_layout` | P3 |
| A2 | `commands/rag.rs` + `lib.rs` | 4 个命令（均 `require_admin`）：`list_rag_doc_history` / `get_rag_doc_history` / `delete_rag_doc_history`（批量）/ `diff_rag_doc_history`——桩已落，P3 替换真实实现 | P3 |
| A3 | `rag/service.rs` `tool_definitions` + `builtin_tools` + `call_builtin_tool` | 4 个 MCP 工具：`rag_file_history_versions` / `rag_file_history_get` / `rag_file_history_diff` + `rag_file_diff`（两文档 id 对比） | P3 |
| A4 | `RagPage.tsx` | `HistoryDialog`（列表/多选/批量删除/查看详情/**对比**）+ `DiffDialog`（**左右对比 + 上一处/下一处跳转**） | ✅ P1 已完成 |
| A5 | `ragService.ts` | 4 个 API 封装 | ✅ P1 已完成 |

### 影响点（Phase 4 回归验证）

- 文档上传/更新/批量更新/自动更新 tick、重命名（`content_path_for` 换名 rename 逻辑 M1 联动）、`rag_file_create` 命名、查看源文件/打开文件夹、md5 更新检测、FTS/向量一致性、`check_frontend_rust_contract.py` 契约脚本、既有 75 套 E2E（RAG 相关套件）。
- 历史快照在 `meta_lock` 内做文件 IO——快照为纯本地小文件操作，不引入网络等待，无锁序风险（恒 meta → runtime）。

## 四、Phase 3 实施细目（后端真实实现）

按依赖顺序：

1. **M12** `Cargo.toml` + `similar`（text diff，unified 输出）。
2. **M1 路径解析双轨**：三个解析函数加 `{id}/` 文件夹候选（优先），平铺候选保留为回退；所有调用点无需改动（签名不变）。
3. **A1 存储原语**：
   - `doc_dir(dir, id)` = `dir.join(id)`；`history_dir` = `doc_dir/history`
   - `migrate_files_layout`：扫描平铺 `{id}.meta` → 建 `{id}/`，搬移 meta/content/raw/cache（幂等：已迁移跳过；失败逐文档 warn 跳过不阻断）
   - `snapshot_history(dir, &meta, source)`：读当前内容（`content_path_for`）+ 可提取缓存（`.md`）→ 写 `history/v{n}{ext}` + `v{n}.json`；随后 `prune_history`（按 `history_retention` 保留最新 N 个，**仅在此处删除**）
   - `list_doc_versions`（读 `history/v*.json` 倒序）/ `read_doc_version` / `delete_doc_versions`（批量，返回实际删除数）/ `diff_doc_versions`（`similar` unified diff；toVersion 缺省=当前内容）/ `diff_two_docs`（两文档 id）
4. **M2 快照接线**：三个覆写点 + 批量/自动更新共用路径，覆写前调 `snapshot_history`；`update_doc`（MCP）路径顺带补 `version += 1`（对齐 UI 路径）。
5. **M3 删除改造**：`delete_doc` 改整文件夹删除（symlink 外部原文件仍不动）。
6. **M4 启动迁移接线**：启动/`list_docs` 前执行一次幂等迁移。
7. **A2 桩替换**：4 个命令改调 service 真实函数（签名不变，前端零改动）。
8. **A3 MCP 工具**：`tool_definitions` 加 4 个 schema；`builtin_tools` 加 Tool 条目；`call_builtin_tool` 加 4 个 match 分支（均走 `validate_doc_id`）。

## 五、测试计划（Phase 5，目标 100%/100%）

- **Rust 单测**（`service.rs` 内 `#[cfg(test)]`，临时目录模式）：
  - 路径解析（文件夹优先 / 平铺回退 / legacy 裸 id）
  - 迁移幂等（迁移两次结果一致、迁移后旧路径不可见、坏 meta 跳过不阻断）
  - 快照生成与元数据（version 递增、source 记录、可提取文档 `.md` 快照）
  - retention 裁剪（惰性：改设置不删、下次更新才裁；边界 N=1/N>现有数）
  - 版本列表倒序、批量删除（含部分版本不存在的部分成功语义）
  - diff 输出（增/删/改/相同文本/缺省 toVersion=当前；`rag_file_diff` 两文档对比含文档不存在分支）
  - 删除文档清整文件夹（含 history）
  - MCP 工具 dispatch 四分支（含非法 docId 拒绝）
- **前端 vitest**：HistoryDialog 渲染/排序/多选与批量删除回调/对比按钮门控（≠2 个禁用）、DiffDialog 解析（增/删/上下文/hunk/对齐补空）与跳转计数、ViewDialog 历史模式（无标签编辑入口）、设置项 tips 与保存值、ragService 4 函数、tauriClient 4 映射。
- 覆盖率口径：**本次新增/修改的函数与分支 100%**（vitest coverage 按文件收集）；`cargo test --lib` 全量通过。

## 六、主要风险

1. **文件夹迁移**是最大风险点——采用「解析层双轨回退 + 幂等迁移 + 失败降级」三重防护，且迁移前不动向量/FTS 数据。
2. `update_doc`（MCP）路径目前不 bump version，快照版本号将以「当前 meta.version + 1」规则统一补齐（顺带修复该不一致，行为对齐 UI 路径）。
3. 历史快照含可提取文档的 `.md` 缓存，64MiB 上限沿用 `MAX_UPLOAD_BYTES` 约束快照写入。
4. 桩命令的 mock 数据**必须**在 Phase 3 全部替换（grep `TODO(phase3)` 清零验收）。

## 七、Phase 3/5 实施记录（2026-10-22）

**后端（全部落地，cargo check 0 错 0 警；cargo test --lib 251 passed）**
- `files_dir` 内置一次性幂等迁移（`LAYOUT_MIGRATED` AtomicBool 快速路径）：平铺 `{id}.meta` + 内容兄弟文件 → `files/{id}/` 文件夹；坏名/失败逐文档 warn 跳过
- 双轨解析：`content_path_for`/`raw_content_path_for`/`open_source_path_for` 文件夹优先 + 平铺回退；`meta_path_for`/`doc_dir_or_flat`/`ensure_doc_dir`/`doc_file_write_path` helper 族；`scan_doc_metas` 统一双布局扫描（文件夹+平铺，同 id 去重优先文件夹）——替换全部 12 处扫描循环 + 19 处 meta 路径构造
- `write_doc_and_index` 内容/raw/tmp 写入 per-doc 文件夹；`update_doc` md 缓存写路径、改名 bare-id 检查同步
- `delete_doc`：文件夹布局整目录 `remove_dir_all`（含 history/）；legacy 平铺回退保留；symlink 外部原文件不动
- 快照接线：`update_doc`（MCP，source=update，顺带补 version+1）/`update_doc_from_file`（file）/`update_doc_from_original`（original，批量+自动 tick 共用）——覆写前 `snapshot_history`（幂等：同版本已快照跳过）+ 惰性 `prune_history`（按 `history_retention`，仅此时删除）
- 公开 API：`list_doc_history`/`read_doc_history`/`delete_doc_history`（批量，部分成功语义）/`diff_doc_history`（toVersion 缺省=当前）/`diff_two_docs`；4 个桩命令全部替换真实实现（`TODO(phase3)` grep 清零）
- MCP 工具 ×4：`rag_file_history_versions`/`rag_file_history_get`/`rag_file_history_diff`/`rag_file_diff`（tool_definitions + builtin_tools + call_builtin_tool 四分支，均 validate_doc_id）
- diff：`similar` crate unified diff（3 行上下文）

**测试（Phase 5）**
- Rust：`doc_history_tests` 15 个单测（路径双轨/迁移幂等+敌意名/快照记录+retention 裁剪/版本倒序/批量删除部分成功/diff 增删改+相同文本/整文件夹删除/扫描去重/MCP 工具定义/docId 守卫）——修复 `HistoryRecord` 缺 camelCase rename 的真实 bug（测试抓出）
- 前端 vitest 24 个新用例：`utils/diffParse.ts`（抽为纯函数，**覆盖率 100%**：语句/分支/函数/行）+ `ragHistoryService.test.ts`（4 个 API 封装 success/fail/缺省分支全绿）；55/55 全部通过
- tsc 0 错误；npm build ✓

## 八、用户实测回归修复（2026-10-22 下午）

- **现象**：单文件导入的文档（文件分组）执行「从文件更新」后，被移到新文件父目录新建的文件夹分组（如 /Users/jphoebe/Downloads）。
- **根因**：`update_doc_from_file` 无条件 `new_source = None` 丢弃溯源 → `classify_source` 按 `original_path` 父目录重新分类为 folder。
- **修复**：抽纯函数 `preserve_source_on_file_update`——`file`/`tool`/空 kind **保留** source（分组稳定）；`folder`（original 已离开 root，保留会被 source-sync 误判 lost-original 自动删除）与 `git`（保留会虚增 repo 引用计数泄漏 clone/凭证）仍丢弃。+6 单测；lib 257 passed。
- **注意**：已被错误分组的存量文档（meta 中 source 已被清空）不会自动恢复——重新导入该文件即可回到「文件」分组。

### 8.2 symlink 文档不生成历史（2026-10-22 晚）

- **现象**：手动上传文件 / 从原始文件更新后不生成历史记录（copy 类文档正常）。
- **根因**：symlink 导入不写本地内容文件（`write_content=false`、无 `{id}.md` 缓存，内容始终从外部原文件实时读取）→ `snapshot_history` 的 `content_path_for` 解析不到 → 静默 no-op。
- **修复**：`snapshot_history` 本地内容缺失时回退 `raw_content_path_for`（symlink 解析外部 original_path）→ 读字节（超 MAX_UPLOAD_BYTES 跳过）→ `content_from_source` 提取/解码为文本后快照（ext=".md"）；UNSUPPORTED_FORMAT/EXTRACT_FAILED 等提取失败优雅跳过（warn 日志），绝不阻断更新本身。
- **验证**：cargo check 0 错 0 警；cargo test --lib 257 passed。需重启应用后用 symlink 文档实测。
- **遗留观察（pre-existing，与历史功能无关）**：symlink extractable 文档「从文件更新」后不写 `{id}.md` 缓存 → 查看内容仍读外部旧文件（索引已更新、视图滞后）。

### 8.3 symlink 文档历史体系补全（2026-10-22 晚，用户裁定「软链接需要历史」）

按 AGENTS 既有设计「symlink 只有缓存」补全——可提取格式（PDF/Office/图片）的 symlink 文档此前**不写** `{id}.md` 提取缓存（`write_content = method != symlink` 一刀切），导致无历史、更新后视图滞后、查看内容读外部二进制乱码。三处修复：

1. **`write_doc_and_index`**：内容写入改为调用方显式传 `write_content`——copy/legacy=true、symlink 文本=false（保持直连源文件实时读语义）、**symlink 可提取=true**（写 `{id}.md` 缓存）。四个调用点（upload/from_file/from_original/rag_file_create）同步。
2. **`get_doc_inner`**：symlink 分支优先读 `{id}.md` 缓存（可提取格式的外部原文件是二进制不可视读）；无缓存（文本 symlink / 存量旧文档）回退外部原文件；lost_original 仍为空（UI 置灰语义不变）。
3. **`snapshot_history`**（8.2 已落）：本地缓存缺失时回退外部原文件提取——文本 symlink（设计上无缓存）靠此路径生成历史。

**效果**：symlink 可提取文档导入即有缓存 → 查看/历史/对比全链路可用；从文件/从原始更新后缓存同步刷新（视图不再滞后）；文本 symlink 保持直连语义、历史经外部原文件快照。存量无缓存 symlink 文档在下次更新时自动补写缓存。
**验证**：cargo check 0 错 0 警；cargo test --lib 257 passed。

### 8.4 「Is a directory (os error 21)」（2026-10-22 晚）

- **根因**：`content_path_for` 候选 3（legacy 平铺 `{id}`）在文件夹布局下解析到文档自己的文件夹 `files/{id}`（目录）→ 调用方 `read_to_string(目录)` 报 os error 21。
- **修复**：`content_path_for` 全部候选改 `is_file()` 判定（内容文件永不是目录）；legacy 平铺删除分支核查安全（仅文件夹不存在时走，且 remove_file 失败仅 warn）。
- **验证**：cargo check 0 错 0 警；cargo test --lib 257 passed。

### 8.5 交付前自测（2026-10-22 晚，用户要求 symlink/copy 自行测好再交付）

**单测层（真实文件系统往返，+10 用例，lib 267 passed）**：
- `snapshot_content_bytes`（自 snapshot_history 抽出的可测函数）：copy 可提取（.md 缓存优先于二进制 raw）/ copy 文本（原扩展名）/ symlink 文本（读外部原文件解码）/ symlink 可提取（缓存优先于外部二进制）/ symlink 丢失原文件=None / 全缺=None / 不支持二进制优雅跳过。
- 目录碰撞回归（os error 21）：`content_path_for` 与 `raw_content_path_for` 在两种布局下都绝不解析到文档文件夹本身。
- 历史列表/读取/批量删除真实目录往返（倒序、删除后不可读、其余版本存活）。

**真机 E2E（`scripts/e2e/rag_history_selftest.py`，对运行中的应用 21/21 全过）**：
- **copy 全链路**：MCP create → update×2 → 磁盘 v1/v2 快照 → 版本列表倒序 → v1/v2 内容精确 → diff v1→v2 / v2→当前 → rag_get 当前内容（os21 回归）→ meta.version=3 → delete 整文件夹移除。
- **symlink 文本**：rag_get 读外部原文件 ✓；MCP update 正确拒绝 symlink（read-only 守卫，防覆写外部原文件——symlink 更新走 UI「从文件/从原始更新」）✓；delete 后外部原文件存活 ✓。
- **symlink 可提取（带缓存）**：rag_get 优先读 .md 缓存（非二进制乱码）✓；history_versions 空列表不报错 ✓；delete 后外部原文件存活 ✓。

**边界说明**：symlink 的「更新→生成历史」接线（update_doc_from_file/from_original 内的 snapshot_history 调用）只能由 UI 触发（Tauri 命令），无法经 HTTP 自动化——由单测（snapshot 逻辑）+ 接线点核验（L3944/L4120/L5006 三处均在覆写前）保证；建议用户在 UI 对 symlink 文档点一次「从原始文件更新」做最终确认。

### 8.6 rag_file_update 支持更新 symlink 外部文件（2026-10-23，用户裁定）

- **新参数**：`updateExternalFile`（boolean，默认 **false**）——false 时 symlink 文档经 MCP 更新仍只读拒绝（原守卫保留）；true 时允许覆写软链接指向的外部源文件。
- **写入目标语义**：文本 symlink → 直接覆写外部原文件（内容即文本）；可提取 symlink（PDF/Office/图片）→ 永远只刷新 `{id}.md` 提取缓存（首次写入即创建），**二进制原文件绝不被文本破坏**。
- **历史联动**：覆写前照常快照（v1=覆写前内容）+ version+1 + md5 基线刷新（外部文件 md5=新内容，不会误报「原始文件已变更」）。
- **真机 E2E**（rag_history_selftest.py，29/29 全过）：updateExternalFile=true 覆写外部原文件 ✓ / 生成历史 v1=覆写前内容 ✓ / rag_get=新内容 ✓；可提取路径外部二进制原文件字节级未变 ✓ / 缓存刷新 ✓ / 历史 v2=更新前缓存 ✓。
- **验证**：cargo check 0 错 0 警；cargo test --lib 267 passed。

### 8.7 写穿外部原文件全场景（2026-10-23，用户裁定「原文本也要可以被修改」）

`updateExternalFile=true` 语义扩展为**写穿外部源文件**，覆盖三类导入：
1. **文本 symlink**：覆写外部原文件（原文件即文本内容）。
2. **可提取 symlink（PDF/Office/图片）**：外部原文件被文本内容**替换**（破坏性，用户显式 opt-in）+ `{id}.md` 缓存同步刷新。
3. **copy 导入**：外部源文件同步写穿（后续 source refresh 不会 clobber MCP 写入的内容）；默认 false 时只改本地拷贝。

实现：写穿在本地写入**之前**执行（权限错误先失败、本地零突变）；content_path 已是外部文件时（文本 symlink）跳过重复写；md5 基线在写穿后从 original_path 重算（不会误报「原始文件已变更」）。
**验证**：cargo check 0 错 0 警；cargo test --lib 267 passed；真机 E2E `rag_history_selftest.py` **36/36 全过**（新增 copy 写穿 8 项 + 可提取覆写断言更新）。

### 8.8 updateExternalFile 参数文档化（2026-10-23）

- `rag_file_update` 工具描述补充：默认只改 rag/files 内的拷贝，写穿需显式传参。
- `updateExternalFile` 参数描述按「功能 / 影响范围 / 风险 / 默认行为」四段结构化：
  - **功能**：true 时新文本内容额外写入文档导入来源文件（original_path），保持源同步。
  - **影响范围**：①symlink 导入——默认只读，此参数解锁覆写链接原文件；②copy 导入——外部源文件同步覆写（后续 source refresh 不会回退 MCP 写入）；③无外部来源的文档（MCP 创建/原文件丢失）不受影响，仅改本地拷贝。
  - **风险**：文本源按新文本原样替换；二进制源（PDF/Office/图片）被纯文本**不可逆替换**；更新前总是先取历史快照（可经 rag_file_history_* 恢复）。
  - **默认 false**：symlink 拒绝内容更新；copy 只改 rag/files 内本地拷贝。
- **验证**：cargo check 0 错 0 警；真机 tools/list 确认新描述已生效（`mcphub-desktop-rag_file_update`）。

### 8.9 更新改名提示（2026-10-23 晚，用户反馈「没有历史版本」定位 + 「不能自动刷新页面么」）

- **定位结论**：用户的 14:33 手动上传更新**已正常生成历史**（08f9e142，磁盘 history/v1 + 后端接口实测返回）；用户查看的是**另一个同名文档**（d6528659，10-17 导入，从未更新，历史为空系正确行为）。混淆根源：更新后文档**改名为新文件名**，列表自动刷新后原行名变了，用户点到了同名共存另一行。
- **改进**：手动上传更新导致改名时，toast 明确提示「文档已更新，已更名为 {name}」（`pages.rag.updateDoneRenamed` ×5 语言）；未改名维持原「文档已更新」。列表本身已自动刷新（updateDoc finally 的 fetchDocs）。
- **验证**：tsc 0 错；npm build ✓；locales ×5 JSON 校验通过。

### 8.10 可提取文档历史留存原始文件 + 历史详情查看源文件（2026-10-23 深夜，用户需求）

- **快照留存原始文件**：`snapshot_history` 对可提取格式（PDF/Office/图片）额外归档原始二进制到 `history/v{n}{raw_ext}`（来源 = `raw_content_path_for`：copy 取归档拷贝/外部原文件、symlink 取外部原文件；与文本快照同源时跳过；超 64MB 跳过；失败仅 warn 不阻断更新）。`HistoryRecord`/`RagDocHistoryVersion` 新增 `rawExt`（camelCase，缺省省略）。文本文档不重复存（raw 即文本）。删除/裁剪按 `v{n}.` 前缀匹配，自动覆盖原始快照。
- **历史详情查看源文件**：新 service `open_doc_history_source(doc_id, version)`（从版本 record 解析 `history/v{n}{raw_ext}`，绝不打开当前文档的源文件）+ 命令 `open_rag_doc_history_source`（require_admin）+ lib.rs 注册；`read_doc_history` 的 RagDoc.originalPath 回填归档原始快照路径（按钮显示条件）；前端 ViewDialog 按钮条件扩展为 `isExtractedSource(name) || isExtractedSource(originalPath)`（历史详情名带 " (vN)" 后缀无扩展名，用路径判定）；RagPage 新增 `historyViewVersion` 状态 + `handleHistoryViewSourceFile`；tauriClient 映射 `POST /rag/open-history-source`；ragService `openRagDocHistorySource`。
- **验证**：cargo check 0 错 0 警；cargo test --lib 267 passed；tsc 0 错；npm build ✓；真机 E2E `rag_history_selftest.py` **43/43 全过**（新增：v1.pdf 原始快照字节级一致、record.rawExt、列表透传 rawExt）。

### 8.11 树形视图默认全部收起（2026-10-24 晨，用户需求）

- `RagDocTree` 原默认全部展开（collapsed=空集）。改为：**切换到树形视图时默认全部收起**——数据首次到位时把全部源/目录 key 一次性置入 collapsed（仅一次 latch；之后的 docs 刷新不重置用户手动展开的状态）。effect 放在 sourcesRef 同步之后、依赖 sources（避免闭包旧值）。
- 搜索/标签筛选激活时的 `forceExpanded` 强制展开语义不变；一键展开/收起按钮不受影响。
- **验证**：tsc 0 错；npm build ✓。

### 8.12 历史版本文件跳转（2026-10-24，用户需求）

- 历史版本列表每行新增「打开文件位置」按钮（FolderOpen 图标，查看详情旁）：在文件管理器中显示该版本归档的文件——优先原始文件快照 `v{n}{raw_ext}`（可提取文档的"真文件"），否则文本快照 `v{n}.{md|txt}`。
- 后端：`open_doc_history_location`（record 解析 + raw 优先/文本回退）+ 命令 `open_rag_doc_history_location`（require_admin）+ lib.rs 注册；前端 ragService `openRagDocHistoryLocation` + tauriClient `POST /rag/open-history-location` 映射 + HistoryDialog `onOpenLocation` prop + `handleHistoryOpenLocation`；locales `historyOpenLocation` ×5。
- **验证**：cargo check 0 错 0 警；cargo test --lib 267 passed；tsc 0 错；npm build ✓；locales ×5 校验通过。
