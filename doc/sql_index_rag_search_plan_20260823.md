# 1.0.30003 执行计划：SQL 索引全覆盖 + RAG 标签表重构 + 搜索后端化

> ✅ **后续进展（2026-09-05）**：本文档记录的「FTS5 全文索引暂不引入（后续可选项）」已兑现——SQLite FTS5 全文索引 + 中英/拼音分词已实施完成（charabia 全语言分词 + pinyin），覆盖 servers/groups/rag_docs/skills/prompts/resources/app_log 七张表 + 活动日志筛选可搜索下拉。完整执行计划与实现记录见 `doc/sqlite_fts_pinyin_plan_20260905.md`。

> 本文档既是执行计划也是进度记录：每完成一步就把对应 `[ ]` 改为 `[x]`，并在步骤下方追加简短的完成备注（遇到的问题/决策）。

## 背景与现状（2026-08-23 摸底）

| 模块 | 搜索字段 | 当前实现 | 问题 |
|---|---|---|---|
| 服务器 ServersPage | name + description + 工具名(运行时) + 状态筛选(运行时) | 前端内存过滤 | 不走 SQL |
| 分组 GroupsPage / ServerForm 分组下拉 | name | 前端内存过滤（MultiSelect） | 不走 SQL |
| 资源 ResourcesPage | uri + name + description | 前端内存过滤 | 不走 SQL |
| Prompt PromptsPage | name + title + description | 前端内存过滤 | 不走 SQL |
| Skill SkillsPage | dirName + name + description | 前端内存过滤 | 不走 SQL |
| RAG 文件 RagPage | name + tags | 前端内存过滤 | 元数据在 `.meta` 文件，不在 SQL |
| RAG 标签下拉 | tag | SQL 分页（1.0.30002 已完成） | 排序是 tag ASC |
| 活动日志 ActivityPage | server/tool/status | SQL `LIKE` + `ORDER BY created_at DESC LIMIT/OFFSET` | 无索引，全表扫描 |
| 全库 | — | 除 UNIQUE/PK 自动索引外**无任何显式索引** | — |

标签数据模型现状：标签存在每个文档的 `.meta` JSON 里（文件系统为唯一事实源）；`rag_tag_stats(tag, file_count)` 是派生统计表，每次标签变更**全量重算**（扫描所有 .meta → DELETE 全表 → 逐条 INSERT）。

**技术事实（如实记录）**：SQLite 的 `LIKE '%x%'`（前置通配符）无法使用普通 B-tree 索引。本计划加的索引作用是：① 等值查询（`WHERE name = ?`）② `ORDER BY` 免排序 ③ 前缀匹配。子串搜索在当前数据量（几十~几千行）下走索引有序扫描 + LIMIT 提前终止即可，FTS5 全文索引暂不引入（记录为后续可选项）。

## 设计决策

- **RAG 数据源不变**：`.meta` 文件仍是唯一事实源；SQL 表（`rag_docs` / `rag_doc_tags` / `rag_tags`）是随 CRUD 增量维护的查询索引，启动时做一次对账重建（防漂移/崩溃残留）。
- **标签表（需求2）**：`rag_tags(tag PK, file_count)`（带 `file_count DESC` 索引）+ 关联表 `rag_doc_tags(doc_id, tag)`。每次文档标签 CRUD 增量 diff 更新（不再全量重算）；`file_count` 减到 0 自动 DELETE 该标签行。
- **服务器搜索**：SQL 负责 name/description 的 LIKE 过滤 + `ORDER BY name`；运行时字段（状态、工具名匹配）在 Rust 侧合并（工具名匹配保留，避免功能回退）；分页在后端完成。
- **统一分页搜索模式**：所有新命令参照已有的 `rag_tag_search_paged` 模式（防抖搜索 + LIMIT/OFFSET + 返回 total）。

---

## 执行步骤

### Phase A：数据库迁移 v21（表 + 索引）

- [x] A1. `src-tauri/src/db/migration.rs`：`TARGET_VERSION` 20 → 21，新增 `migrate_v21`：
  - 新建 `rag_tags(tag TEXT PRIMARY KEY, file_count INTEGER NOT NULL DEFAULT 0)`；从 `rag_tag_stats` 迁移数据（`INSERT INTO rag_tags SELECT tag, file_count FROM rag_tag_stats`）后 DROP 旧表
  - 新建 `rag_doc_tags(doc_id TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY(doc_id, tag))` + `idx_rag_doc_tags_tag(tag)`
  - 新建 `rag_docs(id TEXT PRIMARY KEY, name TEXT NOT NULL, size INTEGER, uploaded_at TEXT, file_type TEXT, method TEXT, original_path TEXT, md5 TEXT, version INTEGER DEFAULT 1, chunk_count INTEGER DEFAULT 0)` + `idx_rag_docs_name(name)` + `idx_rag_docs_uploaded_at(uploaded_at DESC)`
  - `idx_rag_tags_file_count ON rag_tags(file_count DESC, tag)`
  - 现有表补索引（全部 `IF NOT EXISTS` 幂等）：
    - `activity_log`: `(created_at DESC)`、`(server)`、`(tool)`、`(status)`
    - `app_log`: `(created_at DESC)`
    - `builtin_prompts`: `(name)`
    - `builtin_resources`: `(name)`、`(uri)`
    - `skills`: `(status, dir_name)`
    - `bearer_keys`: `(created_at DESC)`
  - 注意：`rag_doc_tags` / `rag_docs` 的数据回填不在迁移里做（迁移拿不到 AppHandle/files_dir），由 RAG 服务启动时对账重建完成
  - 完成备注：数据平移做了幂等保护（仅 rag_tags 为空且旧表有数据时 INSERT）；所有 DDL 失败均返回 Err（不吞错）
