//! fts_service — SQLite FTS5 全文索引服务（中/英/拼音分词）
//!
//! 设计要点（详见 doc/sqlite_fts_pinyin_plan_20260905.md §3/§4）：
//! - 方案 B：写入时 Rust 分词（charabia 全语言 + pinyin），FTS5 普通虚拟表，
//!   每实体一张表：fts_{servers,groups,rag_docs,skills,prompts,resources,app_log}，
//!   列 = ref_id UNINDEXED + zh + py + ini。
//! - FTS5 虚拟表不支持 UPDATE、不支持 WHERE 删除 → sync_upsert = 查 rowid → 删 → 插；
//!   sync_delete = 查 rowid → 删。同事务性由调用方负责（pool.begin() 显式包裹，§4.3）。
//! - 查询路由 build_match_query：含 CJK → zh 列；纯 ASCII → py/ini/zh 三列 OR；
//!   混合 → AND 连接；所有 token `"`→`""` 转义防 FTS5 语法注入；空/纯符号 → None。
//! - rebuild_all() 只管 5 张实体表（fts_rag_docs 归 RAG 服务、fts_app_log 归日志写路径）。
//!
//! sqlx 0.9 约束：查询字符串必须是 &'static str —— 动态表名经白名单枚举 +
//! format!.leak() 生成（每表名仅泄漏一次，量级可忽略）。

use anyhow::Result;
use charabia::{Tokenizer, TokenizerBuilder};
use sqlx::Row;
use std::sync::OnceLock;

/// FTS 目标表白名单（杜绝表名注入；SQL 由 table_sql() 生成）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtsTable {
    Servers,
    Groups,
    RagDocs,
    Skills,
    Prompts,
    Resources,
    AppLog,
}

impl FtsTable {
    /// 源表名（rebuild 回填用）
    pub fn source_table(self) -> &'static str {
        match self {
            FtsTable::Servers => "servers",
            FtsTable::Groups => "groups",
            FtsTable::RagDocs => "rag_docs",
            FtsTable::Skills => "skills",
            FtsTable::Prompts => "builtin_prompts",
            FtsTable::Resources => "builtin_resources",
            FtsTable::AppLog => "app_log",
        }
    }

    /// ref_id 取的源列（rebuild 回填用）
    pub fn ref_column(self) -> &'static str {
        match self {
            FtsTable::Servers => "name",
            FtsTable::Groups => "id",
            FtsTable::RagDocs => "id",
            FtsTable::Skills => "dir_name",
            FtsTable::Prompts => "name",
            FtsTable::Resources => "name",
            FtsTable::AppLog => "id",
        }
    }

    /// FTS 表名
    pub fn fts_name(self) -> &'static str {
        match self {
            FtsTable::Servers => "fts_servers",
            FtsTable::Groups => "fts_groups",
            FtsTable::RagDocs => "fts_rag_docs",
            FtsTable::Skills => "fts_skills",
            FtsTable::Prompts => "fts_prompts",
            FtsTable::Resources => "fts_resources",
            FtsTable::AppLog => "fts_app_log",
        }
    }
}

// ---------------------------------------------------------------------------
// 分词器（charabia 全语言 default + latin-camelcase/snakecase）
// ---------------------------------------------------------------------------

/// charabia 的 Tokenizer<'tb> 类型上借用 builder 生命周期——把 builder 泄漏成
/// 'static 后 build() 即返回 Tokenizer<'static>（builder 默认值是 Owned Cow，
/// 不实际借用数据，泄漏一次安全）。
fn tokenizer() -> &'static Tokenizer<'static> {
    static TOK: OnceLock<Tokenizer<'static>> = OnceLock::new();
    TOK.get_or_init(|| {
        let builder: &'static mut TokenizerBuilder<'static, Vec<u8>> =
            Box::leak(Box::new(TokenizerBuilder::default()));
        builder.build()
    })
}

/// 判断字符是否 CJK（汉字区 + 扩展A + 兼容表意，覆盖常用简繁体）
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF   // CJK 扩展 A
        | 0x4E00..=0x9FFF // CJK 统一表意
        | 0xF900..=0xFAFF // CJK 兼容表意
    )
}

/// 词元是否含 CJK 字符
fn has_cjk(s: &str) -> bool {
    s.chars().any(is_cjk)
}

