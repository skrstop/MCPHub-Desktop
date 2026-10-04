//! The `smart_tool` lancedb table — one row per indexed tool plus one row per
//! server, on the SHARED mv connection. Schema mirrors origin's
//! `vector_embeddings` rows (content_type / content_id / text_content /
//! embedding / metadata / model) with desktop additions (server_name for
//! scoped deletes + tool_set_hash for the skip check).
//!
//! Dim handling mirrors `rag_chunk`: an EXISTING table whose embedding width
//! differs from the loaded model is dropped + recreated (all embeddings are
//! meaningless under the new model) — `needs_full_reindex()` then tells the
//! index layer to re-embed every server.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use futures_util::TryStreamExt;
use lancedb::{
    arrow::arrow::array::{
        Array, ArrayRef, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator,
        StringArray,
    },
    arrow::arrow::datatypes::{DataType, Field, Schema, SchemaRef},
    arrow::arrow_array::RecordBatchReader,
    Connection,
    query::{ExecutableQuery, QueryBase},
};
use serde_json::Value;

pub const TABLE_NAME: &str = "smart_tool";

/// One row to persist (tool or server level).
pub struct SmartRow<'a> {
    /// "tool" or "server".
    pub content_type: &'a str,
    /// Tool: "{server}:{tool_name}". Server: server name.
    pub content_id: &'a str,
    pub server_name: &'a str,
    /// Tool name (empty for server rows).
    pub tool_name: &'a str,
    /// The text that was embedded (searchable text).
    pub text_content: &'a str,
    /// The embedding model identifier (mv active model).
    pub model: &'a str,
    /// toolSetHash (tools only; empty for server rows).
    pub tool_set_hash: &'a str,
    /// JSON metadata (toolName/description/inputSchema; free-form).
    pub metadata: &'a Value,
    pub embedding: &'a [f32],
}

/// A hit returned by vector/keyword search.
#[derive(Clone, Debug)]
pub struct SmartHit {
    pub content_type: String,
    pub content_id: String,
    pub server_name: String,
    pub tool_name: String,
    pub text_content: String,
    pub metadata: Option<Value>,
    /// Cosine distance (`1 - cos`, lower = closer), 0.0 for keyword hits.
    pub distance: f32,
}

pub struct SmartToolStore {
    conn: Connection,
    embed_dim: usize,
    /// True iff an existing table was dropped due to a dim mismatch — the
    /// caller must re-index every server.
    needs_full_reindex: bool,
}

impl SmartToolStore {
    /// Open (creating if absent) the table on the shared mv connection.
    pub async fn open_with_conn(conn: Connection, embed_dim: usize) -> Result<Self> {
        let needs_full_reindex = ensure_table(&conn, embed_dim).await?;
        Ok(Self { conn, embed_dim, needs_full_reindex })
    }

    pub fn needs_full_reindex(&self) -> bool {
        self.needs_full_reindex
    }