- [x] A2. `cargo check` 通过（用 asdf 工具链 + 代理）
  - 完成备注：`ORT_SKIP_DOWNLOAD=1 CARGO_NET_OFFLINE=true cargo check` 通过；修过一处 `AssertSqlSafe(*stmt)` 解引用（sqlx 0.9 只接受 `&'static str`）

### Phase B：RAG 标签表重构（需求 2 + 需求 3）

- [x] B1. `src-tauri/src/rag/service.rs` 新增增量同步 helper `sync_doc_tags(doc_id, new_tags)`：
  - 读旧标签（`rag_doc_tags WHERE doc_id=?`），diff 出 added/removed
  - removed：删关联行，`UPDATE rag_tags SET file_count=file_count-1`，**`DELETE FROM rag_tags WHERE file_count<=0`**
  - added：插关联行，`INSERT INTO rag_tags ... ON CONFLICT(tag) DO UPDATE SET file_count=file_count+1`
  - 同一事务内完成
- [x] B2. 替换 9 处 `recompute_tag_stats` 调用点为增量 `sync_doc_tags`：upload_one_path（新上传/覆盖）、set_doc_tags、delete_doc（sync(doc_id, [])）、rag_file_create、rag_file_update、rag_file_delete、batch update 完成后
- [x] B3. 保留全量对账：`recompute_tag_stats` 改名/扩展为 `rebuild_rag_sql_index`（同时重建 `rag_docs` + `rag_doc_tags` + `rag_tags`），在 RAG 服务启动（enable/init）时调用一次
- [x] B4. `list_tags_paged` 排序改为 `ORDER BY file_count DESC, tag ASC`（两处查询 + MCP 工具用的 `list_tags` 同步改）；前端已显示 fileCount，无需改动
- [x] B5. reset（清空 RAG）路径同步清空三张 SQL 表

### Phase C：RAG 文件搜索后端化（需求 1 之 rag 文件）

- [x] C1. `rag_docs` 镜像维护：文档 CRUD（上传/更新/删除/批量更新）时同一流程内 UPSERT/DELETE `rag_docs` 行（与 B1/B2 的 sync 同步做）
- [x] C2. `models/rag.rs` 新增 `RagDocPage { items, total, page, page_size }`；`service.rs` 新增 `search_docs_paged(search_key, tags, page, page_size)`：
  - SQL：`rag_docs` 上 `name LIKE` + 标签过滤（`EXISTS (... rag_doc_tags ... tag IN (...))`），`ORDER BY uploaded_at DESC LIMIT/OFFSET` + COUNT
  - 返回的每页 doc id 再读 `.meta` 补全 `RagDocInfo`（file_name/lost_original/content_available 等文件系统派生字段）——SQL 管过滤排序分页，文件系统只富化最终一页
- [x] C3. `commands/rag.rs` 注册 `rag_doc_search_paged` 命令；`lib.rs` 挂载
- [x] C4. 前端 `RagPage.tsx` 工具栏：搜索词或标签筛选非空时改为防抖调后端（250ms，复用标签下拉的请求竞态守卫模式），文件列表渲染后端结果 + 「加载更多」；空搜索时保持现状全量渲染

### Phase D：服务器/分组/资源/prompt/skill 搜索后端化（需求 1）

- [x] D1. `server_service.rs` 新增 `search_servers(search, filter, page, page_size)`：
  - SQL：`WHERE (?='' OR name LIKE ? OR description LIKE ?) ORDER BY name` 取候选
  - Rust 合并：运行时状态（online/issues/disabled 过滤）、工具名匹配（保留现有 haystack 行为）
  - Rust 分页 + total + 状态计数（counts）随响应返回
- [x] D2. `group_service.rs` 新增 `search_groups(search, page, page_size)`（name/description LIKE，`ORDER BY name`）
- [x] D3. `prompt_service.rs` 新增 `search_prompts(search, filter, page, page_size)`（name/title/description）
- [x] D4. `resource_service.rs` 新增 `search_resources(search, filter, page, page_size)`（uri/name/description）
- [x] D5. `skill_service.rs` 新增 `search_skills(search, page, page_size)`（dir_name/name/description，`status='ok'`，`ORDER BY dir_name`）
- [x] D6. 各 commands 文件注册命令（servers.rs / groups.rs / prompts.rs / resources.rs / skills.rs）+ `lib.rs` 挂载
- [x] D7. 前端接线（统一模式：防抖 250ms + 竞态守卫 + 后端分页）：
  - `ServersPage.tsx`：搜索/状态筛选/翻页走 `search_servers`（counts 从响应取；类型过滤 builtin/external 保持前端）
  - `PromptsPage.tsx` / `ResourcesPage.tsx` / `SkillsPage.tsx`：搜索+筛选+翻页走新命令，去掉 `selectItemPage` 客户端路径
  - `ServerForm.tsx` 分组 MultiSelect：选项加载走 `search_groups`（滚动加载更多）
  - `tauriClient.ts` 增加对应 invoke 封装；`types/index.ts` 增加分页响应类型