/// 汉字逐字转拼音，返回 (全拼连写, 首字母连写)。
/// 非 CJK 字符原样（lowercase）并入两串，保证混排词（如 "5G基站"）不丢字符。
/// 多音字取 pinyin crate 默认（最常见）读音。
fn cjk_to_pinyin(word: &str) -> (String, String) {
    use pinyin::ToPinyin;
    let mut full = String::with_capacity(word.len() * 4);
    let mut initials = String::with_capacity(word.len());
    for c in word.chars() {
        if is_cjk(c) {
            if let Some(pi) = c.to_pinyin() {
                full.push_str(pi.plain());
                if let Some(first) = pi.plain().chars().next() {
                    initials.push(first);
                }
            }
            // 无拼音（罕见）丢弃该字
        } else {
            let lower = c.to_ascii_lowercase();
            full.push(lower);
            initials.push(lower);
        }
    }
    (full, initials)
}

/// 对文本分词，产出 (zh, py, ini) 三列文本。
/// - zh：charabia 词元空格连接（lemma 已 lowercase）
/// - py：CJK 词 → 全拼连写 token；纯英文词 → 原样并入（英文走 py 命中）
/// - ini：CJK 词 → 首字母连写 token
pub fn tokenize_fields(text: &str) -> (String, String, String) {
    let mut zh = String::new();
    let mut py = String::new();
    let mut ini = String::new();

    for token in tokenizer().tokenize(text) {
        if token.is_separator() {
            continue;
        }
        let lemma = token.lemma();
        if lemma.is_empty() {
            continue;
        }
        if !zh.is_empty() {
            zh.push(' ');
        }
        zh.push_str(lemma);

        if has_cjk(lemma) {
            let (full, initials) = cjk_to_pinyin(lemma);
            if !py.is_empty() {
                py.push(' ');
            }
            py.push_str(&full);
            if !ini.is_empty() {
                ini.push(' ');
            }
            ini.push_str(&initials);
        } else {
            // 纯英文/数字词：原样并入 py 列（保证英文也能走 py 前缀命中）
            if !py.is_empty() {
                py.push(' ');
            }
            py.push_str(lemma);
        }
    }
    (zh, py, ini)
}

// ---------------------------------------------------------------------------
// 查询路由（§3.1 build_match_query）
// ---------------------------------------------------------------------------

/// FTS5 字符串转义：`"` → `""`
fn fts_escape(s: &str) -> String {
    s.replace('"', "\"\"")
}

