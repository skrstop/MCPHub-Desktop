// 临时一次性回填工具：把 rag_docs 全表灌进 fts_rag_docs（用 fts_service 同款 tokenizer）。
// 跑完即删，不进 git。
use sqlx::sqlite::SqlitePoolOptions;

#[tokio::test]
async fn backfill_fts_rag_docs_from_live_db() {
    use mcphub_lib::services::fts_service::{self, FtsTable};

    // fts_service 的 tokenize_fields 是私有函数 —— 但 sync_upsert_tx 是 pub 的，
    // 且会内部调 tokenize_fields。所以这里直接调 sync_upsert_tx。
    // 但 sync_upsert_tx 需要 &mut SqliteConnection —— 从 pool 里取一个 conn。
    let db_url = format!(
        "sqlite://{}?mode=rw",
        std::env::var("LIVE_DB").unwrap_or_else(|_| {
            format!(
                "{}/Library/Application Support/app.mcphub.desktop/mcphub.db",
                std::env::var("HOME").unwrap()
            )
        })
    );
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&db_url)
        .await
        .unwrap();

    let rows = sqlx::query("SELECT id, name FROM rag_docs")
        .fetch_all(&pool)
        .await
        .unwrap();
    println!("rag_docs rows: {}", rows.len());

    let mut conn = pool.acquire().await.unwrap();
    for row in &rows {
        let id: String = sqlx::Row::try_get(row, "id").unwrap();
        let name: String = sqlx::Row::try_get(row, "name").unwrap();
        fts_service::sync_upsert_tx(
            &mut *conn,
            FtsTable::RagDocs,
            &id,
            &name,
        )
        .await
        .unwrap();
        println!("indexed: {} -> {}", id, name);
    }
}