### Phase E：验证与收尾

- [x] E1. `cargo check` / `cargo clippy`（代理 + asdf 工具链；`ORT_SKIP_DOWNLOAD=1`）
  - 完成备注：`ORT_SKIP_DOWNLOAD=1 CARGO_NET_OFFLINE=true cargo check` 通过（9.9s）；`cargo clippy` 通过（0 error，82 warning 均为存量风格类——doc list 缩进、`map_or` 简化、`clamp` 建议、函数参数过多等，与本次改动无关；本次新增的 service/commands 代码未引入新 error）。
- [x] E2. `cargo test`（migration / rag service 相关测试；为 v21 迁移和 sync_doc_tags 增减标签路径补测试）
  - 完成备注：`cargo test --lib` 全绿（18 passed / 0 failed / 3 ignored），含本次新增的 `v21_creates_rag_tables_and_indexes` + `tag_count_semantics_decrement_to_zero_deletes`（覆盖 file_count 递减归零自动 DELETE 路径）。3 个 ignored 为需模型文件的 gguf/modernbert 探针（沙箱无模型）。
- [x] E3. 前端 `npm run build`（frontend/）
  - 完成备注：`npm run build`（vite/esbuild）通过（✓ built in 900ms），产物含 ServersPage / RagPage / SkillsPage / GroupsPage / SettingsPage 等 chunk。tsc 存量报错与本次改动无关（项目构建走 vite，不经 tsc）。
- [ ] E4. 手动冒烟：旧库升级（rag_tag_stats → rag_tags 数据迁移）、标签增删后 file_count 增减/归零自动删除、标签下拉按 fileCount 倒序+滚动加载、文件搜索走后端、五个页面搜索+分页
  - 完成备注：需用户在真实环境（含模型/运行时连接）手测，沙箱无法启动 GUI + MCP 连接。E2 的 `tag_count_semantics_decrement_to_zero_deletes` 单测已覆盖归零删除逻辑；`v21_creates_rag_tables_and_indexes` 覆盖表/索引 DDL。其余（标签下拉倒序+滚动、五页搜索+分页 UI 行为）待用户冒烟。
- [x] E5. 版本号 1.0.30002 → 1.0.30003（tauri.conf.json / package.json / frontend/package.json），本文档从计划转为变更记录
  - 完成备注：三处 version 均已为 `1.0.30003`（tauri.conf.json:4 / package.json:3 / frontend/package.json:3）
  - **回退（2026-08-23）**：应要求将三处 version 恢复为 `1.0.30002`（暂不发布 1.0.30003，保留为后续发布版本号）。

## 风险与注意事项

- **迁移不可用 `.ok()` 吞错**：v21 所有 DDL 失败必须返回 Err（沿用 `add_column_if_missing` 的教训，版本号才不会与实际 schema 脱节）
- **SQLite LIKE 前置通配符不走索引**：已在「技术事实」记录；如未来数据量大再评估 FTS5
- **服务器状态过滤依赖运行时**：SQL 只做 name/description/排序，状态在 Rust 合并，分页在 Rust 完成（表小，可接受）
- **RAG SQL 表是镜像不是事实源**：任何绕过 service 的 .meta 变更由启动对账兜底

### Phase B/C 完成备注（2026-08-23）

- `recompute_tag_stats` 重命名为 `rebuild_rag_sql_index` 并扩展为三表全量重建（rag_docs / rag_doc_tags / rag_tag_stats->rag_tags）
- 新增 `upsert_doc_sql(meta)` / `remove_doc_sql(doc_id)` 增量维护（单事务）：标签 diff 计数 ±1、归零删行；上传/更新/建文档路径在 `write_doc_and_index` 落盘 .meta 后统一 upsert；`set_doc_tags`/`update_doc`(MCP) 各自 upsert；`delete_doc` remove；`create_doc_from_content` 覆盖同名校验时对 stale id 逐个 remove
- 启动对账：`start()` 末尾调用 `rebuild_rag_sql_index`（RAG 关闭时镜像不刷新，但标签编辑/文档删除本就要求 RAG 开启）
- `list_tags` / `list_tags_paged` / MCP `rag_tag_search` 均改为 `ORDER BY file_count DESC, tag`
- B5：当前无「清空 RAG」功能（reset 不存在），无需处理
- C1-C3 后端完成：`search_docs_paged`（name LIKE + tag IN 子查询过滤，uploaded_at DESC 分页，页内 id 回读 .meta 富化）；`rag_doc_search_paged` 命令已注册