/// 判断 token 是否全 ASCII（字母/数字）
fn is_ascii_token(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// 构造 FTS5 MATCH 查询串。
/// 返回 None = 不走 FTS（空输入/纯符号），调用方保持「查全部」语义。
///
/// 路由（§3.1）：
/// - 含 CJK 段：zh:("词1" "词2"*)（尾 token 加前缀 *）
/// - 纯 ASCII 段：(py:("t"*) OR ini:("t"*) OR zh:("t"*))
/// - 混合输入：CJK 段 AND ASCII 段
///
/// 注：输入先经 charabia 分词（与写入侧同源，保证查询词元与索引词元一致），
/// 再逐词元按形态路由；词元统一 `"`→`""` 转义后引号包裹。
pub fn build_match_query(input: &str) -> Option<String> {
    // 词元提取（与写入侧同一分词器 → 词元口径一致）
    let (zh_tokens, ascii_tokens) = extract_tokens(input)?;

    let mut parts: Vec<String> = Vec::with_capacity(2);

    // 多 token 语义 = OR（任一命中即返回）：
    // - 用户多词输入意图是「任一命中」，FTS5 空格分隔是隐式 AND（全词才命中），
    //   多词查不到数据（2026-09-06 用户报告）；OR 命中是超集，rank（bm25）把
    //   同时命中多词的行排最前，兼顾召回与相关性。
    // - 每 token 独立前缀匹配（此前只有最后一个 token 加 *）。
    if !zh_tokens.is_empty() {
        let mut q = String::from("zh:(");
        for (i, t) in zh_tokens.iter().enumerate() {
            if i > 0 {
                q.push_str(" OR ");
            }
            q.push('"');
            q.push_str(t);
            q.push('"');
            q.push('*');
        }
        q.push(')');
        parts.push(q);
    }

    if !ascii_tokens.is_empty() {
        // 每个 ASCII token 一组三列前缀 OR（py 全拼 / ini 首字母 / zh 英文原词兜底）
        let mut q = String::new();
        for (i, t) in ascii_tokens.iter().enumerate() {
            if i > 0 {
                q.push_str(" OR ");
            }
            q.push_str(&format!(r#"(py:"{t}"* OR ini:"{t}"* OR zh:"{t}"*)"#));
        }
        parts.push(q);
    }

    if parts.is_empty() {
        None
    } else {
        // zh 组与 ascii 组之间同为 OR（混合输入如 "web 搜索" 任一命中即可）
        Some(parts.join(" OR "))
    }
}

/// 查询词元提取：与写入侧同一分词器，返回 (CJK 词元, ASCII 词元)。
/// 空输入 / 纯符号返回 None。zh 词元为繁体规范形（kvariants + lowercase），
/// ascii 词元为 lowercase —— 供 weighted 搜索按 token 逐个统计命中数。
fn extract_tokens(input: &str) -> Option<(Vec<String>, Vec<String>)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut zh_tokens: Vec<String> = Vec::new(); // CJK 词元
    let mut ascii_tokens: Vec<String> = Vec::new(); // 纯 ASCII 字母数字词元

    for token in tokenizer().tokenize(trimmed) {
        if token.is_separator() {
            continue;
        }
        let lemma = token.lemma();
        if lemma.is_empty() {
            continue;
        }
        if has_cjk(lemma) {
            zh_tokens.push(fts_escape(lemma));
        } else if is_ascii_token(lemma) {
            ascii_tokens.push(fts_escape(lemma));
        }
        // 其他（符号词元）忽略
    }

    if zh_tokens.is_empty() && ascii_tokens.is_empty() {
        return None; // 空输入 / 纯符号
    }
    Some((zh_tokens, ascii_tokens))
}

// ---------------------------------------------------------------------------
// SQL 生成（sqlx 0.9 要 &'static str：每表每语句只生成一次并缓存，
// 严禁在调用路径上 format!.leak() —— add_log 高频路径每次泄漏 3 个串是真实内存泄漏）
// ---------------------------------------------------------------------------

/// 每张表的全部预生成 SQL（sync/search/count/clear 共用）
#[derive(Debug)]
struct FtsSql {
    /// sync_upsert_tx / sync_delete_tx 共用：按 rowid 删
    delete_by_rowid: &'static str,
    /// 插入一行（upsert 第二步 + rebuild 回填共用）
    insert: &'static str,
    /// 查 ref_id 对应 rowid
    select_rowid: &'static str,
    /// MATCH 搜索（rank 排序 + 限量）
    search: &'static str,
    count: &'static str,
    clear: &'static str,
}

impl FtsTable {
    /// 全部表（顺序 = 缓存数组下标，新增实体时同步维护）
    pub const ALL: [FtsTable; 7] = [
        FtsTable::Servers,
        FtsTable::Groups,
        FtsTable::RagDocs,
        FtsTable::Skills,
        FtsTable::Prompts,
        FtsTable::Resources,
        FtsTable::AppLog,
    ];

    fn idx(self) -> usize {
        self as usize
    }
}

fn sqls(t: FtsTable) -> &'static FtsSql {
    static ALL: OnceLock<Vec<FtsSql>> = OnceLock::new();
    &ALL.get_or_init(|| {
        FtsTable::ALL
            .iter()
            .map(|&t| {
                let name = t.fts_name();
                FtsSql {
                    delete_by_rowid: format!("DELETE FROM {name} WHERE rowid = ?1").leak(),
                    insert: format!(
                        "INSERT INTO {name} (ref_id, zh, py, ini) VALUES (?1, ?2, ?3, ?4)"
                    )
                    .leak(),
                    select_rowid: format!("SELECT rowid FROM {name} WHERE ref_id = ?1").leak(),
                    search: format!("SELECT ref_id FROM {name} WHERE {name} MATCH ?1 ORDER BY rank LIMIT ?2")
                        .leak(),
                    count: format!("SELECT COUNT(*) FROM {name}").leak(),
                    clear: format!("DELETE FROM {name}").leak(),
                }
            })
            .collect()
    })[t.idx()]
}

// ---------------------------------------------------------------------------
// 同步接口（供各服务写路径调用；同事务性由调用方负责）
// ---------------------------------------------------------------------------

/// 同步 upsert（自动提交版：自带事务，写一条源记录配一条 FTS 记录的场景用）。
/// 调用方若在显式事务内，请用 sync_upsert_tx。
pub async fn sync_upsert(table: FtsTable, ref_id: &str, text: &str) -> Result<()> {
    let pool = crate::db::pool();
    let mut tx = pool.begin().await?;
    sync_upsert_tx(&mut tx, table, ref_id, text).await?;
    tx.commit().await?;
    Ok(())
}

/// 同步 upsert（外部事务版：不提交，由调用方与源表写同事务提交——§4.3 铁律）
pub async fn sync_upsert_tx(
    tx: &mut sqlx::SqliteConnection,
    table: FtsTable,
    ref_id: &str,
    text: &str,
) -> Result<()> {
    let (zh, py, ini) = tokenize_fields(text);

    // FTS5 无 UPDATE：先查 rowid → 命中则删 → 再插
    let sel = sqls(table).select_rowid;
    let rowid: Option<i64> = sqlx::query_scalar(sel)
        .bind(ref_id)
        .fetch_optional(&mut *tx)
        .await?;
    if let Some(rid) = rowid {
        let del = sqls(table).delete_by_rowid;
        sqlx::query(del).bind(rid).execute(&mut *tx).await?;
    }

    let ins = sqls(table).insert;
    sqlx::query(ins)
        .bind(ref_id)
        .bind(&zh)
        .bind(&py)
        .bind(&ini)
        .execute(&mut *tx)
        .await?;
    Ok(())
}

/// 同步删除（自动提交版）
pub async fn sync_delete(table: FtsTable, ref_id: &str) -> Result<()> {
    let pool = crate::db::pool();
    let mut tx = pool.begin().await?;
    sync_delete_tx(&mut tx, table, ref_id).await?;
    tx.commit().await?;
    Ok(())
}

/// 同步删除（外部事务版）
pub async fn sync_delete_tx(
    tx: &mut sqlx::SqliteConnection,
    table: FtsTable,
    ref_id: &str,
) -> Result<()> {
    let sel = sqls(table).select_rowid;
    let rowid: Option<i64> = sqlx::query_scalar(sel)
        .bind(ref_id)
        .fetch_optional(&mut *tx)
        .await?;
    if let Some(rid) = rowid {
        let del = sqls(table).delete_by_rowid;
        sqlx::query(del).bind(rid).execute(&mut *tx).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 搜索（§4.4 统一形态：MATCH + rank 排序 + ref_id 投影）
// ---------------------------------------------------------------------------

/// 全文搜索，返回按相关度（bm25 rank）排序的 ref_id 列表。
/// build_match_query 返回 None（空/纯符号输入）时返回空集——
/// 调用方应以「空搜索 = 查全部」语义在 search 之前分流（§4.4）。
pub async fn search_ref_ids(table: FtsTable, input: &str, limit: i64) -> Result<Vec<String>> {
    Ok(search_ref_ids_weighted(table, input, limit)
        .await?
        .into_iter()
        .map(|(id, _)| id)
        .collect())
}

/// 加权搜索：返回 (ref_id, 命中查询词元数)，按命中数降序。
///
/// 排序语义（2026-09-06 用户要求）：**命中词元数（相关度）第一优先**——
/// 同时命中「中心」+「數据」两词的行必须排在只命中一词的行之前；
/// **同相关度层内的次序由调用方决定**（调用方按表的原生序稳定排序，
/// 如 rag 的 uploaded_at DESC、日志的 created_at DESC）。
///
/// 实现：逐 token 独立查询（每个 token 一次索引查询，token 数通常 ≤5），
/// 合并统计每行命中数。不用单条 OR + bm25——bm25 受文档长度/IDF 影响，
/// 稀有单词命中可能压过双词命中，不符合「命中越多越相关」的直觉。
pub async fn search_ref_ids_weighted(
    table: FtsTable,
    input: &str,
    limit: i64,
) -> Result<Vec<(String, i64)>> {
    search_ref_ids_weighted_on(table, crate::db::pool(), input, limit).await
}

/// 可注入 pool 的内部版本（单测用内存池；生产走上方全局池包装）
pub(crate) async fn search_ref_ids_weighted_on(
    table: FtsTable,
    pool: &sqlx::SqlitePool,
    input: &str,
    limit: i64,
) -> Result<Vec<(String, i64)>> {
    let Some((zh_tokens, ascii_tokens)) = extract_tokens(input) else {
        return Ok(vec![]);
    };

    // 每 token 一个 MATCH 表达式（与 build_match_query 同口径的单 token 形态）
    let mut exprs: Vec<String> = Vec::with_capacity(zh_tokens.len() + ascii_tokens.len());
    for t in &zh_tokens {
        exprs.push(format!(r#"zh:"{t}"*"#));
    }
    for t in &ascii_tokens {
        exprs.push(format!(r#"(py:"{t}"* OR ini:"{t}"* OR zh:"{t}"*)"#));
    }

    let sql = sqls(table).search;
    // ref_id → (命中数, 首次出现序)。首次出现序 = 各 token 查询的 rank 序依次
    // 合并的顺序，作为 count 相同时的确定性 tiebreak（调用方通常还会用
    // 表原生序再稳定排序覆盖它）。
    let mut merged: std::collections::HashMap<String, (i64, u32)> = std::collections::HashMap::new();
    for expr in &exprs {
        let ids: Vec<String> = sqlx::query_scalar(sql)
            .bind(expr)
            .bind(limit)
            .fetch_all(pool)
            .await?;
        for (seq, id) in ids.into_iter().enumerate() {
            let e = merged.entry(id).or_insert((0, seq as u32));
            e.0 += 1;
        }
    }

    let mut out: Vec<(String, i64)> = merged.into_iter().map(|(id, (c, _))| (id, c)).collect();
    // 命中数降序；count 相同保持首次出现序（sort_by_key 稳定）
    out.sort_by_key(|&(_, c)| std::cmp::Reverse(c));
    out.truncate(limit.max(0) as usize);
    Ok(out)
}

/// FTS 表行数（对账/兜底判断用）
pub async fn table_row_count(table: FtsTable) -> Result<i64> {
    let sql = sqls(table).count;
    let n = sqlx::query_scalar(sql).fetch_one(crate::db::pool()).await?;
    Ok(n)
}

/// FTS 表是否为空（首次启动对账前兜底判断，§4.4）
pub async fn table_is_empty(table: FtsTable) -> Result<bool> {
    Ok(table_row_count(table).await? == 0)
}

/// 清空某张 FTS 表（外部事务版：与源表写同事务提交）。
/// ⚠️ 不要在持有写锁的事务外另开连接调用——会与事务互相等锁死锁（曾致 clear_logs 失败）。
pub async fn clear_table_tx(tx: &mut sqlx::SqliteConnection, table: FtsTable) -> Result<()> {
    let sql = sqls(table).clear;
    sqlx::query(sql).execute(&mut *tx).await?;
    Ok(())
}

/// app_log 存量回填（升级后首次启动：fts_app_log 为空而 app_log 非空时，
/// 单事务「清空 + 全量回填」一次）。此后写路径 add_log 全部带 FTS 同步，
/// 清理路径同事务联动，正常不再漂移；崩溃/回滚由 .bak 快照一致性保证。
pub async fn backfill_app_log_if_empty() {
    let start = std::time::Instant::now();
    let src: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM app_log")
        .fetch_one(crate::db::pool())
        .await
        .unwrap_or(0);
    if src == 0 {
        return;
    }
    match table_row_count(FtsTable::AppLog).await {
        Ok(0) => {}
        Ok(_) => return, // 已有索引（非首次），跳过
        Err(e) => {
            log::warn!("[fts] backfill app_log check failed: {e}");
            return;
        }
    }

    let rows = match sqlx::query("SELECT id, message FROM app_log")
        .fetch_all(crate::db::pool())
        .await
    {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[fts] backfill app_log read failed: {e}");
            return;
        }
    };

    let pool = crate::db::pool();
    let mut tx = match pool.begin().await {
        Ok(t) => t,
        Err(e) => {
            log::warn!("[fts] backfill app_log begin failed: {e}");
            return;
        }
    };
    let clear_sql = sqls(FtsTable::AppLog).clear;
    if let Err(e) = sqlx::query(clear_sql).execute(&mut *tx).await {
        log::warn!("[fts] backfill app_log clear failed: {e}");
        return;
    }
    let ins = sqls(FtsTable::AppLog).insert;
    let mut n: i64 = 0;
    for row in &rows {
        let id: String = row.try_get(0).unwrap_or_default();
        let message: String = row.try_get(1).unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let (zh, py, ini) = tokenize_fields(&message);
        if let Err(e) = sqlx::query(ins)
            .bind(&id)
            .bind(&zh)
            .bind(&py)
            .bind(&ini)
            .execute(&mut *tx)
            .await
        {
            log::warn!("[fts] backfill app_log insert failed: {e}");
            return;
        }
        n += 1;
    }
    if let Err(e) = tx.commit().await {
        log::warn!("[fts] backfill app_log commit failed: {e}");
        return;
    }
    log::info!(
        "[fts] backfill fts_app_log: {} rows in {}ms",
        n,
        start.elapsed().as_millis()
    );
}

// ---------------------------------------------------------------------------
// 对账回填（§4.2：只管 5 张实体表；fts_rag_docs 归 RAG、fts_app_log 归日志写路径）
// ---------------------------------------------------------------------------

/// rebuild_all 覆盖的实体表
const REBUILD_TABLES: [FtsTable; 5] = [
    FtsTable::Servers,
    FtsTable::Groups,
    FtsTable::Skills,
    FtsTable::Prompts,
    FtsTable::Resources,
];

/// 每张表参与全文索引的源列（P3 多字段拼接规则，§1 表格一一对应）。
/// **必须显式列举，严禁 SELECT *** —— 否则 servers 的 env/headers/openapi JSON
/// （含密钥与超大 spec）会被索引进 FTS（泄露 + 膨胀），prompts 的 template /
/// resources 的 content 同理。
impl FtsTable {
    pub fn text_columns(self) -> &'static [&'static str] {
        match self {
            FtsTable::Servers => &["name", "description"],
            FtsTable::Groups => &["name", "description"],
            FtsTable::RagDocs => &["name"],
            FtsTable::Skills => &["dir_name", "name", "description"],
            FtsTable::Prompts => &["name", "title", "description"],
            FtsTable::Resources => &["name", "uri", "description"],
            FtsTable::AppLog => &["message"],
        }
    }
}

/// 全量重建（db::initialize 成功后调用一次）。逐实体：
/// DELETE 全表 → 源表全量读 → 逐行 tokenize → 单事务批量 INSERT → 计时日志。
/// 漂移自愈：用户手改 DB / 崩溃残留在这里被覆盖。
pub async fn rebuild_all() {
    let pool = crate::db::pool();
    for table in REBUILD_TABLES {
        let start = std::time::Instant::now();
        match rebuild_one(pool, table).await {
            Ok(n) => {
                log::info!(
                    "[fts] rebuild {}: {} rows in {}ms",
                    table.fts_name(),
                    n,
                    start.elapsed().as_millis()
                );
            }
            Err(e) => {
                // 重建失败不阻断启动：FTS 为空时各搜索走空表兜底（§4.4）
                log::warn!("[fts] rebuild {} failed: {e}", table.fts_name());
            }
        }
    }
}

/// 单表重建，返回写入行数（pool 显式传入以便测试注入）
pub async fn rebuild_one(pool: &sqlx::SqlitePool, table: FtsTable) -> Result<i64> {
    // 源表列：ref 列 + 白名单文本列（text_columns()，P3：多字段单空格拼接）。
    // 严禁 SELECT * —— 否则 servers 的 env/headers/openapi JSON（含密钥）等
    // 非白名单列会被索引（泄露 + 膨胀）。
    let src = table.source_table();
    let cols = table.text_columns();
    let select_sql: &'static str = {
        let mut s = format!("SELECT {} AS fts_ref", table.ref_column());
        for c in cols {
            s.push_str(", ");
            s.push_str(c);
        }
        s.push_str(" FROM ");
        s.push_str(src);
        // 低频启动路径，一次性分配可接受
        s.leak()
    };

    let rows = sqlx::query(select_sql).fetch_all(pool).await?;
    if rows.is_empty() {
        let clear_sql = sqls(table).clear;
        sqlx::query(clear_sql).execute(pool).await?;
        return Ok(0);
    }

    // 拼接：fts_ref = ref 列，之后按白名单列序单空格拼接（NULL/空串跳过）
    let mut entries: Vec<(String, String)> = Vec::with_capacity(rows.len());
    for row in &rows {
        let ref_id: Option<String> = row.try_get(0)?;
        let Some(ref_id) = ref_id else { continue };

        let mut text = String::new();
        for idx in 1..=cols.len() {
            let v: Option<String> = match row.try_get(idx) {
                Ok(v) => v,
                Err(_) => continue, // 非 TEXT 列（INTEGER/REAL/BLOB）跳过
            };
            if let Some(v) = v {
                if !v.is_empty() {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(&v);
                }
            }
        }
        entries.push((ref_id, text));
    }

    // 单事务：清空 + 批量插入
    let mut tx = pool.begin().await?;
    let clear_sql = sqls(table).clear;
    sqlx::query(clear_sql).execute(&mut *tx).await?;

    let ins = sqls(table).insert;
    let mut n: i64 = 0;
    for (ref_id, text) in &entries {
        let (zh, py, ini) = tokenize_fields(text);
        sqlx::query(ins)
            .bind(ref_id)
            .bind(&zh)
            .bind(&py)
            .bind(&ini)
            .execute(&mut *tx)
            .await?;
        n += 1;
    }
    tx.commit().await?;
    Ok(n)
}

// ---------------------------------------------------------------------------
// 单测（B6）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn zh_of(t: &str) -> String {
        tokenize_fields(t).0
    }
    fn py_of(t: &str) -> String {
        tokenize_fields(t).1
    }
    fn ini_of(t: &str) -> String {
        tokenize_fields(t).2
    }

    #[test]
    #[test]
    fn tokenize_chinese_word() {
        // "数据库" → jieba 词级切分（"数据"+"库"）+ charabia chinese-normalization
        // 繁体规范形归一（"数据"→"數据"）。查询侧同管道归一，简繁输入互通；
        // py/ini 为词元级全拼/首字母 token（pinyin crate 同时收录简繁映射，不受归一影响）。
        let (zh, py, ini) = tokenize_fields("数据库");
        assert_eq!(zh, "數据 庫", "zh: normalized (traditional-form) lemmas joined, got: {zh}");
        assert_eq!(py, "shuju ku", "py: per-word full-spelling tokens, got: {py}");
        assert_eq!(ini, "sj k", "ini: per-word initials, got: {ini}");
    }

    #[test]
    fn tokenize_english_passthrough() {
        let (zh, py, ini) = tokenize_fields("Chrome DevTools");
        let zh_l = zh.to_lowercase();
        assert!(zh_l.contains("chrome"), "zh got: {zh}");
        // 英文词原样进入 py（保证英文走 py 命中）
        assert!(py.to_lowercase().contains("chrome"), "py got: {py}");
        assert!(ini.is_empty() || !ini.chars().any(is_cjk));
    }

    #[test]
    #[test]
    fn tokenize_mixed_and_punct() {
        // 混排 + 标点：标点被过滤，中英各归其位；中文词元为繁体规范形
        let (zh, py, _ini) = tokenize_fields("chrome-调试,工具!");
        let zh_l = zh.to_lowercase();
        assert!(zh_l.contains("chrome"), "zh got: {zh}");
        assert!(zh.contains("調試") || zh.contains("調") || zh.contains("試"), "zh got: {zh}");
        assert!(py.contains("chrome"), "py got: {py}");
        // 标点不进任何列
        assert!(!zh.contains(',') && !zh.contains('!'));
    }

    #[test]
    fn tokenize_empty_and_symbols() {
        let (zh, py, ini) = tokenize_fields("");
        assert!(zh.is_empty() && py.is_empty() && ini.is_empty());
        let (zh, py, ini) = tokenize_fields("!!!@@@###");
        assert!(zh.is_empty() && py.is_empty() && ini.is_empty(), "pure symbols must yield empty");
    }

    #[test]
    fn pinyin_non_cjk_passthrough() {
        // 非 CJK 字符在 cjk_to_pinyin 中原样 lowercase 并入
        let (full, initials) = cjk_to_pinyin("aB5");
        assert_eq!(full, "ab5");
        assert_eq!(initials, "ab5");
    }

    #[test]
    fn match_query_cjk_route() {
        let q = build_match_query("数据库管").unwrap();
        assert!(q.starts_with("zh:(\""), "got: {q}");
        assert!(q.ends_with("*)"), "last token must have prefix star, got: {q}");
        // 多 token 语义 = OR（任一命中；隐式 AND 曾导致多词查询 0 结果）
        assert!(q.contains(" OR "), "multi-token must join with OR, got: {q}");
        assert!(!q.contains("py:"), "pure CJK should not route to py/ini, got: {q}");
        // 每个 token 都带前缀星号（此前只有最后一个 token 加）
        assert!(q.matches("\"*").count() >= 2, "every token must carry prefix star, got: {q}");
    }

    #[test]
    fn match_query_ascii_route() {
        let q = build_match_query("shuju").unwrap();
        assert!(q.contains("py:"), "got: {q}");
        assert!(q.contains("ini:"), "got: {q}");
        assert!(q.contains("zh:"), "got: {q}");
        assert!(q.ends_with("*)"), "got: {q}");
    }

    #[test]
    fn match_query_none_cases() {
        assert!(build_match_query("").is_none());
        assert!(build_match_query("   ").is_none());
        assert!(build_match_query("!!!###@@@").is_none());
    }

    #[test]
    fn match_query_injection_safe() {
        // FTS5 语法注入尝试必须被转义/过滤，不得产出可改变查询结构的语句
        for evil in ["\" OR 1", "\") OR (", "\"\"\"", "a\" OR b"] {
            if let Some(q) = build_match_query(evil) {
                // 转义后不允许出现未转义的引号边界破坏：每个引号都应成对闭合
                assert_eq!(q.matches('"').count() % 2, 0, "unbalanced quotes for input {evil:?}: {q}");
                assert!(!q.contains("OR 1\"") || evil == "\" OR 1");
            }
        }
    }

    /// 建库 → 插 → 查 → 删 round-trip（内存 SQLite，验证 MATCH 前缀与 rowid 删除）
    #[tokio::test]
    async fn roundtrip_insert_search_delete() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE VIRTUAL TABLE fts_servers USING fts5(ref_id UNINDEXED, zh, py, ini)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let (zh, py, ini) = tokenize_fields("数据库服务");
        sqlx::query("INSERT INTO fts_servers (ref_id, zh, py, ini) VALUES ('srv1', ?1, ?2, ?3)")
            .bind(&zh)
            .bind(&py)
            .bind(&ini)
            .execute(&pool)
            .await
            .unwrap();

        // 中文词前缀命中
        let q = build_match_query("数据").unwrap();
        let hit: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fts_servers WHERE fts_servers MATCH ?")
            .bind(&q)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(hit, 1, "CJK prefix query should match, q={q}");

        // 拼音全拼前缀命中
        let q = build_match_query("shuj").unwrap();
        let hit: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fts_servers WHERE fts_servers MATCH ?")
            .bind(&q)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(hit, 1, "pinyin prefix query should match, q={q}");

        // 首字母前缀命中
        let q = build_match_query("sj").unwrap();
        let hit: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fts_servers WHERE fts_servers MATCH ?")
            .bind(&q)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(hit, 1, "initials prefix query should match, q={q}");

        // 不相关拼音不命中
        let q = build_match_query("wangluo").unwrap();
        let hit: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fts_servers WHERE fts_servers MATCH ?")
            .bind(&q)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(hit, 0, "unrelated pinyin must not match, q={q}");

        // 按 rowid 删除（FTS5 删除语义）
        let rowid: i64 =
            sqlx::query_scalar("SELECT rowid FROM fts_servers WHERE ref_id = 'srv1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query("DELETE FROM fts_servers WHERE rowid = ?")
            .bind(rowid)
            .execute(&pool)
            .await
            .unwrap();
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fts_servers")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0, "row delete via rowid should remove the entry");

        pool.close().await;
    }

    /// rebuild_one 对账（第 3 轮数据一致性）：run_pending 建全 schema →
    /// 插入 server 行 → rebuild_one 重建 → 行数对账 + 搜索命中 + 删源行后
    /// 再 rebuild 归零。servers 的可搜索文本 = name + description（P3）。
    #[tokio::test]
    async fn rebuild_one_reconciles_servers() {
        use crate::db::migration::run_pending;
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run_pending(&pool).await.expect("run migrations");

        // 插两行 server（直接 SQL，绕过 service 层）
        sqlx::query(
            "INSERT INTO servers (id, name, server_type, description, enabled) \
             VALUES ('id-a', '数据库服务', 'stdio', '连接数据库的工具', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO servers (id, name, server_type, description, enabled) \
             VALUES ('id-b', 'chrome-devtools', 'stdio', NULL, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let n = super::rebuild_one(&pool, FtsTable::Servers).await.unwrap();
        assert_eq!(n, 2, "rebuild should index both rows");

        // 行数对账：FTS = 源
        let src: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM servers")
            .fetch_one(&pool)
            .await
            .unwrap();
        let fts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fts_servers")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(src, fts, "row-count reconciliation: fts == source");

        // 中文名命中（含描述字段文本）
        let q = build_match_query("数据").unwrap();
        let hit: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fts_servers WHERE fts_servers MATCH ?")
            .bind(&q)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(hit, 1, "description text should be searchable, q={q}");

        // 英文名命中
        let q = build_match_query("chrome").unwrap();
        let hit: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fts_servers WHERE fts_servers MATCH ?")
            .bind(&q)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(hit, 1, "english name should match, q={q}");

        // 删源行后重建 → 对账归零
        sqlx::query("DELETE FROM servers WHERE name='数据库服务'")
            .execute(&pool)
            .await
            .unwrap();
        super::rebuild_one(&pool, FtsTable::Servers).await.unwrap();
        let fts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fts_servers")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fts, 1, "rebuild after delete should reconcile to 1");

        pool.close().await;
    }

    /// weighted 相关度排序（2026-09-06 用户要求）：命中查询词元数第一优先——
    /// 命中 2 词的行排在只命中 1 词的行之前；count 相同保持首次出现序。
    #[tokio::test]
    async fn weighted_orders_by_token_hits() {
        use crate::db::migration::run_pending;
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run_pending(&pool).await.expect("run migrations");

        // 查询词 "数据服务"（两词）；id-a 命中两词，id-b 只命中 "數据"
        sqlx::query(
            "INSERT INTO servers (id, name, server_type, description, enabled) \
             VALUES ('id-a', '数据服务中心', 'stdio', NULL, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO servers (id, name, server_type, description, enabled) \
             VALUES ('id-b', '数据库', 'stdio', NULL, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        super::rebuild_one(&pool, FtsTable::Servers).await.unwrap();

        let out = search_ref_ids_weighted_on(FtsTable::Servers, &pool, "数据服务", 100)
            .await
            .unwrap();
        assert_eq!(out.len(), 2, "both rows should hit, got {out:?}");
        // servers 表 ref_id = name 列（非 id）
        assert_eq!(out[0].0, "数据服务中心", "double-token hit must rank first, got {out:?}");

        // 期望命中数 = 查询词 token 数（weighted 每个 token 一次查询，同口径用 extract_tokens 计算）
        let (zh_toks, ascii_toks) = extract_tokens("数据服务").unwrap();
        let expect = (zh_toks.len() + ascii_toks.len()) as i64;
        assert!(expect >= 2, "query should tokenize to >=2 tokens, zh={zh_toks:?}");
        assert_eq!(out[0].1, expect, "id-a hits all query tokens, got {out:?}");
        assert!(out[1].1 < expect, "id-b hits fewer tokens, got {out:?}");

        pool.close().await;
    }
}
