# SQLite 全文索引（FTS5）+ 中英/拼音分词 执行计划

> 2026-09-05 起草。本文档既是执行计划也是进度记录：每完成一步把对应 `[ ]` 改为 `[x]`，并在步骤下方追加简短完成备注（问题/决策）。执行完后按惯例同步 `agent.md` 第 3 节。

## 0. 一句话

把 2026-08-23 计划（`doc/sql_index_rag_search_plan_20260823.md`）中明确记录为「后续可选项」的 **FTS5 全文索引** 落地，并在写入时做 **中文分词（charabia）+ 拼音（全拼/首字母）双索引**，实现：中文按词搜索、英文原词搜索、**用拼音搜中文**（`shujuku` / `sj` → 数据库）。**迁移前自动备份原库到 `mcphub.db.bak`**（需求硬性要求）。另新增 **活动日志筛选优化**（§8）：去掉用户筛选，服务器/工具/分组/API秘钥改为可搜索的分页下拉框。

## 1. 背景

- 2026-08-23 计划（v21/v22/v23）已把 servers / groups / RAG 文档 / skills / prompts / resources / activity 的搜索**后端化**（SQL `LIKE '%x%'` + 分页 + total），前端防抖调后端。但 `LIKE '%x%'` 前置通配符无法用 B-tree 索引，且拼音无从谈起——用户搜「数据」能靠子串命中「数据库」，但搜 `shujuku`、`sj` 无法命中任何东西。
- 各实体当前搜索实现（本计划的升级落点，全部已存在）：

| 实体 | 函数 | 表/字段 | 现状 |
|---|---|---|---|
| 服务器 | `server_service::search_configs` | `servers.name/description` | LIKE，运行时字段（状态/工具名）Rust 合并 |
| 分组 | `group_service::search_paged` | `groups.name/description` | LIKE |
| RAG 文档 | `rag::service::search_docs_paged` | `rag_docs.name` + 标签 | LIKE + EXISTS |
| 技能 | `skill_service::search_library_paged` | `skills.name/description/dir_name` | LIKE |
| Prompt | `prompt_service::search_paged` | `builtin_prompts.name/title/description` | LIKE |
| 资源 | `resource_service::search_paged` | `builtin_resources.name/uri/description` | LIKE |
| 活动日志 | `log_service::query_tool_activities` | `activity_log.server/tool/status` | LIKE（查询列不走 FTS；**筛选下拉优化见 §8**） |
| 应用日志 | `log_service::query_logs` | `app_log.message` | LIKE（高频写 + 15 天清理，**Phase F 可选**） |

## 2. 技术选型

### 2.1 候选方案对比

| 方案 | 原理 | 优点 | 缺点 | 结论 |
|---|---|---|---|---|
| A. FTS5 + 内置 unicode61 直接 MATCH | 不加词库 | 零依赖 | CJK 连续文本被切成**整段单 token**，中文搜索基本不可用；无拼音 | ✗（仅英文可用，不解决需求） |
| B. 写入时直接 jieba-rs 分词 + FTS5 | 业务写路径用 jieba-rs 分词/转拼音后写入 FTS5 表 | 纯 Rust；依赖最省 | 分词 API 只覆盖中文场景，拉丁规范化（大小写/变音符）、多语言混排都要自己补；后续加语言要换库 | 备选（charabia 的 `chinese` feature 内核就是 jieba-rs，直接用 jieba 无净收益） |
| **C. 写入时 charabia 分词 + FTS5（+ pinyin crate 做拼音）** | 业务写路径用 charabia（Meilisearch 分词器，**default 全语言 features**）tokenize + pinyin crate 转拼音后写入 FTS5 表；查询同构处理 | **统一 Tokenizer API + 全语言**：中文（内核 jieba-rs）+ 日/韩（lindera 词典）+ 泰/希伯来/希腊/越南/土耳其/德语复合词等；拉丁 lowercase/规范化内置；`Token` 自带 `lemma()`/偏移量，纯 Rust 零 C/FFI；后续语言只动 feature；分词逻辑可单测 | 需要在每个写路径补同步调用（SQL 触发器无法调 Rust）；FTS 文本≠原文，snippet 需自己截；japanese/korean 内嵌词典体积大（见 §2.3/§6） | ✅ **推荐（用户指定 charabia，全语言）** |
| D. C 扩展静态链接（wangfenjin/simple，自带拼音） | `cc` 编译 C++ 扩展源码 + 词典，注册为 FTS5 tokenizer | 触发器即可维护；拼音支持官方维护 | C++ 源码 + 词典资源入 build；6 平台交叉编译验证成本；与 sqlx 的注册路径要走 FFI | ✗（工程重；项目有刻意避开 C 依赖的先例——Cargo.toml 避开 onig/esaxx 的注释） |
| E. FFI 注册 Rust 自定义 tokenizer（libsqlite3-sys `fts5_api`） | `SqliteConnection::lock_handle()`（sqlx 0.9 已有）拿 `sqlite3*`，xCreateTokenizer 注册 | 触发器可维护、查询侧透明 | unsafe FFI、tokenizer 生命周期管理复杂、sqlx 升级易碎、可测试性差 | ✗（备选，仅当写路径同步被证明不可维护时再评估） |

### 2.2 选型结论

**charabia 分词（全语言） + FTS5 + pinyin crate 做拼音**（用户指定 charabia，且需开启全语言支持）。决定性理由：

1. **统一 Tokenizer API + 全语言覆盖**——charabia 用一个 `Token` 模型同时处理：中文（`chinese` feature，内核即 jieba-rs 词级切分）、日文/韩文（lindera 内嵌词典）、泰/希伯来/希腊/越南/土耳其/德语复合词等（default features 即全语言），拉丁文 lowercase/变音符规范化、混排标点丢弃都内置，`Token::lemma()` 直接给出规范化词元。
2. **纯 Rust / 零 FFI / 零平台差异**——桌面端 6 个构建目标（macOS ARM64/x64、Linux x64/ARM64、Windows x64/ARM64），不引入任何 C 工具链。
3. **可扩展**——后续语言支持只动 feature，不动代码；这正是选 charabia 而非裸 jieba 的主要价值。
4. **与项目既有模式同构**——FTS 表沿用 v21 起 `rag_docs` SQL 镜像的「源数据为事实源，SQL 表为查询索引，启动对账 + 写路径增量同步」模式，团队心智一致。
5. **可测试**——分词/拼音/查询构造是纯函数，`#[cfg(test)]` 覆盖无数据库也能测。