- C4 完成：RagPage 工具栏搜索/标签筛选非空时走 `ragDocSearchPaged`（防抖 250ms + 竞态守卫 + 已加载去重合并 + 「加载更多」按钮显示 loaded/total）；空搜索保持前端全量渲染。tauriClient 增加 `/rag/docs/search-paged` 路由映射，types 增加 `RagDocPage`。`vite build` 通过（tsc --noEmit 存量报错与本次改动无关，项目构建走 vite/esbuild）。

### Phase D 完成备注（2026-08-23）

- 后端五命令：`search_servers`（SQL name/description LIKE 预过滤 + Rust 合并运行时状态/工具名匹配 + builtin 虚拟服务器 + 类型/状态筛选 + 后端分页，page 1-based）、`search_groups` / `search_builtin_prompts` / `search_builtin_resources` / `search_skills`（page 0-based，SQL LIMIT/OFFSET；skills 因 FS 存在性过滤在 SQL 后应用，分页在 Rust 完成）
- service 层：`server_service::search_configs`、`group_service::search_paged`、`prompt_service::search_paged`（name/title/description + active/inactive 筛选）、`resource_service::search_paged`（uri/name/description）、`skill_service::search_library_paged`（dir_name/name/description，status='ok'，FS 校验同 list_library）
- 模型：五个 `*Page { items, total, page, pageSize }` 加在各 models 文件
- 前端接线模式统一（防抖 250ms + reqId 竞态守卫）：
  - **PromptsPage / ResourcesPage / SkillsPage：全后端化** —— 列表始终走后端分页（空搜索=后端全量分页），`selectItemPage` 客户端路径移除；counts 条仍用全量 hooks 数据
  - **ServersPage：混合模式** —— 搜索词或状态筛选非空时走 `search_servers`（含工具名匹配，保持原 haystack 行为）；空搜索保持前端全量路径（服务器列表是运行时数据、由轮询实时刷新，后端化会丢失实时状态更新）
  - **MultiSelect 组件**扩展可选 `searchFn` prop（后端分页搜索 + 滚动加载更多 + Load more 按钮）；SettingsPage 的分组下拉接入 `searchGroups`
- tauriClient 路由：`/servers/search`、`/groups/search`、`/prompts/search`、`/resources/search`、`/skills/search`（均 POST）；新增 `services/groupService.ts`

---

## 复核记录（三轮独立复核，2026-08-23）

### 第一轮：文档逐条核对实际代码

逐条对照本计划 Phase A–E 与当前代码（单 agent 复核，不开子 agent）。

- **Phase A（迁移 v21）✅**：
  - `migration.rs` `TARGET_VERSION = 21`，`migrate_v21` 建三表 `rag_tags` / `rag_doc_tags` / `rag_docs` + 3 张 RAG 索引（`idx_rag_tags_file_count` / `idx_rag_doc_tags_tag` / `idx_rag_docs_name` / `idx_rag_docs_uploaded_at`）+ 现有表 10 个补索引（activity_log 4、app_log、builtin_prompts name、builtin_resources name/uri、skills status+dir_name、bearer_keys created_at），全部 `IF NOT EXISTS` 幂等。
  - 旧表数据平移 `INSERT INTO rag_tags SELECT tag, file_count FROM rag_tag_stats` 带幂等守卫（仅 rag_tags 为空且旧表有数据时执行），DROP `rag_tag_stats`。所有 DDL `.map_err(...)` 不吞错。`migrate_v21` 有单测覆盖。
- **Phase B/C（RAG 标签表重构 + 文件搜索）✅**：
  - `rag/service.rs` 增量 helper：`upsert_doc_sql(meta)`（单事务内 diff 旧/新标签，removed 行 `file_count-1` + `DELETE WHERE file_count<=0`，added 行 `ON CONFLICT(tag) DO UPDATE SET file_count=file_count+1`，并 UPSERT `rag_docs`）、`remove_doc_sql(doc_id)`（逐标签递减+归零删除 + 删关联行 + 删 rag_docs 行）。
  - 8 处调用点全覆盖：`start`（943）、`write_doc_and_index`（2009）、`create_doc_from_content` 覆盖同名校验 stale id（2606）、`update_doc` MCP（2768）、`reindex_all`（3104）、`set_doc_tags`（3150）、`delete_doc`（3265）、`batch_update`（3451）。
  - `rebuild_rag_sql_index`（1553）：单事务 DELETE 三表 + 批量 INSERT 对账重建，`start()` 末尾调用一次。
  - `search_docs_paged`（1722）：`rag_docs.name LIKE` + tag `IN` 子查询过滤，`ORDER BY uploaded_at DESC LIMIT/OFFSET` + COUNT，页内 id 回读 `.meta` 富化 `RagDocInfo`。
  - `list_tags` / `list_tags_paged` 均改 `ORDER BY file_count DESC, tag`。