    /// Replace all rows of one server (tools + server row) with `rows`.
    /// Delete-then-insert (lancedb has no upsert); the per-server delete keeps
    /// this O(server rows).
    pub async fn replace_server(&self, server_name: &str, rows: &[SmartRow<'_>]) -> Result<()> {
        self.delete_server_rows(server_name).await?;
        if rows.is_empty() {
            return Ok(());
        }
        let schema = self.schema();
        let n = rows.len();
        let dim = self.embed_dim;

        let ct: Vec<&str> = rows.iter().map(|r| r.content_type).collect();
        let cid: Vec<&str> = rows.iter().map(|r| r.content_id).collect();
        let sn: Vec<&str> = rows.iter().map(|r| r.server_name).collect();
        let tn: Vec<&str> = rows.iter().map(|r| r.tool_name).collect();
        let tc: Vec<&str> = rows.iter().map(|r| r.text_content).collect();
        let md: Vec<&str> = rows.iter().map(|r| r.model).collect();
        let tsh: Vec<&str> = rows.iter().map(|r| r.tool_set_hash).collect();
        let meta: Vec<String> = rows
            .iter()
            .map(|r| serde_json::to_string(r.metadata).unwrap_or_else(|_| "{}".into()))
            .collect();

        let mut flat = Vec::with_capacity(n * dim);
        for r in rows {
            if r.embedding.len() != dim {
                return Err(anyhow!(
                    "embedding dim {} != table dim {} for {}",
                    r.embedding.len(),
                    dim,
                    r.content_id
                ));
            }
            flat.extend_from_slice(r.embedding);
        }

        let emb = FixedSizeListArray::new(
            Arc::new(Field::new("item", DataType::Float32, false)),
            dim as i32,
            Arc::new(Float32Array::from(flat)) as ArrayRef,
            None,
        );

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(ct)) as ArrayRef,
                Arc::new(StringArray::from(cid)),
                Arc::new(StringArray::from(sn)),
                Arc::new(StringArray::from(tn)),
                Arc::new(StringArray::from(tc)),
                Arc::new(StringArray::from(md)),
                Arc::new(StringArray::from(tsh)),
                Arc::new(StringArray::from(meta.iter().map(String::as_str).collect::<Vec<_>>())),
                Arc::new(emb),
            ],
        )
        .map_err(|e| anyhow!("smart_tool build batch: {}", e))?;

        let reader: Box<dyn RecordBatchReader + Send> =
            Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema));
        let table = self.conn.open_table(TABLE_NAME).execute().await?;
        table.add(reader).execute().await?;
        Ok(())
    }

    /// Delete every row (tool + server) of one server.
    pub async fn delete_server_rows(&self, server_name: &str) -> Result<()> {
        let filter = format!("server_name = '{}'", esc(server_name));
        let table = self.conn.open_table(TABLE_NAME).execute().await?;
        table.delete(&filter).await?;
        Ok(())
    }

    /// Delete ALL rows (full reindex path / model swap).
    pub async fn delete_all(&self) -> Result<()> {
        let table = self.conn.open_table(TABLE_NAME).execute().await?;
        table.delete("content_type IS NOT NULL").await?;
        Ok(())
    }

    /// Delete rows matching a raw SQL filter (model-reload purge of stale
    /// rows embedded by a previous model).
    pub async fn delete_where(&self, filter: &str) -> Result<()> {
        let table = self.conn.open_table(TABLE_NAME).execute().await?;
        table.delete(filter).await?;
        Ok(())
    }

    /// (content_id, tool_set_hash) pairs for one server + model — the skip
    /// check's identity comparison.
    pub async fn identities(&self, server_name: &str, model: &str) -> Result<Vec<(String, String)>> {
        let filter = format!(
            "server_name = '{}' AND content_type = 'tool' AND model = '{}'",
            esc(server_name),
            esc(model)
        );
        let cols = ["content_id", "tool_set_hash"];
        let mut out = Vec::new();
        for batch in self
            .query_columns(
                &filter,
                &cols.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            )
            .await?
        {
            let ids = col::<StringArray>(&batch, "content_id");
            let hashes = col::<StringArray>(&batch, "tool_set_hash");
            for i in 0..batch.num_rows() {
                out.push((ids.value(i).to_string(), hashes.value(i).to_string()));
            }
        }
        Ok(out)
    }

    /// The server-level row: (model, text_content) — the skip check compares
    /// both against the expected values.
    pub async fn server_row(&self, server_name: &str) -> Result<Option<(String, String)>> {
        let filter = format!(
            "server_name = '{}' AND content_type = 'server'",
            esc(server_name)
        );
        let cols = vec!["model".to_string(), "text_content".to_string()];
        for batch in self.query_columns(&filter, &cols).await? {
            let models = col::<StringArray>(&batch, "model");
            let texts = col::<StringArray>(&batch, "text_content");
            if batch.num_rows() > 0 {
                return Ok(Some((models.value(0).to_string(), texts.value(0).to_string())));
            }
        }
        Ok(None)
    }

    /// Vector nearest-neighbor over tool rows (cosine), restricted to
    /// `allowed` servers when given.
    pub async fn vector_search(
        &self,
        query: &[f32],
        limit: usize,
        allowed: Option<&[String]>,
    ) -> Result<Vec<SmartHit>> {
        let table = self.conn.open_table(TABLE_NAME).execute().await?;
        let mut q = table
            .query()
            .nearest_to(query)
            .map_err(|e| anyhow!("smart nearest_to: {}", e))?
            .distance_type(lancedb::DistanceType::Cosine)
            .limit(limit);
        if let Some(list) = allowed {
            if list.is_empty() {
                return Ok(Vec::new());
            }
            let in_list = list
                .iter()
                .map(|s| format!("'{}'", esc(s)))
                .collect::<Vec<_>>()
                .join(", ");
            q = q.only_if(&format!("server_name IN ({})", in_list));
        }
        let mut stream = q.execute().await?;
        let mut hits = Vec::new();
        while let Some(batch) = stream
            .try_next()
            .await
            .map_err(|e| anyhow!("read smart vector hits: {}", e))?
        {
            collect_hits(&batch, &mut hits);
        }
        Ok(hits)
    }

    /// Keyword channel: rows whose text_content contains any query term
    /// (case-insensitive LIKE), tool + server rows both searchable.
    pub async fn keyword_search(
        &self,
        query: &str,
        // Superseded by KEYWORD_FETCH_CAP below: the Rust-side merge_hits
        // applies the real threshold + limit pruning deterministically.
        _limit: usize,
        allowed: Option<&[String]>,
    ) -> Result<Vec<SmartHit>> {
        let terms: Vec<&str> = query.split_whitespace().filter(|t| !t.is_empty()).collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let clauses: Vec<String> = terms
            .iter()
            .map(|t| {
                format!(
                    "lower(text_content) LIKE lower('%{}%') ESCAPE '\\'",
                    esc_like(t)
                )
            })
            .collect();
        let mut filter = clauses.join(" OR ");
        if let Some(list) = allowed {
            if list.is_empty() {
                return Ok(Vec::new());
            }
            let in_list = list
                .iter()
                .map(|s| format!("'{}'", esc(s)))
                .collect::<Vec<_>>()
                .join(", ");
            // The server restriction must AND with the term clauses — OR-ing
            // it would return rows from OUT-OF-SCOPE servers that merely
            // match a term (scope/bearer leak).
            filter = format!("({}) AND server_name IN ({})", filter, in_list);
        }
        let table = self.conn.open_table(TABLE_NAME).execute().await?;
        // Fetch a wide candidate set (deterministic, desktop-scale) instead of
        // `limit`: with an SQL-side cap of `limit` and no ORDER BY, which
        // matching rows survive is scan-order dependent — merge_hits would
        // then be deterministic over a nondeterministic subset. The Rust-side
        // merge applies the real threshold + limit pruning.
        const KEYWORD_FETCH_CAP: usize = 10_000;
        let mut stream = table
            .query()
            .only_if(&filter)
            .limit(KEYWORD_FETCH_CAP)
            .execute()
            .await?;
        let mut hits = Vec::new();
        while let Some(batch) = stream
            .try_next()
            .await
            .map_err(|e| anyhow!("read smart keyword hits: {}", e))?
        {
            collect_hits(&batch, &mut hits);
        }
        Ok(hits)
    }

    async fn query_columns(
        &self,
        filter: &str,
        cols: &[String],
    ) -> Result<Vec<RecordBatch>> {
        let table = self.conn.open_table(TABLE_NAME).execute().await?;
        let mut stream = table
            .query()
            .only_if(filter)
            .select(lancedb::query::Select::Columns(cols.to_vec()))
            .execute()
            .await?;
        let mut out = Vec::new();
        while let Some(b) = stream
            .try_next()
            .await
            .map_err(|e| anyhow!("smart query: {}", e))?
        {
            out.push(b);
        }
        Ok(out)
    }

    fn schema(&self) -> SchemaRef {
        smart_schema(self.embed_dim)
    }
}