> 如实说明：charabia 的中文切分底层同样调用 jieba-rs（0.10，词典随 feature 进二进制），词级切分结果与裸 jieba-rs 一致；选择 charabia 的价值在统一 Token 模型、全语言覆盖与规范化能力，而非分词算法本身。

### 2.3 关键依赖（均已核实）

| 依赖 | 版本 | 事实核实 |
|---|---|---|
| **FTS5 本体** | — | ✅ `libsqlite3-sys 0.37.0`（sqlx 0.9 传递依赖，Cargo.lock 已锁）bundled build **无条件**加 `-DSQLITE_ENABLE_FTS5`（build.rs L132，已查本地 registry 源码）。**无需改任何 Cargo.toml 的 sqlite 部分**。执行时第一步仍跑 `pragma_compile_options` 冒烟断言兜底 |
| `charabia` | **0.10.0**（crates.io 最新，2026-08-13 发布，已核实 feature 表） | Meilisearch 的分词器，纯 Rust。**`charabia = { version = "0.10", features = ["latin-camelcase", "latin-snakecase"] }`（default 全语言 + 拉丁 camel/snake 拆分）**——default 全语言：chinese / hebrew / japanese / thai / korean / greek / khmer / vietnamese / swedish-recomposition / turkish / german-segmentation。`chinese` = jieba-rs 0.10 词级切分 + 中文规范化；`japanese` = lindera **embed-unidic** 内嵌词典；`korean` = lindera ko-dic 内嵌词典。`TokenizerBuilder::build()` → `tokenizer.tokenize(text)`，`Token::lemma()` 给出规范化词元、`is_separator()` 过滤标点。⚠️ ① japanese/korean 词典使二进制显著变大（unidic 为大头）——若后续体积不可接受，可退 `default-features=false, features=["chinese","japanese-segmentation-ipadic","korean",...]`（ipadic 替代 unidic），**默认按全语言执行**；② **严禁启用 `chinese-normalization-pinyin`**——它会把中文 lemma 归一成拼音，与 zh 列（存原文词元）语义冲突 |
| `pinyin` | 0.10 | mozillazg/rust-pinyin 纯 Rust；`ToPinyin` 迭代器支持 `Style::Plain`（`shu`）与首字母；多音字默认取最常见读音（heteronym 完整支持列为后续增强）。charabia 自身的 `chinese-normalization-pinyin` feature 也用 pinyin crate 做归一化，但那是繁简/拼音归一用途——本计划的 `py`/`ini` 索引列由我们自己生成，不受其影响 |

> 依赖面说明：charabia `chinese` feature 传递引入 jieba-rs 0.10 及词典；`japanese`/`korean` 传递引入 lindera + **内嵌** unidic / ko-dic 词典（体积大头，详见 §6 风险表）。
>
> 词库加载时机：`static TOKENIZER: OnceLock<Tokenizer>` 惰性初始化（首次分词调用构建；启动对账会触发首次加载，属预期）。

### 2.4 FTS5 表模式选型

| 模式 | 说明 | 结论 |
|---|---|---|
| **普通 FTS5 表（存分词文本 + ref_id UNINDEXED）** | FTS 表自身持有分词/拼音后的文本 + 源表主键 | ✅ 推荐：自包含、支持 `snippet()`、支持按 `rowid` 删除（普通表支持 `DELETE ... WHERE rowid=?`）。实体文本都是 name/description 级别（几十~几百字节），冗余可忽略 |
| external content（`content='servers'`） | 从源表回读原文供 snippet | ✗ 我们的 FTS 存的是**分词/拼音派生文本**，与源列不一致，external content 的回读假设不成立 |
| contentless（`content=''`） | 只存倒排 | ✗ 删除要 `contentless_delete=1`（SQLite ≥3.43），且无 snippet——收益只剩省空间，不值得 |

## 3. 分词与拼音方案（核心算法）