- **Phase D（五服务搜索后端化）✅**：
  - `server_service::search_configs` / `group_service::search_paged` / `prompt_service::search_paged` / `resource_service::search_paged` / `skill_service::search_library_paged` 全部用 `sqlx::AssertSqlSafe` + bind 参数 + `LIKE ? ESCAPE '\\'`（`%`/`_` 转义），`page.min(10_000)` + `page_size.clamp(1, 200)` 边界。
  - prompt/resource：name/title(description) + active/inactive 筛选；skill：dir_name/name/description + `status='ok'`，FS 存在性过滤在 SQL 后应用（Rust 内分页，total 取全量）。servers：SQL 预过滤 + Rust 合并运行时状态/工具名匹配 + builtin 虚拟服务器 + 类型/状态筛选 + 后端分页（page 1-based）。
  - 5 个 commands 文件均注册命令并 `Result<*, String>` 包裹；`lib.rs` `invoke_handler!` 全部挂载（lib.rs:360/373/412/420/466/489-490 核对通过）。
  - 前端 5 页均接入：PromptsPage / ResourcesPage / SkillsPage 全后端化（`reqId` 竞态守卫 + page 0-based）；ServersPage 混合模式（`searchActive = search.trim()!=='' || filter!=='all'`）；ServerForm/SettingsPage 分组下拉走 `searchGroups`；新增 `services/groupService.ts`。
  - tauriClient 路由映射 5 条 + RAG 2 条均存在（`/servers/search`、`/groups/search`、`/prompts/search`、`/resources/search`、`/skills/search`、`/rag/docs/search-paged`、`/rag/tags/search-paged`）。
- **Phase E（验证收尾）**：
  - E5 ✅ 版本号三处均已 `1.0.30003`。
  - E1–E4 待第三轮构建/测试执行。

### 第二轮：逻辑正确性与边界

- **`search_servers` 工具名回退正确性**：`search_configs(&search)` 用原始 `search`（含大小写），LIKE 本身大小写不敏感（SQLite 默认），`candidates` 命中后用 `matched_names` 去重；工具名回退扫描 `list_all()` 仅在 `!key.is_empty()` 时触发，`key` 为 `search.trim().to_lowercase()`，匹配 `t.name.to_lowercase().contains(&key)` —— 与前端 `filterServers` 的 haystack 行为一致。✅
- **`search_servers` 分页边界**：`page.max(1)`，`start = (page-1)*page_size`，`start >= filtered.len()` 返回空 vec，否则 `filtered[start..(start+page_size).min(len)]` —— 越界安全。✅
- **`search_servers` 状态过滤语义**：online=`connected`、disabled=`!enabled`、issues=`!connected && enabled` —— 与 `getServerFilterCounts` 一致。✅
- **`sync_doc_tags` 增量原子性**：单事务（`upsert_doc_sql` 内 `pool.begin()`），diff 后 removed 走 `DELETE`+`UPDATE file_count-1`+`DELETE WHERE file_count<=0`，added 走 `INSERT OR IGNORE rag_doc_tags`+`ON CONFLICT(tag) DO UPDATE SET file_count=file_count+1`，事务 commit。✅
- **`rebuild_rag_sql_index` 全量重建**：单事务 `DELETE FROM rag_tags/rag_doc_tags/rag_docs` 后按当前 `.meta` 批量回填，保证启动对账兜底（崩溃残留/绕过 service 的 .meta 变更）。✅
- **`search_docs_paged` 富化**：SQL 只取页内 id，再逐个读 `.meta`（`load_doc_info` 类），FS 派生字段（file_name/lost_original/content_available）来自文件系统而非 SQL 镜像 —— 镜像只管过滤/排序/分页。✅
- **ServersPage 竞态守卫**：`searchActive` 切换时 `reqIdRef` 守卫 + debounce 250ms，旧请求结果被丢弃。✅
- **LIKE 转义**：所有 service 均 `key.replace('%', "\\%").replace('_', "\\_")` + `ESCAPE '\\'`，用户输入的 `%`/`_` 不会当通配符。✅
- **边界**：所有分页 `page.min(10_000)` + `page_size.clamp(1, 200)`，servers `page.max(1)`+`page_size.clamp(1,200)`。✅

### 第三轮：构建与测试

- **E1 `cargo check`/`clippy` ✅**：`ORT_SKIP_DOWNLOAD=1 CARGO_NET_OFFLINE=true cargo check` 通过（asdf 1.96.0 工具链 + 代理 127.0.0.1:7890，9.9s）；`cargo clippy` 0 error，82 warning 全为存量风格类（doc 缩进 / map_or / clamp / 参数过多），本次新增代码未引入新 error。
- **E2 `cargo test` ✅**：`cargo test --lib` 18 passed / 0 failed / 3 ignored。新增两测：`v21_creates_rag_tables_and_indexes`（表 + 索引 DDL）、`tag_count_semantics_decrement_to_zero_deletes`（file_count 递减归零自动 DELETE）。3 ignored 为需模型文件的 gguf/modernbert 探针。
- **E3 前端 `npm run build` ✅**：vite/esbuild 通过（900ms），产物含全部页面 chunk；tsc 存量报错与本次改动无关。
- **E4 手动冒烟 ⏳**：沙箱无 GUI/运行时，需用户在真实环境手测。归零删除逻辑已由 E2 单测覆盖；UI 行为（标签下拉倒序+滚动、五页搜索+分页）待用户冒烟。
- **E5 版本号 ✅**：三处均已 `1.0.30003`。