/// Pull SmartHit rows out of a query batch (shared by vector + keyword paths;
/// `_distance` only exists on vector queries — keyword hits default to 0.0).
fn collect_hits(batch: &RecordBatch, hits: &mut Vec<SmartHit>) {
    let n = batch.num_rows();
    if n == 0 {
        return;
    }
    let ct = col::<StringArray>(batch, "content_type");
    let cid = col::<StringArray>(batch, "content_id");
    let sn = col::<StringArray>(batch, "server_name");
    let tn = col::<StringArray>(batch, "tool_name");
    let tc = col::<StringArray>(batch, "text_content");
    let meta = col::<StringArray>(batch, "metadata");
    let dist = batch
        .column_by_name("_distance")
        .and_then(|a| a.as_any().downcast_ref::<Float32Array>());
    for i in 0..n {
        hits.push(SmartHit {
            content_type: ct.value(i).to_string(),
            content_id: cid.value(i).to_string(),
            server_name: sn.value(i).to_string(),
            tool_name: tn.value(i).to_string(),
            text_content: tc.value(i).to_string(),
            metadata: serde_json::from_str(meta.value(i)).ok(),
            distance: dist.map(|d| d.value(i)).unwrap_or(0.0),
        });
    }
}

/// Create the table if absent; drop + recreate when the embedding width no
/// longer matches the loaded model (returns `true` in that case).
async fn ensure_table(conn: &Connection, embed_dim: usize) -> Result<bool> {
    // dim=0 means the mv runtime isn't up yet (model not loaded) — the dim is
    // UNKNOWN, not a mismatch. Treating it as a mismatch would DROP the whole
    // table (every server's embeddings) and the caller's `needs_full_reindex`
    // return is ignored by cleanup paths like remove_server_embeddings.
    if embed_dim == 0 {
        anyhow::bail!("smart: embedding dim unknown (model not loaded) — refusing to touch table");
    }
    let names = conn
        .table_names()
        .execute()
        .await
        .map_err(|e| anyhow!("smart list tables: {}", e))?;
    let exists = names.iter().any(|n| n == TABLE_NAME);
    if exists {
        let table = conn.open_table(TABLE_NAME).execute().await?;
        let schema = table.schema().await.map_err(|e| anyhow!("smart schema: {}", e))?;
        let dim_matches = match schema.field_with_name("embedding") {
            Ok(f) => matches!(
                f.data_type(),
                DataType::FixedSizeList(_, n) if *n as usize == embed_dim
            ),
            Err(_) => false,
        };
        if dim_matches {
            return Ok(false);
        }
        log::info!(
            "[smart] smart_tool embedding dim != model dim {} — recreating table (reindex required)",
            embed_dim
        );
        let _ = conn.drop_table(TABLE_NAME, &[]).await;
    }
    let schema = smart_schema(embed_dim);
    let empty = RecordBatch::new_empty(schema.clone());
    let reader: Box<dyn RecordBatchReader + Send> =
        Box::new(RecordBatchIterator::new(vec![Ok(empty)], schema));
    conn.create_table(TABLE_NAME, reader)
        .execute()
        .await
        .map_err(|e| anyhow!("smart create table: {}", e))?;
    Ok(exists) // recreated over an existing table → prior rows are gone
}