### 3.1 索引字段设计（每实体一张 FTS5 表，4 列）

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS fts_servers USING fts5(
    ref_id UNINDEXED,   -- 源表定位键（§4.1 有逐表清单；servers→name，groups→id）
    zh,                 -- charabia 分词后的词序列（空格分隔；lemma 已 lowercase 规范化）
    py,                 -- 每个 CJK 词的全拼连写 token（数据库→shujuku；英文词原样并入）
    ini                 -- 每个 CJK 词的首字母连写 token（数据库→sjk）
);
```

写入侧 `tokenize_fields(text) -> (zh, py, ini)`：

1. charabia `tokenizer.tokenize(text)` 产出 `Token` 流；`is_separator()` 过滤标点/空白；取 `lemma()`（charabia 已做 latin lowercase 等规范化）。charabia 的中文切分（chinese feature，jieba 内核）给出词级 token，中英混排自动处理。执行时核对 Token API 实际签名（is_separator/lemma 在所用版本的位置）。
2. `zh` = 各词元空格连接。
3. 对每个含 CJK 字符的词元：用 `pinyin` crate 把其中的汉字逐字转全拼连写追加进 `py`、首字母连写追加进 `ini`；纯英文词元原样同时写入 `py`（保证英文也能走 py 列命中）。
4. 多音字取 `pinyin` crate 默认（最常见）读音。

**多字段文本拼接（校验轮 P3 修正）**：`sync_upsert` 只有一个 text 入参，由**调用方拼接**该实体全部可搜索字段（与 §1 表格的搜索字段一一对应）：servers = `name + ' ' + description`、groups = `name + ' ' + description`、skills = `dir_name + ' ' + name + ' ' + description`、prompts = `name + ' ' + title + ' ' + description`、resources = `name + ' ' + uri + ' ' + description`、rag_docs = `name`。字段为 NULL/空则跳过，拼接用单空格分隔（bm25 不区分字段权重——可接受，若后续需要字段级权重再拆列）。

**查询路由 `build_match_query(input) -> Option<String>`**（None = 不走 FTS，调用方走原全量/空语义）：

| 输入形态 | 路由 | 示例 |
|---|---|---|
| 含 CJK | charabia 分词 → `zh:("词1" "词2"*)` | `数据库管` → `zh:("数据库" "管"*)` |
| 纯 ASCII | `(py:("t1"*) OR ini:("t1"*) OR zh:("t1"*))`（zh 兜底英文原词） | `shuju` / `sj` / `chrome` |
| 中英混合 | CJK 段走 zh、ASCII 段走 py/ini/zh，AND 连接 | `数据abc` → `zh:("数据") AND (py:("abc"*) OR zh:("abc"*))` |

- 所有用户 token 先经 `"` → `""` 转义再引号包裹，**杜绝 FTS5 语法注入**（`") OR (` 之类）。
- 前缀 `*` 只加在最后一个 token。
- 空输入/纯符号 → 返回 None，调用方保持「查全部」语义。

### 3.2 已知边界（如实记录，不隐瞒）

- **拼音词中前缀**：`ju` 匹配不到 `shujuku`（FTS5 前缀匹配只从 token 头开始）。与主流应用拼音搜索行为一致：需要完整词或词首前缀。
- **多音字**：默认读音，`重庆` 按 `chongqing` 索引。heteronym 双读音索引列为后续增强。
- **简繁**：charabia 的 `chinese-normalization` 含 kvariants 繁简归一（irg-kvariants），繁体输入经归一后**有望命中**简体索引内容；精确覆盖范围以 E4 实测为准（不在此承诺）。

## 4. 数据模型与迁移方案

### 4.0 迁移前备份（MUST——需求硬性要求）

**目标**：v24 迁移执行前把原库完整备份一份，任何迁移/回填事故可手工回滚。

- **备份文件**：`$APPDATA/mcphub.db.bak`（与 `mcphub.db` 同目录；macOS `~/Library/Application Support/app.mcphub.desktop/mcphub.db.bak`，Windows `%APPDATA%/app.mcphub.desktop/mcphub.db.bak`）。
- **时机**：`db::initialize` 中，`migration::run_pending()` **之前**，且仅当确有迁移待执行（`current schema_version < TARGET_VERSION`）——已升级过的老库每次启动不会重复备份，备份只在真正跑迁移的那次启动产生/覆盖。
- **方式**：`VACUUM INTO '…/mcphub.db.bak'`——SQLite 原生一致性快照（自带读事务，WAL 模式下无需先 checkpoint、无并发风险），输出为去碎片化的完整单文件副本，比手工 copy `db + -wal + -shm` 三件套可靠得多。
- **实现要点（校验轮 P1 修正）**：① SQLite 规定 `VACUUM INTO` 目标文件**已存在时直接报错**——覆盖前必须先 `std::fs::remove_file(&bak).ok()`（不存在则忽略）；② 目标路径作为 SQL 字符串进入语句，优先 bind 参数（`VACUUM INTO ?1`，执行时验证 sqlx 支持），不支持则对路径做单引号转义（`'` → `''`）——Windows 用户名可能含单引号，不能裸拼。
- **失败策略**：备份失败（磁盘满/权限）→ **中止迁移并返回 Err**，应用启动失败——绝不带病迁移；日志记 `[db] backup before migration failed: …`。
- **覆盖语义**：再次有迁移待执行时覆盖旧 `.bak`（先删后建，见上）；备份始终是「上一次迁移前」的状态——语义正确。回滚演练后再次启动同样会重新备份+迁移，旧 `.bak` 被覆盖——如需保留现场先手动改名。
- **pending 判断实现**：给 `migration.rs` 加 `pub async fn has_pending(pool) -> Result<bool>`（读 `schema_version` 与 TARGET 比较，复用既有 `get_current_version`），供备份步骤调用；`run_pending` 内部逻辑不变。
- **回滚方法**（人工，写进文档备查）：关应用 → `mv mcphub.db.bak mcphub.db`（同目录删掉 `-wal`/`-shm`）→ 重启（旧版 schema 由旧代码/或重新迁移）。

### 4.1 迁移 v24（`TARGET_VERSION` 23 → 24）

遵循 `agent.md` §3.5.5「新增迁移步骤（MUST FOLLOW）」五步。**迁移只建表，不做数据回填**（迁移拿不到 AppHandle，charabia 分词也不该在迁移里跑）——回填由 fts_service 对账完成，与 v21 的 rag_docs 完全同构。**且迁移执行前必先走 §4.0 的 db.bak 备份**。

```sql
-- migrate_v24（全部幂等；配套 migrations/0024_fts_tables.sql 供 sqlx::migrate! 兼容）
CREATE VIRTUAL TABLE IF NOT EXISTS fts_servers   USING fts5(ref_id UNINDEXED, zh, py, ini);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_groups    USING fts5(ref_id UNINDEXED, zh, py, ini);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_rag_docs  USING fts5(ref_id UNINDEXED, zh, py, ini);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_skills    USING fts5(ref_id UNINDEXED, zh, py, ini);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_prompts   USING fts5(ref_id UNINDEXED, zh, py, ini);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_resources USING fts5(ref_id UNINDEXED, zh, py, ini);
```

- ref_id 取值（**校验轮 P2 修正，与 §4.3 一致**）：servers→`name`（UNIQUE 且有 get_by_name 回表先例）、groups→`id`（主键即 id）、rag_docs→`id`、skills→`dir_name`、prompts/resources→`name`。
- `CREATE VIRTUAL TABLE IF NOT EXISTS` 合法（SQLite ≥3.9）；执行时如遇报错，改为先查 `sqlite_master` 再 CREATE（迁移函数内有先例）。
- **旧库升级**：v23 → v24 自动建空表 → 启动对账回填，用户无感。**全新安装**：空表 + 空源表 → no-op。

### 4.2 回填与对账（服务层，非迁移）

`fts_service::rebuild_all()`：`db::initialize` 成功后（lib.rs setup 内）调用，**职责范围（校验轮 P6 修正）：只重建 servers/groups/skills/prompts/resources 五张 FTS 表；`fts_rag_docs` 归 RAG 服务的 `rebuild_rag_sql_index` 管理**（与 rag_docs 镜像同生命周期：RAG 未启用时该表保持为空，避免 rebuild_all 越权扫描空表与 RAG 启动对账双重建）。逐实体 `DELETE FROM fts_x` → 源表全量读 → 逐行 tokenize → 批量 INSERT（单事务/实体），逐实体计时日志 `[fts] rebuild fts_servers: N rows in Xms`。漂移自愈能力与 rag_docs 对账一致（用户手改 DB / 崩溃残留）。

### 4.3 增量同步（写路径清单，MUST 全部覆盖——**硬性验收：不准出现数据不一致**）

> **一致性铁律（2026-09-05 需求）**：任何对源表的 INSERT/UPDATE/DELETE，FTS 表必须在**同一事务**内完成对应变更；漏一处即视为缺陷（验收不过）。兜底保障：启动对账（§4.2）自愈漂移 + §9 第 3 轮行数对账校验。

FTS 行与源行**同事务**写入（复用 `db::pool()`，WAL + busy_timeout=5000 已就绪）。**实现提示（校验轮 P9）**：现有写路径多为裸 `execute()`（单语句自动提交）——「同事务」要求调用方用 `pool.begin()` 显式包裹「源表写 + FTS 写」两条语句，执行时逐写路径补事务：

| 实体 | 写路径（现有代码位置） | 同步动作 |
|---|---|---|
| servers | `server_service::create/update/delete`（+ `commands/servers.rs` 删除分支） | upsert/delete `fts_servers`（ref_id=name）。**改名路径（校验轮 P5）**：update 变更 name 时 = 删旧 ref_id 行 + 插新 ref_id 行，否则旧行残留（搜旧名误命中）且新行缺失 |
| groups | `group_service::create/update/delete` | upsert/delete `fts_groups`（ref_id=id，改名不受 ref_id 影响） |
| rag_docs | `rag/service.rs` 的 `write_doc_and_index` / `update_doc` / `delete_doc` / reset 清空——**跟随既有 `rag_docs` SQL 镜像同步点，一个位置两件事** | upsert/delete `fts_rag_docs`（ref_id=id，文本=name） |
| skills | `skill_service` 的 skills 表写路径（安装/卸载/重命名） | upsert/delete `fts_skills`（ref_id=dir_name；重命名 = 删旧插新） |
| prompts/resources | builtin 表 seed/写路径（如有） | 同上 |
| **app_log** | `app_logger` drain task INSERT（唯一写入口）；`clear_logs` 全清；`cleanup_old_logs`/`cleanup_by_days` 按期清理 | upsert `fts_app_log`（ref_id=app_log.id，文本=message）/全清/按 ref_id→rowid 范围删（详见 Phase F1a–F1d） |

> **FTS5 无 UPDATE（校验轮 P4 修正）**：FTS5 虚拟表不支持 UPDATE 语句——`sync_upsert` 实现 = 先 `SELECT rowid WHERE ref_id=?` → 命中则 `DELETE WHERE rowid=?` → 再 `INSERT`。删除路径同理按 rowid。

> MCP 工具 `rag_file_create/update` 走 `rag::service` 内部函数，天然被上表覆盖。

### 4.4 搜索函数升级模式（每实体一个，统一形态）

```rust
// server_service::search_configs 升级后形态（其余实体同构；伪代码）
pub async fn search_configs(search_key: &str) -> Result<Vec<ServerConfig>> {
    if search_key.trim().is_empty() { /* 原全量路径不变 */ }
    // search_ref_ids 内部先调 build_match_query（§3.1），None 时返回空集（调用方空表兜底逻辑接管）
    match fts_service::search_ref_ids("fts_servers", search_key, LIMIT).await {
        Ok(ids) if !ids.is_empty() => /* 按 ids 回表（Rust 侧按 FTS rank 顺序重排） */,
        Ok(_) if fts_service::table_is_empty("fts_servers") => /* 空表兜底：走原 LIKE（首次启动对账前） */,
        Ok(_) => Ok(vec![]),
        Err(e) => { log::warn!("[fts] search failed, fallback to LIKE: {e}"); /* 降级原 LIKE 路径 */ }
    }
}
```

- **排序**：FTS 命中按 bm25 `rank` 排序后回表（回表 `WHERE name IN (...)` 后在 Rust 侧按 FTS 序重排）。
- **运行时字段**（servers 的状态过滤/工具名匹配）逻辑不动，仍 Rust 合并——FTS 只替换「文本列匹配」这一段。
- **防御**：所有 FTS SQL 常量字符串；动态部分（表名白名单枚举拼接 + MATCH 词 bind 传参）。

## 5. 执行计划

### Phase A：依赖、备份与迁移

- [x] A0. **迁移前备份**：`db::initialize` 在 `run_pending` 前检测到有待执行迁移时执行 `VACUUM INTO 'mcphub.db.bak'`（§4.0 完整规范）；失败即中止启动。单测：模拟 pending 迁移 → `.bak` 生成且可打开
- [x] A1. `Cargo.toml` 加 `charabia = { version = "0.10", features = ["latin-camelcase", "latin-snakecase"] }`（default 全语言 + camelCase/snake_case 拉丁拆分——**校验轮 P11**：charabia 0.10 的 default 已不含这两项，显式补上；**严禁再加 `chinese-normalization-pinyin`**——它会把中文 lemma 归一成拼音，破坏 zh 列语义——**校验轮 P10**）+ `pinyin = "0.10"`；`cargo check` 确认无 C 工具链引入（charabia 全链纯 Rust：jieba-rs/fst/whatlang/lindera 均纯 Rust）
- [x] A2. `db/migration.rs`：`TARGET_VERSION` 23 → 24；`migrate_v24` 建 6 张 FTS5 表（幂等）；配套 `migrations/0024_fts_tables.sql`
- [x] A3. FTS5 可用性冒烟断言：迁移开头查 `pragma_compile_options` 含 `ENABLE_FTS5`，否则 `Err`（fail-fast，不让建表静默失败留脏状态）
- [x] A4. 迁移幂等测试：复用文件尾既有测试基建（重跑 `run_pending` 应 no-op 到 v24；二次启动不再覆盖 `.bak`）

### Phase B：fts_service 核心模块（`src-tauri/src/services/fts_service.rs`）

- [x] B1. `static TOKENIZER: OnceLock<Tokenizer>`（charabia，全语言 default）+ `tokenize_fields(text) -> (zh, py, ini)`：charabia tokenize → `is_separator()` 过滤 → `lemma()` 取词元（Latin 已 lowercase 规范化）→ 含 CJK 词元经 pinyin crate 生成全拼/首字母
- [x] B2. `build_match_query(input) -> Option<String>`（None=不走 FTS；含转义/路由/前缀）
- [x] B3. `sync_upsert(table, ref_id, text)`（**FTS5 无 UPDATE：先查 rowid → 删 → 插**） / `sync_delete(table, ref_id)`（先查 rowid 再删；同事务由调用方包裹——调用方需显式 `BEGIN`，见 §4.3）
- [x] B4. `search_ref_ids(table, input, limit) -> Result<Vec<String>>`（MATCH + ORDER BY rank + ref_id 投影）
- [x] B5. `rebuild_all()` / `rebuild_one(table)`（对账回填）
- [x] B6. 单测：分词（中/英/混合/标点/空串）、拼音（全拼/首字母/非 CJK 透传）、查询构造（注入用例 `" OR 1`、空、纯符号）、建库→插→查→删 round-trip

### Phase C：实体接入（顺序按用户感知频率）

- [x] C1. **RAG 文档**：`write_doc_and_index`/`update_doc`/`delete_doc`/reset 同步 + `search_docs_paged` 升级 + `rebuild_rag_sql_index` 并入 fts_rag_docs 重建
- [x] C2. **servers**：`server_service` 三写路径同步 + `search_configs` 升级（保留运行时字段 Rust 合并）
- [x] C3. **groups**：`group_service` 同步 + `search_paged` 升级
- [x] C4. **skills**：`skill_service` 同步 + `search_library_paged` 升级
- [x] C5. **prompts/resources**：seed 写路径同步 + 两处 `search_paged` 升级
- [x] C6. `db::initialize` 尾部（或 lib.rs setup）调 `rebuild_all()`，日志 `[fts] rebuild fts_servers: N rows in Xms`

### Phase D：前端

- [x] D1. 预期零改动（后端命令签名/响应结构不变，仅排序与召回变好）；逐一 `grep` 确认无契约漂移
- [ ] D2.（可选增强，默认不做）结果高亮 snippet——需前端改渲染，单独立项

### Phase E：验证（MUST 全绿才算完成）

- [ ] E1. `cargo check`（`CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse`）
- [ ] E2. `cargo test`（fts_service 单测 + 迁移测试）
- [ ] E3. `cd frontend && npm run build`
- [ ] E4. 手动回归（`tauri dev`）：
  - 中文搜「数据」命中名称含「数据库」的 server / RAG 文档
  - 拼音搜 `shujuku`、首字母 `sj` 命中同上
  - 英文搜原词、词首前缀命中
  - 新建/改名/删除 server 与 RAG 文档后搜索结果即时正确（增量同步）
  - 升级路径：旧版 db 文件启动 → **`mcphub.db.bak` 自动生成（内容为 v23 状态，可直接 sqlite3 打开验证）** → v24 迁移 + 对账回填 → 搜索可用；再次启动无待迁移 → `.bak` 不被覆盖（mtime 不变）
  - 备份回滚演练：关应用 → 用 `.bak` 覆盖 `mcphub.db`（删 `-wal`/`-shm`）→ 重启 → 数据回到 v23 前状态且重新走迁移+备份
  - 降级路径：向搜索框注入 `") OR (` 之类，不 panic、无错误弹窗
  - **活动日志（Phase G）**：server/tool/group/key 四下拉可搜索、分页加载；选中后列表正确过滤（group/key 此前被丢弃，验证真正生效）；username 筛选已消失
- [ ] E5. 若随版本发布：四源版本号 bump（tauri.conf.json / Cargo.toml / 根 package.json / frontend/package.json）+ `doc/upgrade/{version}.md`

### Phase F：应用日志 FTS（**必做**——2026-09-05 需求升级：应用日志纳入全文检索）

> 原 F1 从可选项升级为必做。app_log 特性：高频写（唯一写入口 `app_logger::log_to_db` → channel → 后台 drain task 单点 INSERT）、15 天自动清理、message 长文本。**heartbeat 调试日志（每 30s 一条，2880 条/天）已于 2026-09-05 删除**，日志量大幅回落后 FTS 成本可控。

- [x] F1a. 迁移 v24 增建第 7 张表 `fts_app_log USING fts5(ref_id UNINDEXED, zh, py, ini)`（ref_id=app_log.id，TEXT uuid；文本=message；0024_fts_tables.sql 同步）
- [x] F1b. 写路径同步（**唯一写入口**）：`app_logger` 后台 drain task 的 INSERT app_log 处，同事务 INSERT fts_app_log（tokenize message）
- [x] F1c. 清理联动（MUST——不准数据不一致）：
  - `clear_logs()`：DELETE app_log 全表 → 同事务 DELETE fts_app_log 全表
  - `cleanup_old_logs()`/`cleanup_by_days()`：先 `SELECT id FROM app_log WHERE created_at < cutoff` 取待删 id 集 → 删 app_log → 删 fts_app_log（`DELETE WHERE rowid IN (SELECT rowid FROM fts_app_log WHERE ref_id IN (...ids...))`——FTS5 无范围删除，按 ref_id→rowid 两步删）
- [x] F1d. `query_logs` 搜索升级：message LIKE → FTS MATCH（走 §4.4 统一形态：build_match_query + rank 排序 + Err 降级 LIKE）
- [ ] F1e. 存量 heartbeat 日志清理：代码已删源头，但 app_log 表中存量 heartbeat 记录由 15 天自动清理逐步消化；E4 验证时如仍刷屏，可手动点日志页清除按钮一次性清掉
- [ ] F2.（可选）拼音 heteronym 多读音索引
- [ ] F3.（可选）snippet/高亮前端渲染

> Phase G（活动日志筛选优化）执行清单见 §8.3。

## 6. 风险与对策

> **执行进度（2026-09-05）**：Phase A/B/C/F/G(Rust+前端)/E1-E3 全部完成。
> - A：v24 迁移 + db.bak 备份 + FTS5 冒烟断言 + 幂等/备份语义测试 + `0024_fts_tables.sql` 配套
> - B：fts_service.rs（charabia 全语言分词 + pinyin 全拼/首字母 + 查询路由 + sync/search/rebuild/backfill）12 个单测
> - C：servers/groups/rag_docs/skills/prompts/resources 全部写路径同事务同步 + 5 个搜索函数升级 + lib.rs 启动对账（fire-and-forget）
> - F：add_log 同事务写 fts_app_log；clear_logs/cleanup_old_logs 清理联动（QueryBuilder 批量 rowid 删）；query_logs 加 search 参数（FTS + LIKE 降级）；app_log 存量回填（fts 空且 src 非空 → 单事务回填）
> - G：ActivityQuery 加 group_name/key_name；query_tool_activities 等值条件；get_activity_filter_options（field 白名单 + DISTINCT LIKE 分页）；tauriClient 透传 groupName/keyName + filter-options 路由；SearchableSelect 组件；ActivityPage 四筛选换组件 + username 删除
> - E1 cargo check ✅ / E2 cargo test --lib 33 passed ✅ / E3 npm run build ✅；E4 手动回归（tauri dev）待用户执行

> **实现要点记录（与计划的偏差/确认）**：
> - charabia 0.10 `Tokenizer<'tb>` 借用 builder 生命周期 → builder `Box::leak` 为 'static（builder 默认值 Owned Cow，不实际借用数据）
> - charabia `chinese-normalization` 会把简体归一成**繁体规范形**（"数据"→"數据"）——写入/查询同管道，简繁输入互通；pinyin crate 同时收录简繁映射（"數"→shu），py/ini 不受影响（已探针验证）
> - pinyin 0.10 API：`use pinyin::ToPinyin; c.to_pinyin() -> Option<Pinyin>`，`plain()` 需 feature `plain`（Cargo.toml 已加）
> - skills 安装流程为 FS 耦合多步非事务流 → FTS 同步 best-effort + 启动 rebuild 对账兜底；删除路径同事务
> - servers 改名=删旧插新（update 内比对 name）；prompts/resources ref_id=name → update/delete 前先查 name（同事务）
> - groups/rag_docs FTS 升级后排序为 FTS rank 序（原为 name/uploaded_at 序）——检索相关度优先

## 6. 风险与对策

| 风险 | 概率 | 对策 |
|---|---|---|
| 迁移把库写坏且无法恢复 | 低 | **§4.0 db.bak 强制备份**（VACUUM INTO 一致性快照，失败即拒迁）；E4 含回滚演练 |
| 全语言 charabia 二进制体积膨胀（lindera 内嵌 unidic/ko-dic 词典，数十 MB 级） | 确定 | 需求指定全语言，接受；E1 时记录各平台产物体积对比。若不可接受：退 `japanese-segmentation-ipadic`（小词典）或裁语言 features，分词代码不变 |
| 某平台 FTS5 未编译进 bundled sqlite | 低（0.37 build.rs 已核实无条件启用） | A3 冒烟断言 fail-fast |
| 写路径遗漏 → FTS 行漂移（搜不到新数据/搜到已删数据） | 中 | **§4.3 一致性铁律：所有 CRUD 同事务同步，漏一处即缺陷**；执行时逐写路径 grep 核对 + E4 回归专项 + §9 第 3 轮行数对账；漂移由启动对账自愈兜底 |
| MATCH 语法意外错误导致搜索全挂 | 低 | §5 Err 分支降级原 LIKE 路径，搜索永不报错弹窗 |
| 首启动对账耗时（超大库） | 低 | 逐实体计时日志；预估 <1s；必要时改后台 spawn |
| sqlx 0.9 `&'static str` 约束（AssertSqlSafe） | 确定（已知） | FTS SQL 常量字符串；动态部分（MATCH 词）走 bind 参数；表名白名单枚举拼接 |
| `LIKE` 与 FTS 结果排序差异引起前端困惑 | 低 | 前端已是「后端给什么渲染什么」，无排序假设；如需可加 i18n 文案 |

## 7. 与既有文档的关系

- 兑现 `doc/sql_index_rag_search_plan_20260823.md` §技术事实中「FTS5 全文索引暂不引入（记录为后续可选项）」；执行完成后在该文档顶部加一行指针注记。
- 完成后按 `agent.md` 核心约束，在第 3 节追加「3.x SQLite 全文索引（FTS5 + 中英/拼音分词）」小节，记录：迁移版本 v24、db.bak 备份机制、fts_service 模块、写路径同步清单、已知边界（拼音词中前缀/多音字）；并补登 §8 活动日志筛选优化（SearchableSelect 组件 + `get_activity_filter_options` 命令 + username 筛选移除）。

## 8. 活动日志筛选优化（可搜索分页下拉）

> 2026-09-05 新增需求：活动日志筛选条件 **去掉「用户」**；服务器/工具/分组/API秘钥四个条件改成**可搜索的分页下拉框**。

### 8.1 现状摸底（2026-09-05）

| 项 | 现状 | 问题 |
|---|---|---|
| 筛选 UI（`ActivityPage.tsx`） | 6 个文本框 + 原生 `<datalist>`：server / tool / status / group / **username** / keyName | datalist 是浏览器原生前缀补全，无搜索高亮、无分页、选项列表一次性全量 |
| 前端查询 | `filters = { server, tool, status, group, username, keyName }` → activityService → `GET activities?...` | group/username/keyName 三个参数**发出但桌面端静默丢弃**（见下）——纯摆设 |
| tauriClient 映射（`get_tool_activities`） | 只透传 `server/status/tool` 三个 query param | group/username/keyName **未映射** → Rust 收不到 |
| Rust `ActivityQuery` | `{ page, page_size, server, status, tool }` | 无 group_name / key_name 字段（serde 忽略未知字段） |
| `get_activity_filters` 命令 | 只返回 `DISTINCT server` | tools/groups/usernames/keyNames 的 datalist **全空** |
| 结论 | —— | username 筛选要删；group/keyName 需**补齐后端链路**；四个条件统一换成可搜索分页下拉 |

### 8.2 设计

**后端（查询仍走 LIKE——activity_log 不做 FTS，见 §5 Phase F 边界）**：

1. `ActivityQuery` 增 `group_name: Option<String>` / `key_name: Option<String>`（Tauri command 参数 camelCase `groupName`/`keyName`）。
2. `query_tool_activities` WHERE 增加等值条件 `group_name = ?` / `key_name = ?`（server 同为等值；tool 保持现有 LIKE）。
3. 新命令 `get_activity_filter_options(field, search, page, page_size) -> ActivityFilterOptionsPage`：
   - `field` 白名单枚举（防注入）：`server`→`server` 列、`tool`→`tool` 列、`group`→`group_name` 列、`key`→`key_name` 列；
   - SQL：`SELECT DISTINCT {col} AS v FROM activity_log WHERE {col} != '' AND {col} LIKE ? ESCAPE '\\' ORDER BY v LIMIT ? OFFSET ?` + `COUNT(DISTINCT {col})` 取 total（列名走枚举白名单拼接，search 词 bind 传参；**search 词先转义 `%`/`_`/`\`——沿用 group_service 的 `ESCAPE '\\'` 先例**）；
   - 返回 `{ items, total, page, page_size }`（复用既有分页响应模式）。
4. `lib.rs` 注册命令；`tauriClient.ts` 增加路由映射（如 `GET activities/filter-options?field=server&search=&page=1` → `get_activity_filter_options`）+ 响应转换分支。

**前端**：

5. 新组件 `frontend/src/components/ui/SearchableSelect.tsx`（**可搜索分页单选下拉**，参照 RagPage `TagSearchSelect` 的交互模式）：
   - 输入框聚焦/输入 → 防抖 250ms 调 `get_activity_filter_options`（search + page 1）；
   - 下拉列表：选项 + 尾部「加载更多」（`total > 已加载` 时）+ 竞态守卫（ref 锁，沿用 useRagData 的 `viewFetchingRef` 模式）；
   - 已选态：显示选中值 chip + 清除按钮（复用现有清除图标）；
   - 复用 hub 设计 token（`hub-input` / `hub-card` 风格下拉浮层）。
6. `ActivityPage.tsx` 筛选区改造：
   - server / tool / group / keyName 四个 input+datalist → `<SearchableSelect field=…>`（各自 field 枚举）；
   - **删除 username 筛选块**（`searchUsername` state、`filters.username`、datalist、UI 块；i18n 键 `activity.user`/`searchUsername` 保留不删，避免 locale 文件 diff 扩大）；
   - status 保持原生 select（成功/失败两态枚举，无需分页）。
7. tauriClient `get_tool_activities` 映射补透传 `groupName`/`keyName`；`get_activity_stats` 不动（统计条维度不需要）。

**i18n**：预期零新增（复用 `activity.server/tool/group/keyName` + `common.clear` + 通用「加载更多」文案；执行时核对四语言现有键）。

### 8.3 执行清单（Phase G）

- [x] G1. `models/log.rs`：`ActivityQuery` 加 `group_name`/`key_name`；新增 `ActivityFilterOptionsPage` 结构
- [x] G2. `log_service.rs`：`query_tool_activities` 加两列等值条件；新增 `get_activity_filter_options(field, search, page, page_size)`（field 白名单枚举 + DISTINCT LIKE 分页）
- [x] G3. `commands/logs.rs` + `lib.rs`：注册 `get_activity_filter_options` 命令
- [x] G4. `tauriClient.ts`：`get_tool_activities` 补透传（query param 名与 ActivityPage 现有 filters 键 `group`/`keyName` 对齐，映射为 Tauri args `groupName`/`keyName`）；新增 `activities/filter-options` 路由 + 响应转换
- [x] G5. `components/ui/SearchableSelect.tsx`：可搜索分页单选下拉组件（防抖/加载更多/竞态守卫/清除）
- [x] G6. `ActivityPage.tsx`：四个筛选换 SearchableSelect；删除 username 筛选块；验证 group/keyName 筛选真正生效（此前被丢弃）
- [x] G7. 回归：server/tool/group/key 四条件单独筛选 + 组合筛选 + 与分页/统计条联动；旧 `get_activity_filters` 命令保留（不破坏潜在 web 兼容）或一并清理（执行时定，倾向保留命令、前端不再调用）

## 9. 五轮符合校验（执行完成后 MUST 全部通过）

> 起草时已用「眼校验」做过一轮全文逻辑审查并修正 11 处问题（P1–P13，标注在各节）。执行完成后按下表跑满 5 轮，每轮结果记录在对应小节（发现问题回到对应 Phase 修复后重跑该轮）。

### 第 1 轮：需求符合校验（对照需求逐条勾验）

| # | 需求 | 计划落点 | 校验点 |
|---|---|---|---|
| 1 | SQLite 全文索引 | §2.4/§4.1 v24 六张 FTS5 表 | `sqlite_master` 可见 6 张 `fts_*` 表且 `pragma_compile_options` 含 ENABLE_FTS5 |
| 2 | 中英文分词支持 | §3.1 zh 列（charabia 词级切分 + latin lowercase） | 中文按词、英文原词/词首前缀均可命中 |
| 3 | 拼音支持 | §3.1 py/ini 列 + 查询路由 | `shujuku`→数据库、`sj`→数据库、`sjk` 全拼首字母连写 |
| 4 | 原库备份 db.bak | §4.0 + A0 | 升级路径生成 `mcphub.db.bak`（v23 状态可打开）；无待迁移不覆盖 |
| 5 | charabia 全语言 | §2.3/A1 | Cargo.toml default features + camel/snake；**未**启用 chinese-normalization-pinyin |
| 6 | 活动日志：去用户筛选 | §8 G6 | username 筛选块消失、`filters.username` 不再发送 |
| 7 | 活动日志：四条件可搜索分页下拉 | §8 G5/G6 | server/tool/group/key 走 SearchableSelect，防抖+加载更多生效 |
| 8 | 所有 CRUD 路径 FTS 与源表同步一致 | §4.3 铁律 + Phase F1c 清理联动 | §9 第 3 轮行数对账全过；CRUD 即时验证（增/改/删/清/清理） |
| 9 | 删除 [http-server-watch] heartbeat 日志 | 已执行（http_server.rs 调试心跳任务删除，2026-09-05） | 日志页不再刷 heartbeat；存量由 15 天清理消化（F1e） |

### 第 2 轮：代码一致性校验（文档承诺 ↔ 实际代码）

- [ ] v24 DDL 与 §4.1 六表 + fts_app_log 逐列一致（ref_id UNINDEXED + zh/py/ini），`TARGET_VERSION=24`，配套 `0024_fts_tables.sql` 存在
- [ ] ref_id 逐表取值与 §4.1 清单一致（servers→name / groups→id / rag_docs→id / skills→dir_name / prompts+resources→name / app_log→id）
- [ ] §4.3 六条写路径全部有同步调用（grep 各写函数体内的 `fts_service::`），servers/skills 改名路径含删旧插新
- [ ] app_logger 唯一写入口已同步；`clear_logs`/`cleanup_old_logs`/`cleanup_by_days` 清理联动已同步
- [ ] `sync_upsert` 实现为「查 rowid → 删 → 插」（无 UPDATE）；搜索 SQL 全部常量字符串 + bind
- [ ] 命令注册齐全（`get_activity_filter_options` 等），tauriClient 路由/透传与 G4 一致
- [ ] charabia 依赖行与 A1 完全一致

### 第 3 轮：数据一致性校验

- [ ] 7 张 FTS 表行数 = 对应源表行数（`SELECT COUNT(*)` 对账；fts_rag_docs 在 RAG 启用时 = rag_docs 行数）
- [ ] 手工抽查：改一个 server 描述后 FTS 行同步更新（py/ini 含新词拼音）；删除后 FTS 行消失
- [ ] 日志一致性：写几条日志 → fts_app_log 行数同步增加；手动清除日志 → fts_app_log 同步清空；等触发/手动清理后 FTS 同步删旧行（无孤儿行）
- [ ] `mcphub.db.bak` 可被 `sqlite3` 打开且 `schema_version` 为迁移前版本

### 第 4 轮：回归安全校验

- [ ] `cargo check` + `cargo test`（含迁移幂等/备份单测/fts_service 单测）+ `npm run build` 全绿
- [ ] E4 手动回归全项通过（含注入降级、备份回滚演练、Phase G 四下拉真过滤）
- [ ] FTS 异常注入（改名 MATCH 列? 模拟 Err）→ 搜索静默降级 LIKE，无报错弹窗

### 第 5 轮：文档同步校验

- [ ] 本计划 Phase A–G 全部 `[x]` 且带完成备注
- [ ] `agent.md` 第 3 节新增「3.x SQLite 全文索引（FTS5 + 中英/拼音分词）」小节 + §8 活动日志优化条目
- [ ] `doc/sql_index_rag_search_plan_20260823.md` 顶部加指针注记
- [ ] 若随版本发布：四源版本号一致 + `doc/upgrade/{version}.md` 存在
- [ ] 本计划 §0/§2/§3 的表述与最终实现无漂移（发现的漂移回写文档）


---

## 五轮代码级复核结果（2026-09-05/06，已完成）

对 Phase A–G 产物做 5 轮代码级复核（逻辑/性能/一致性/扩展性/回归），结果如下：

| 轮次 | 焦点 | 发现 | 处理 |
| ---- | ---- | ---- | ---- |
| 1 逻辑 | rebuild_one 索引范围 | **F1 [严重]**：`SELECT *` 拼全部 TEXT 列，servers 的 env/headers/openapi/proxy JSON（含密钥）被索引进 FTS，违反 P3 白名单 | ✅ 已修复：新增 `FtsTable::text_columns()` 白名单（servers/groups=name+description、rag_docs=name、skills=dir_name+name+description、prompts=name+title+description、resources=name+uri+description、app_log=message），`rebuild_one` 改 `SELECT {ref} AS fts_ref, {白名单列}` 按位读取 |
| 1 逻辑 | FTS5 多短语语义 | **F3 疑点**：`zh:("a" "b"*)` 空格分隔是否隐式 AND | ✅ 探针实证：多短语=隐式 AND、`zh:"a b"`=相邻短语、单 token `*`=前缀，符合 §2 设计；探针已删 |
| 2 性能 | SQL 生成泄漏 | **F2 [高]**：全部 `sql_*` 生成函数每次调用 `format!.leak()`，add_log 每条日志泄漏 3 个串 | ✅ 已修复：SQL 区替换为 `FtsSql` 缓存结构体 + `sqls()`（OnceLock，进程内只生成一次）；顺带删除死代码 `sql_tx_delete_delete`/`table_is_empty_sql`（F5） |
| 2 性能 | 搜索回表 | **F4**：servers/groups FTS 命中后 fetch 全表再 Rust 过滤 | 📌 已识别暂缓：表量级（几十~几百行）不构成瓶颈，量大时改 `WHERE ref_id IN (...)` |
| 3 一致性 | 事务边界/写路径 | settings_import 复用 server_service::create（同步继承）；7 张源表无 service 外直接写者；migration seed 由启动 rebuild_all 对账兜底 | ✅ 无问题 |
| 4 扩展性 | 新实体接入点 | FtsTable 枚举 / ALL 数组 / text_columns / REBUILD_TABLES 四处同步点已在注释标明 | ✅ 无问题 |
| 5 回归 | 全量验证 | `cargo check` 0 错 0 警（清除 `Column` 冗余导入）；`cargo test --lib` 33 passed / 0 failed（含 `rebuild_one_reconciles_servers` 白名单路径回归）；`npm run build` 通过；tsc 24 = 基线 | ✅ 通过 |

复核记录详见 `AGENTS.md` §3.15.7。

---

## 用户实测反馈轮（2026-09-06，已完成）

用户实测发现三问题，均已修复（详见 `AGENTS.md` §3.15.8）：

1. **多词查询查不到**：`build_match_query` 多 token 原为隐式 AND → 改组内/组间 OR 语义。
2. **存量 RAG 文档不在索引**：`rebuild_rag_sql_index` 仅在 RAG enable 时运行，存量 8 篇文档未入 `fts_rag_docs` → 已回填；应用重启后 enable 时 rebuild 自愈。
3. **相关度优先排序**：新增 `search_ref_ids_weighted()`（逐 token 查询合并命中数，命中词数降序），全部 8 处调用方改为 `counts` 稳定排序——命中数第一优先，同命中数保持各表原生序（rag=uploaded_at DESC 等）。FTS 命中 0 条/空表/Err 三种情况统一降级原 LIKE 子串查询。
4. 新增单测 `weighted_orders_by_token_hits`（34 passed / 0 failed）；`cargo check` 0 错 0 警；`npm run build` 通过。
5. **追补修复（同日）**：用户实测服务器搜 "idea sse stream" 排序第一是 `Idea-mcp-server`（仅命中 1 词）——根因是命令层 `search_servers` 在工具名兜底合并后 `sort_by_key(name)` 按字母序重排，覆盖了 weighted 序。真实 DB 模拟证实正确序 = `Idea-mcp-server-sse`(idea+sse=2) → `Idea-mcp-server-stream`(idea+stream=2) → `Idea-mcp-server`(idea=1)。修复：命令层重查 weighted 取相关度计数、FTS 降级时 Rust 逐 token 补齐计数、删除按名重排、工具名兜底升级为 token 级计数（详见 `AGENTS.md` §3.15.9）。
6. **日志页搜索接入后端（同日追补）**：用户搜「检查 更新」只回 1 条——日志页搜索框原为纯前端子串过滤（仅覆盖已拉取的最新 50 条：`/logs` GET 在 tauriClient 硬编码 `query:{}`，后端 FTS 从未被 UI 调用；实测最近 50 条恰含「检查」的仅 1 条 playwright 日志）。修复：`fetchLogs(search?)` 透传 `?search=&pageSize=200` → `get_logs` FTS weighted 相关度排序；`LogViewer` 桌面端搜索态走后端结果（type/source 过滤保留、300ms 防抖+竞态守卫），web 行为不变（详见 `AGENTS.md` §3.15.4）。