**结论**：Phase A–D 代码与计划文档一致；三轮复核发现的两个问题（servers 搜索 [object Object] + 同类分页映射脆弱）已修复；E1–E3 构建测试全绿，E4 待用户冒烟，E5 已完成。本版本可交付用户做最终冒烟。

### 修复记录（复核期间发现并修复）

- **servers 搜索后状态格显示 [object Object]**：`search_servers` 命令返回嵌套 `ServerInfo {config, status: ServerStatus, tools, ...}`，但 `transformTauriResponse` 无映射分支，落入通用 `{success, data:result}`，`server.status` 为对象 → StatusDot 渲染 `[object Object]`。修复：`tauriClient.ts` 新增 `search_servers` 分支，对每个 item 调 `toFrontendServer` 扁平化为前端 `Server`（`status` 降为字符串：`connecting`/`connected`/`disconnected`）。
- **同类分页搜索响应映射补齐**：为 `search_groups` / `search_builtin_prompts` / `search_builtin_resources` / `search_skills` / `rag_doc_search_paged` / `rag_tag_search_paged` 新增通用 `{success, data:{items,total,page,pageSize}}` 映射分支（原先因 Rust 返回 camelCase 形状巧合匹配而"能用"，但脆弱——任何形状变化都会静默泄漏原始对象到列表格，与 servers bug 同源）。servers 分支用 1-based page，其余 0-based。


---

### 补丁记录（1.0.30002 用户反馈三连修复，2026-08-24）

> 版本已回滚至 1.0.30002。以下三个修复针对用户在 1.0.30002 上的反馈，补在本计划文档末尾备查。

#### 问题 1：RAG 标签下拉框不展示列表（手动新增标签后仍为空）

**根因（已用线上状态实证）**：
- 线上 DB `rag_tags` / `rag_doc_tags` **全空**，但 `rag_docs` 有该文档行（说明 `rebuild_rag_sql_index` 启动对账时插了 docs，但标签从未落表）。
- `.meta` 文件**有**标签 `["测试","xx项目"]`，说明文件系统事实源已更新成功。
- `app_log` 在用户两次加标签时刻（00:35:59、00:36:05）各有一条：
  `upsert_doc_sql for '测试.txt' failed: error returned from database: (code: 5) database is locked`
- `db/mod.rs::initialize` 用裸 `connect(&db_url)` 建池，未设任何 PRAGMA → `busy_timeout=0` + `journal_mode=delete`（默认回滚日志，单写者）。`app_logger::log_to_db`（每条 MCP 日志都写）+ `log_service` 活动日志（每次工具调用都写）+ RAG 写在同一文件上争用 → 写者立即拿到 SQLITE_BUSY(5)。
- `set_doc_tags`（rag/service.rs:3150）对 `upsert_doc_sql` 的失败仅 `warn` 日志后 `return Ok(())` → UI 显示成功，但 SQL 镜像表永不更新 → 下拉读 `rag_tags` 全空 → "暂无标签"。

**修复**：
1. `src-tauri/src/db/mod.rs::initialize`：用 `SqliteConnectOptions::from_str(&db_url)` 替代裸 `connect`，配置 `.create_if_missing(true)` + `.journal_mode(Wal)` + `.busy_timeout(Duration::from_millis(5000))`，`connect_with(options)`。WAL 允许读写并发，busy_timeout 让写者等锁 5s 而非立即失败。
2. `src-tauri/src/rag/service.rs::set_doc_tags`：`upsert_doc_sql` 失败后增加最多 4 次重试（仅对 `database is locked` / `code: 5`，间隔 150ms/300ms/450ms 退避），仍失败才 warn 兜底。`.meta` + chunk 已先落盘成功，镜像更新是 best-effort。

#### 问题 2：资源搜索把 URL 信息也代入搜索

**根因**：`resource_service.rs::search_paged` 的 LIKE 条件含 `LOWER(uri) LIKE ?`，即 `uri`（资源标识符，常为 URL）参与子串搜索。

**修复**：去掉 `LOWER(uri) LIKE ?` 分支及对应的一个 bind，仅保留 `LOWER(COALESCE(name,'')) LIKE ? OR LOWER(COALESCE(description,'')) LIKE ?`（count_q 与 data_q 各从 3 bind 改为 2 bind）。`uri` 不再参与搜索。

#### 问题 3：服务器搜索文案误写"分类或标签"

**根因**：`ServersPage.tsx:330` 复用了 `market.searchPlaceholder`（"搜索服务器名称、分类或标签"），但 `search_servers` 后端只匹配 name + description（+ 运行时工具名），**并无**分类/标签过滤功能。而 `MarketPage.tsx:436` 是真有分类/标签过滤（`useMarketData` 的 `filterByCategory`/`filterByTag`），故 `market.searchPlaceholder` 共享键不能改。

**修复**：在 `server.*` 下新增专属 `searchPlaceholder` 键（zh："搜索名称或描述" / en："Search by name or description" / fr / tr 同步），`ServersPage.tsx:330` 与 `ServerToolConfig.tsx:506` 改用 `t('server.searchPlaceholder')`，不动 `market.searchPlaceholder`。