fn smart_schema(embed_dim: usize) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("content_type", DataType::Utf8, false),
        Field::new("content_id", DataType::Utf8, false),
        Field::new("server_name", DataType::Utf8, false),
        Field::new("tool_name", DataType::Utf8, false),
        Field::new("text_content", DataType::Utf8, false),
        Field::new("model", DataType::Utf8, false),
        Field::new("tool_set_hash", DataType::Utf8, false),
        Field::new("metadata", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                embed_dim as i32,
            ),
            false,
        ),
    ]))
}

fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

/// LIKE-pattern escape: `%`/`_` in a query term are wildcards by default and
/// would make `a%b` cross-word match (inflating keyword scores against the
/// term semantics). Use an explicit `ESCAPE '\'` clause in the LIKE.
fn esc_like(s: &str) -> String {
    s.replace('\'', "''")
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Per-server index summary (for the status command / settings panel).
pub async fn store_index_summary() -> Result<Vec<(String, usize)>> {
    let embed_dim = crate::mv::embed_dim().unwrap_or(0);
    let conn = crate::mv::connection_async().await?;
    let store = SmartToolStore::open_with_conn(conn, embed_dim).await?;
    let batches = store
        .query_columns(
            "content_type = 'tool'",
            &vec!["server_name".to_string()],
        )
        .await?;
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for b in batches {
        let sn = col::<StringArray>(&b, "server_name");
        for i in 0..b.num_rows() {
            *counts.entry(sn.value(i).to_string()).or_insert(0) += 1;
        }
    }
    Ok(counts.into_iter().collect())
}

/// Row totals split by content_type (for the performance command).
pub async fn store_performance() -> Result<(usize, usize, usize)> {
    let embed_dim = crate::mv::embed_dim().unwrap_or(0);
    let conn = crate::mv::connection_async().await?;
    let store = SmartToolStore::open_with_conn(conn, embed_dim).await?;
    let count = async |filter: &str| -> Result<usize> {
        let batches = store
            .query_columns(filter, &vec!["content_id".to_string()])
            .await?;
        Ok(batches.iter().map(|b| b.num_rows()).sum())
    };
    let tools = count("content_type = 'tool'").await?;
    let servers = count("content_type = 'server'").await?;
    Ok((servers, tools, tools + servers))
}

fn col<'a, T: Array + 'static>(batch: &'a RecordBatch, name: &str) -> &'a T {
    let arr = batch
        .column_by_name(name)
        .unwrap_or_else(|| panic!("smart_tool missing column {}", name));
    arr.as_any()
        .downcast_ref::<T>()
        .unwrap_or_else(|| panic!("smart_tool column {} wrong type", name))
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use serde_json::json;

    fn row(server: &str, tool: &str, text: &str, dim: usize) -> SmartRow<'static> {
        // Leak tiny per-test strings — fine for a test binary.
        let meta = Box::leak(json!({"toolName": tool}).to_string().into_boxed_str());
        SmartRow {
            content_type: if tool.is_empty() { "server" } else { "tool" },
            content_id: Box::leak(format!("{}:{}", server, tool).into_boxed_str()),
            server_name: Box::leak(server.to_string().into_boxed_str()),
            tool_name: Box::leak(tool.to_string().into_boxed_str()),
            text_content: Box::leak(text.to_string().into_boxed_str()),
            model: "test-model",
            tool_set_hash: "hash",
            metadata: Box::leak(Box::new(serde_json::from_str::<Value>(meta).unwrap())),
            embedding: Box::leak(vec![0.5f32; dim].into_boxed_slice()),
        }
    }

    async fn store(dir: &std::path::Path, dim: usize) -> SmartToolStore {
        let conn = lancedb::connect(dir.to_str().unwrap())
            .execute()
            .await
            .expect("connect");
        SmartToolStore::open_with_conn(conn, dim).await.expect("open")
    }

    #[tokio::test]
    async fn keyword_search_respects_allowed_scope() {
        let dir = std::env::temp_dir().join(format!("smart-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = store(&dir, 4).await;
        let rows = vec![
            row("alpha", "tool_a", "image generation for cats", 4),
            row("beta", "tool_b", "image generation for dogs", 4),
        ];
        s.replace_server("alpha", &rows[..1]).await.unwrap();
        s.replace_server("beta", &rows[1..]).await.unwrap();

        // Scope = alpha only: beta must NOT appear even though it matches the term.
        let hits = s
            .keyword_search("image generation", 10, Some(&["alpha".to_string()]))
            .await
            .unwrap();
        assert!(!hits.is_empty(), "expected alpha hits");
        assert!(hits.iter().all(|h| h.server_name == "alpha"),
            "scope leak: {:?}", hits.iter().map(|h| &h.server_name).collect::<Vec<_>>());

        // No scope: both servers returned.
        let all = s.keyword_search("image generation", 10, None).await.unwrap();
        let servers: std::collections::HashSet<_> = all.iter().map(|h| h.server_name.clone()).collect();
        assert!(servers.contains("alpha") && servers.contains("beta"));

        // Empty allow-list → no results at all.
        let empty = s.keyword_search("image", 10, Some(&[])).await.unwrap();
        assert!(empty.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn vector_search_respects_allowed_and_returns_distance() {
        let dir = std::env::temp_dir().join(format!("smart-test-v-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = store(&dir, 4).await;
        let rows = vec![
            row("alpha", "tool_a", "search the web", 4),
            row("beta", "tool_b", "send an email", 4),
        ];
        s.replace_server("alpha", &rows[..1]).await.unwrap();
        s.replace_server("beta", &rows[1..]).await.unwrap();
        let q = vec![0.5f32; 4];
        let hits = s.vector_search(&q, 10, Some(&["alpha".to_string()])).await.unwrap();
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|h| h.server_name == "alpha"));
        assert!(hits.iter().all(|h| (h.distance - 0.0).abs() < 1e-5)); // identical vectors
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn replace_then_identities_roundtrip_and_dim_recreate() {
        let dir = std::env::temp_dir().join(format!("smart-test-r-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = store(&dir, 4).await;
        let rows = vec![row("alpha", "tool_a", "text one", 4), row("alpha", "", "alpha server row", 4)];
        s.replace_server("alpha", &rows).await.unwrap();
        let ids = s.identities("alpha", "test-model").await.unwrap();
        assert_eq!(ids.len(), 1, "server rows excluded from identities");
        assert_eq!(ids[0].0, "alpha:tool_a");
        let sr = s.server_row("alpha").await.unwrap();
        assert!(matches!(sr, Some((m, t)) if m == "test-model" && t == "alpha server row"));

        // Dim change → recreate + needs_full_reindex, rows gone.
        let s2 = store(&dir, 8).await;
        assert!(s2.needs_full_reindex());
        assert!(s2.identities("alpha", "test-model").await.unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn delete_where_purges_by_model() {
        let dir = std::env::temp_dir().join(format!("smart-test-m-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = store(&dir, 4).await;
        let rows = vec![row("alpha", "tool_a", "text", 4)];
        s.replace_server("alpha", &rows).await.unwrap();
        s.delete_where("model != 'new-model'").await.unwrap();
        assert!(s.identities("alpha", "test-model").await.unwrap().is_empty());
        // Same-model rows survive.
        let rows2 = vec![row("alpha", "tool_a", "text", 4)];
        s.replace_server("alpha", &rows2).await.unwrap();
        s.delete_where("model != 'test-model'").await.unwrap();
        assert_eq!(s.identities("alpha", "test-model").await.unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod similarity_tests {
    use super::*;

    async fn store_with_vecs(dir: &std::path::Path, dim: usize) -> SmartToolStore {
        let conn = lancedb::connect(dir.to_str().unwrap()).execute().await.unwrap();
        SmartToolStore::open_with_conn(conn, dim).await.unwrap()
    }

    fn row(server: &str, tool: &str, emb: &[f32], _dim: usize) -> SmartRow<'static> {
        SmartRow {
            content_type: "tool",
            content_id: Box::leak(format!("{}:{}", server, tool).into_boxed_str()),
            server_name: Box::leak(server.to_string().into_boxed_str()),
            tool_name: Box::leak(tool.to_string().into_boxed_str()),
            text_content: Box::leak(tool.to_string().into_boxed_str()),
            model: "m",
            tool_set_hash: "h",
            metadata: Box::leak(Box::new(serde_json::json!({}))),
            embedding: Box::leak(emb.to_vec().into_boxed_slice()),
        }
    }

    /// Distance-type sanity at the lancedb level: identical normalized vectors
    /// → cosine distance 0; orthogonal → ≈1; opposite → ≈2.
    #[tokio::test]
    async fn cosine_distance_semantics() {
        let dir = std::env::temp_dir().join(format!("smart-sim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = store_with_vecs(&dir, 2).await;
        // One row whose embedding we vary per assert would be simpler, but
        // lancedb needs distinct content_ids — use three rows with unit vectors.
        let rows = vec![
            row("s", "same", &[1.0, 0.0], 2),
            row("s", "orth", &[0.0, 1.0], 2),
            row("s", "oppo", &[-1.0, 0.0], 2),
        ];
        s.replace_server("s", &rows).await.unwrap();

        let hits = s.vector_search(&[1.0, 0.0], 3, None).await.unwrap();
        let by_tool: std::collections::HashMap<String, f32> = hits
            .into_iter()
            .map(|h| (h.tool_name.clone(), h.distance))
            .collect();
        assert!((by_tool["same"]).abs() < 1e-4, "identical → 0, got {}", by_tool["same"]);
        assert!((by_tool["orth"] - 1.0).abs() < 1e-4, "orthogonal → 1, got {}", by_tool["orth"]);
        assert!((by_tool["oppo"] - 2.0).abs() < 1e-4, "opposite → 2, got {}", by_tool["oppo"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Non-normalized embeddings still rank correctly under cosine: a query
    /// parallel to a longer doc vector still has distance 0.
    #[tokio::test]
    async fn cosine_invariant_to_magnitude() {
        let dir = std::env::temp_dir().join(format!("smart-simm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = store_with_vecs(&dir, 2).await;
        let rows = vec![row("s", "scaled", &[10.0, 0.0], 2)];
        s.replace_server("s", &rows).await.unwrap();
        let hits = s.vector_search(&[0.001, 0.0], 1, None).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].distance.abs() < 1e-4, "magnitude must not affect cosine, got {}", hits[0].distance);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