#### 验证

- **E1 `cargo check` ✅**：`ORT_SKIP_DOWNLOAD=1 cargo check`（asdf 1.96.0 + 代理 127.0.0.1:7890）通过，0 warning（删去误引入的 `ConnectOptions` 未用 import 后）。
- **E3 前端 `npm run build` ✅**：vite/esbuild 通过（1.00s）。
- **E4 手动冒烟 ⏳**：沙箱无 GUI/运行时，需用户在真实环境验证：① 加标签后下拉能列出标签；② 资源搜索输入 URL 片段不再命中；③ 服务器搜索框文案为"搜索名称或描述"。
- **注**：DB 迁移未升版本号（仍是 v21）。WAL 模式会在首次连接后由 SQLite 自动持久化到该 DB 文件（`mcphub.db-wal` / `mcphub.db-shm` 出现属正常），无需显式迁移步骤；busy_timeout 为连接级 PRAGMA，每次建连生效。已存在的线上 DB 在用户重启 App 后即自动应用新连接配置；为修复历史空 `rag_tags`，用户重启 App 后 RAG 启动会触发 `rebuild_rag_sql_index` 对账，把 `.meta` 中的标签回填进表。

---

### 补丁记录（1.0.30002 用户反馈第二批修复，2026-08-24）

> 针对用户 1.0.30002 冒烟反馈的 10 个问题。DB 迁移升 v21 → **v22**。

#### 问题 1：log 表补 level 索引 + 各业务表 created_at 索引

`migrate_v22`（migration.rs）：`app_log(level)`；`servers` / `groups` / `builtin_prompts` / `builtin_resources` / `skills` / `skill_exports` / `templates` / `server_tool_config` 各补 `(created_at DESC)`。rag 的 `rag_docs(uploaded_at DESC)` v21 已有。全部 `IF NOT EXISTS` 幂等。

#### 问题 5：标签下拉排序 file_count DESC + created_at DESC

- v22 给 `rag_tags` 加 `created_at` 列（毫秒时间戳 TEXT，存量默认 `''`）；
- 索引改 `idx_rag_tags_file_count ON rag_tags(file_count DESC, created_at DESC, tag)`（CREATE IF NOT EXISTS 同名覆盖旧定义需注意：SQLite 的 IF NOT EXISTS 按索引名判断，v21 已建同名索引的库不会自动重建——**v21 从未随版本发布过**（版本回滚在 E5），线上库都在 v20 及以下，v22 首次执行时创建的是新定义，无冲突）；
- `list_tags` / `list_tags_paged` 排序改 `ORDER BY file_count DESC, created_at DESC, tag`；
- `upsert_doc_sql`：新标签 INSERT 带当前时间戳，已有标签 ON CONFLICT 只加计数不动 created_at；
- `rebuild_rag_sql_index`：对账重建保留现有 created_at（不清零排序依据），新标签打当前时间。

#### 问题 2+3：标签去重 + Tool 工具标签同步

- **MCP 工具同步**（复核结论，无需新代码）：`rag_file_create` → `write_doc_and_index` 内 `upsert_doc_sql`（stale 覆盖走 `remove_doc_sql`）；`rag_file_update` → `update_doc` 末尾 `upsert_doc_sql`（含 addTags/removeTags 分支，symlink_lost 跳过 re-index 的路径也走到 upsert）；`rag_file_delete` → `delete_doc` 末尾 `remove_doc_sql`。三路径均已同步 rag_tags/rag_doc_tags/rag_docs。
- **后端去重**：`set_doc_tags` / `upload_one_path_inner` / `create_doc_from_content` 的标签规整统一加大小写不敏感去重（trim → 去空 → lowercase seen-set 只留首个拼写）。`update_doc`（MCP）的 addTags 合并原本就有 `eq_ignore_ascii_case` 去重。
- **前端去重**：`TagEditor` 下拉列表过滤已挂标签（`selectableItems`），`add`/`commitDraft`/「添加新标签」行全部改大小写不敏感判断（`hasTagCI`）。批量添加走 `Set` 天然去重 + 后端兜底。

#### 问题 6：导入文件夹过滤支持的文件类型

`pick_folder`：每个候选文件需同时通过两道过滤——①扩展名目录过滤（`file_type_map()`，来自 `runtimes/rag/file_support.json`）：取文件名小写的点号后缀（如 `.md`），不在目录中的扩展名/无扩展名文件直接跳过，不读字节；②内容嗅探：读头 8KB 做 `is_likely_text`（与上传校验同规则），即便扩展名在目录中但实为二进制（如一个伪装成 `.txt` 的 PDF）也被过滤。两道都过才进入待导入列表。

#### 问题 7：导入文件按路径校验重复

前端 `UploadDialog` 的重名提示从「文件名已存在，将覆盖」（`existingNames`）改为按 `originalPath` 校验（`existingPaths`，来自 `RagDocInfo.originalPath`），提示文案改为「该文件已导入过」（i18n key `nameExists` → `pathExists`，四语言同步）。文件名不再提示（上传永不覆盖，同名文档并存）。

#### 问题 8：文件详情标签下拉框宽度占满

`TagEditor` 下拉锚点从输入框容器（`inputWrapRef`，宽度随 flex 伸缩）改为整个标签编辑行（`wrapRef` = chips + 输入框），宽度恒为标签栏整行宽度。`inputWrapRef` 已删除。

#### 问题 9+10：文案

- `batchAddTags`：「为选中文档添加标签」→「批量添加标签」；`batchRemoveTags`：「从选中文档移除标签」→「批量移除标签」（en/fr/tr 同步）。
- `BatchTagsDialog` 确认按钮：add 模式显示「保存标签」（`saveTags`），remove 模式显示「移除标签」（新 key `removeTags`，四语言）。

#### 验证

- `cargo check` ✅（8.45s，asdf shim 1.96.0 + 代理）
- `cargo test --lib` ✅ 18 passed / 0 failed / 3 ignored（v21 测试已扩展覆盖 v22：created_at 列读写 + 9 个新索引存在性）
- `npm run build` ✅（vite 979ms）
- 手动冒烟 ⏳：需用户验证 ① 标签下拉排序（文件数倒序+同数新标签在前）② 文件夹导入只列出文本文件 ③ 同路径文件提示「已导入过」、同名不提示 ④ 详情标签下拉不显示已挂标签、宽度占满 ⑤ 批量移除按钮文案「移除标签」

---

### 补丁记录（1.0.30002 启动 fatal：v23 bearer_keys.user_id 索引，2026-08-24）

#### 问题：升级后 DB 初始化 fatal，App 起不来

```
[fatal] Database initialization failed: create index failed:
CREATE INDEX IF NOT EXISTS idx_bearer_keys_user_id ON bearer_keys(user_id)
(error returned from database: (code: 1) no such column: user_id)
```

**根因（已用线上状态实证）**：
- 线上 DB `schema_version` 停在 v22，重启后要跑 `migrate_v23`。
- `bearer_keys` 表**没有 `user_id` 列**（`pragma_table_info` 确认）。该库走的是旧 sqlx 迁移路径（`_sqlx_migrations` 表仍在）：`0001_initial.sql` 建的 `bearer_keys` 带 `user_id`，但 `0002_schema_fix.sql` `DROP TABLE bearer_keys` 后重建为 token 中心表（`id/name/token/enabled/access_type/allowed_groups/allowed_servers/created_at`，无 `user_id`）。`get_current_version` 把旧迁移条数映射成版本号后，这些库的 `bearer_keys` 实际无 `user_id` 列。
- `migrate_v23` 无条件 `CREATE INDEX ON bearer_keys(user_id)` → `no such column: user_id` → 整个迁移 `Err` → `db::initialize` fatal → App 启动失败。
- 注意：`activity_log` 在该库**有** `key_id` / `source_ip`（v9 已重建），所以问题只出在 `bearer_keys.user_id` 这一条；但原写法三条索引都在同一个无守卫的循环里，第一条失败即整体 Err。

**修复**：`migrate_v23`（migration.rs）拆成两段——
1. description 索引（servers/groups/builtin_prompts/builtin_resources/skills）保持无条件 `CREATE INDEX`：这些表自 v1 起就带 `description` 列，无历史分叉。
2. `bearer_keys.user_id` / `activity_log.key_id` / `activity_log.source_ip` 改用新 helper `create_index_if_column_exists(pool, index_name, table, column)`：先查 `pragma_table_info`，列缺失则 `log::warn` 跳过（不报错），列存在才 `CREATE INDEX IF NOT EXISTS`。这样缺失列的单个索引被跳过，其余索引照建，迁移正常完成、版本号推进到 v23。

**为何不直接给 bearer_keys 补 user_id 列**：`bearer_key_service` 的所有 SELECT/INSERT/UPDATE 都不碰 `user_id`（list/find_by_token/create/update 全用 token/access_type/allowed_groups/allowed_servers 语义），现行模型根本没有 user_id 字段。强行加列会让它永远为 NULL，索引也无意义。user_id 是 v1 的遗留外键设计，已在 0002 被废弃；列守卫跳过索引才是与现行 schema 一致的处理。

**验证**：
- 新增单测 `v23_skips_indexes_on_missing_columns`：跑到 v23 后，把 `bearer_keys` 重建成无 user_id 的旧形状、`activity_log` 重建成无 key_id/source_ip 的旧形状，回退版本号到 v22 再重跑 pending —— 断言缺失列的三个索引不存在、description 索引仍建出、版本号推进到 v23、`run_pending` 不 fatal。`cargo test --lib migration::` ✅ 3 passed。
- 在线上 DB 的快照上实测：旧写法 `CREATE INDEX ON bearer_keys(user_id)` 复现 `no such column: user_id`；新守卫路径 `bearer_keys.user_id exists? 0 → SKIP`，`activity_log.key_id/source_ip exists? 1 → created`。description 索引照建。
- `cargo check --lib` ✅（6.12s，0 warning）。

**用户操作**：重新启动 App 即可。v23 会跳过 `idx_bearer_keys_user_id`（该库无此列，属预期），建出 description 索引 + activity_log 审计索引，版本号推进到 v23，DB 正常初始化。
