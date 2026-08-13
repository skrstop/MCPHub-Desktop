//! Vector database (lancedb) for RAG document chunks.
//!
//! Stores one row per text chunk: `{ id, doc_id, doc_name, chunk_index,
//! chunk_text, embedding }`. `embedding` is a `FixedSizeList<f32, embed_dim>`.
//! Search uses **cosine** distance (magnitude-invariant) so ranking reflects
//! semantic direction, not vector magnitude - robust to models whose
//! embeddings aren't L2-normalized (LFM2). With L2 distance, small-magnitude
//! docs would rank high regardless of relevance (inverted results).
//!
//! The DB lives at `<app_data_dir>/rag/lancedb`. Operations:
//!   - `open(dir)`       → connect + ensure the `rag_chunk` table exists
//!   - `add_chunks(...)` → append a batch of chunks for one document
//!   - `search(...)`     → vector nearest-neighbor, returns hits + distances
//!   - `delete_by_doc`   → remove all chunks of a document
//!   - `drop_table`      → drop everything (for reset)

use std::path::Path;
use std::sync::Arc;
use anyhow::{anyhow, Result};
use futures_util::TryStreamExt;
use lancedb::{
    arrow::arrow::array::{
        Array, ArrayRef, FixedSizeListArray, Float32Array, Int64Array, ListArray,
        ListBuilder, RecordBatch, RecordBatchIterator, StringBuilder, StringArray,
    },
    arrow::arrow::datatypes::{DataType, Field, Schema, SchemaRef},
    arrow::arrow_array::RecordBatchReader,
    connect, Connection,
    query::{ExecutableQuery, QueryBase},
};

// embed_dim is passed into VectorDb::open / stored on the struct; no
// embedding-module constants are needed here.

pub const TABLE_NAME: &str = "rag_chunk";

/// A row returned by a similarity search.
#[derive(Clone, Debug)]
pub struct SearchHit {
    pub doc_id: String,
    pub doc_name: String,
    pub chunk_index: i64,
    pub chunk_text: String,
    /// lancedb `_distance` (lower = closer). Convert to a similarity score at
    /// the caller (embeddings are normalized, so ranking == cosine ranking).
    pub distance: f32,
    pub tags: Vec<String>,
}

/// A chunk read back from the table (with its embedding), so tags can be
/// updated in place without re-running the embedding model.
pub struct ChunkRecord {
    pub chunk_id: String,
    pub doc_id: String,
    pub doc_name: String,
    pub chunk_index: i64,
    pub chunk_text: String,
    pub embedding: Vec<f32>,
}

/// A chunk to insert.
pub struct ChunkInput<'a> {
    pub chunk_id: String,
    pub doc_id: String,
    pub doc_name: String,
    pub chunk_index: i64,
    pub chunk_text: String,
    pub embedding: &'a [f32],
    pub tags: Vec<String>,
}

pub struct VectorDb {
    conn: Connection,
    /// Embedding dimension this DB was opened for (matches the loaded model's
    /// `embed_dim`). Drives the `FixedSizeList<f32, embed_dim>` column width
    /// and row slicing in `add_chunks`. Stored so callers don't have to thread
    /// it through every call.
    embed_dim: usize,
    /// True iff `open` dropped an EXISTING table because its schema was
    /// incompatible (incl. embedding dim != the loaded model's). Only this
    /// case requires a re-index — old embeddings are gone / meaningless under
    /// the new model. False on a fresh create (first enable / reset: no prior
    /// data to re-embed) and when the existing table already matches (same-dim
    /// model swap). The service reads this to decide whether to pop the
    /// reindex dialog.
    needs_reindex: bool,
}

impl VectorDb {
    /// Connect to (creating if absent) the lancedb at `dir`, opened for the
    /// given embedding dim. If an EXISTING table's `embedding` column has a
    /// different FixedSizeList width (model swapped to a different dim), the
    /// table is dropped + recreated and `needs_reindex()` returns true so the
    /// caller re-indexes. A fresh create or a same-dim swap returns false — no
    /// reindex, the existing embeddings stay.
    pub async fn open(dir: &Path, embed_dim: usize) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(|e| anyhow!("create lancedb dir {}: {}", dir.display(), e))?;
        let uri = dir.to_str().ok_or_else(|| anyhow!("lancedb path not UTF-8: {}", dir.display()))?;
        let conn = connect(uri)
            .execute()
            .await
            .map_err(|e| anyhow!("lancedb connect {}: {}", dir.display(), e))?;
        let needs_reindex = Self::ensure_table(&conn, embed_dim).await?;
        Ok(Self { conn, embed_dim, needs_reindex })
    }

    /// Whether `open` invalidated existing embeddings (existing table dropped
    /// due to schema/dim mismatch) — caller should re-index all docs when true.
    pub fn needs_reindex(&self) -> bool {
        self.needs_reindex
    }

    /// The embedding dim this DB is shaped for.
    pub fn embed_dim(&self) -> usize {
        self.embed_dim
    }

    async fn ensure_table(conn: &Connection, embed_dim: usize) -> Result<bool> {
        let names = conn
            .table_names()
            .execute()
            .await
            .map_err(|e| anyhow!("list tables: {}", e))?;
        let exists = names.iter().any(|n| n == TABLE_NAME);

        // `invalidated` is true iff an EXISTING table had to be dropped because
        // its schema was incompatible (missing tags col / non-null tags inner /
        // embedding dim != the loaded model). Only that case invalidates the
        // old embeddings and requires a re-index. A FRESH create (first enable,
        // or after a reset) returns false — there's no prior data to re-embed,
        // so the service must NOT pop the reindex dialog. This is the
        // "prefer keeping the existing dim, only reload when the model's dim
        // genuinely differs" behavior: same-dim model swaps (e.g. two 768-dim
        // models) keep the table and never reindex.
        let mut invalidated = false;

        if exists {
            // Schema migration: drop + recreate when the on-disk schema is
            // incompatible with what `add_chunks` now writes. Cases:
            //   1. table predates the `tags` column;
            //   2. `tags` has a non-null inner Utf8 field (old schema) - the
            //      ListBuilder we use produces a nullable inner field, so a
            //      non-null inner makes RecordBatch::try_new reject the batch;
            //   3. the `embedding` FixedSizeList width != the loaded model's
            //      embed_dim (model swapped to a different dim) — old
            //      embeddings are meaningless under the new model.
            let table = conn
                .open_table(TABLE_NAME)
                .execute()
                .await
                .map_err(|e| anyhow!("open table: {}", e))?;
            let schema = table.schema().await.map_err(|e| anyhow!("schema: {}", e))?;
            let need_recreate = match schema.field_with_name("tags") {
                Err(_) => {
                    log::info!("[RAG] rag_chunk table missing 'tags' column - recreating");
                    true
                }
                Ok(field) => match field.data_type() {
                    DataType::List(inner) if !inner.is_nullable() => {
                        log::info!(
                            "[RAG] rag_chunk 'tags' has non-null inner field - recreating for nullable inner"
                        );
                        true
                    }
                    _ => false,
                },
            };
            let need_recreate = need_recreate
                || match schema.field_with_name("embedding") {
                    Ok(f) => match f.data_type() {
                        DataType::FixedSizeList(_, n) => {
                            if *n as usize != embed_dim {
                                log::info!(
                                    "[RAG] rag_chunk 'embedding' dim {} != model embed_dim {} — recreating table (model swapped)",
                                    n, embed_dim
                                );
                                true
                            } else {
                                false
                            }
                        }
                        _ => false,
                    },
                    Err(_) => false,
                };
            if need_recreate {
                let _ = conn.drop_table(TABLE_NAME, &[]).await;
                invalidated = true;
                // fall through to create below
            } else {
                return Self::ensure_index(conn).await.map(|_| false);
            }
        }

        // Create an empty table from the schema (zero rows) so subsequent
        // `add` calls can append. Reached on fresh create (invalidated=false)
        // and after dropping an incompatible existing table (invalidated=true).
        let schema = chunk_schema(embed_dim);
        let empty = RecordBatch::new_empty(schema.clone());
        let reader: Box<dyn RecordBatchReader + Send> =
            Box::new(RecordBatchIterator::new(vec![Ok(empty)], schema));
        conn.create_table(TABLE_NAME, reader)
            .execute()
            .await
            .map_err(|e| anyhow!("create table: {}", e))?;
        Self::ensure_index(conn).await?;
        Ok(invalidated)
    }

    /// Create a LabelList scalar index on `tags` (accelerates tag filters).
    /// Ignore errors (e.g. already exists or not yet supported on this build).
    async fn ensure_index(conn: &Connection) -> Result<()> {
        let table = conn
            .open_table(TABLE_NAME)
            .execute()
            .await
            .map_err(|e| anyhow!("open table: {}", e))?;
        let _ = table
            .create_index(
                &["tags"],
                lancedb::index::Index::LabelList(Default::default()),
            )
            .execute()
            .await;
        Ok(())
    }

    /// Insert a batch of chunks for one document.
    pub async fn add_chunks(&self, chunks: &[ChunkInput<'_>]) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let schema = chunk_schema(self.embed_dim);
        let n = chunks.len();
        let dim = self.embed_dim;

        let ids: Vec<&str> = chunks.iter().map(|c| c.chunk_id.as_str()).collect();
        let doc_ids: Vec<&str> = chunks.iter().map(|c| c.doc_id.as_str()).collect();
        let doc_names: Vec<&str> = chunks.iter().map(|c| c.doc_name.as_str()).collect();
        let chunk_idx: Vec<i64> = chunks.iter().map(|c| c.chunk_index).collect();
        let texts: Vec<&str> = chunks.iter().map(|c| c.chunk_text.as_str()).collect();

        // Embedding: flat f32 array of n*dim, wrapped in FixedSizeList.
        let mut flat = Vec::with_capacity(n * dim);
        for c in chunks {
            flat.extend_from_slice(c.embedding);
        }

        let id_arr = StringArray::from(ids);
        let doc_id_arr = StringArray::from(doc_ids);
        let doc_name_arr = StringArray::from(doc_names);
        let idx_arr = Int64Array::from(chunk_idx);
        let text_arr = StringArray::from(texts);
        let emb_arr = FixedSizeListArray::new(
            Arc::new(Field::new("item", DataType::Float32, false)),
            dim as i32,
            Arc::new(Float32Array::from(flat)) as ArrayRef,
            None,
        );

        // tags: List<Utf8>, one list per chunk (the doc's tags).
        let mut tags_builder = ListBuilder::new(StringBuilder::new());
        for c in chunks {
            for tag in &c.tags {
                tags_builder.values().append_value(tag);
            }
            tags_builder.append(true);
        }
        let tags_arr = tags_builder.finish();

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(id_arr) as ArrayRef,
                Arc::new(doc_id_arr),
                Arc::new(doc_name_arr),
                Arc::new(idx_arr),
                Arc::new(text_arr),
                Arc::new(emb_arr),
                Arc::new(tags_arr),
            ],
        )
        .map_err(|e| anyhow!("build record batch: {}", e))?;

        let reader: Box<dyn RecordBatchReader + Send> =
            Box::new(RecordBatchIterator::new(vec![Ok(batch)], schema));
        let table = self
            .conn
            .open_table(TABLE_NAME)
            .execute()
            .await
            .map_err(|e| anyhow!("open table: {}", e))?;
        table
            .add(reader)
            .execute()
            .await
            .map_err(|e| anyhow!("add chunks: {}", e))?;
        Ok(())
    }

    /// Vector nearest-neighbor search. Returns up to `limit` hits sorted by
    /// distance (closest first). Uses **cosine** distance (magnitude-invariant)
    /// so ranking reflects semantic direction, not vector magnitude - this
    /// matters for models whose embeddings aren't L2-normalized (e.g. LFM2);
    /// with L2 distance, small-magnitude docs would rank high regardless of
    /// relevance. There's no vector index on this table (only a LabelList index
    /// on `tags`), so this is a brute-force cosine scan.
    pub async fn search(&self, query: &[f32], limit: usize) -> Result<Vec<SearchHit>> {
        let table = self
            .conn
            .open_table(TABLE_NAME)
            .execute()
            .await
            .map_err(|e| anyhow!("open table: {}", e))?;
        let mut stream = table
            .query()
            .nearest_to(query)
            .map_err(|e| anyhow!("nearest_to: {}", e))?
            .distance_type(lancedb::DistanceType::Cosine)
            .limit(limit)
            .execute()
            .await
            .map_err(|e| anyhow!("execute search: {}", e))?;

        let mut hits = Vec::new();
        while let Some(batch) = stream
            .try_next()
            .await
            .map_err(|e| anyhow!("read search batch: {}", e))?
        {
            let n = batch.num_rows();
            if n == 0 {
                continue;
            }
            let doc_ids = col::<StringArray>(&batch, "doc_id");
            let doc_names = col::<StringArray>(&batch, "doc_name");
            let chunk_idx = col::<Int64Array>(&batch, "chunk_index");
            let texts = col::<StringArray>(&batch, "chunk_text");
            let dists = col::<Float32Array>(&batch, "_distance");
            let tags = col::<ListArray>(&batch, "tags");
            for i in 0..n {
                hits.push(SearchHit {
                    doc_id: doc_ids.value(i).to_string(),
                    doc_name: doc_names.value(i).to_string(),
                    chunk_index: chunk_idx.value(i),
                    chunk_text: texts.value(i).to_string(),
                    distance: dists.value(i),
                    tags: extract_tags(tags.value(i)),
                });
            }
        }
        Ok(hits)
    }

    /// Read all chunks of a document, WITH their embeddings (so the caller can
    /// re-insert them with new tags without re-running the embedding model).
    pub async fn read_chunks_by_doc(&self, doc_id: &str) -> Result<Vec<ChunkRecord>> {
        let table = self
            .conn
            .open_table(TABLE_NAME)
            .execute()
            .await
            .map_err(|e| anyhow!("open table: {}", e))?;
        let filter = format!("doc_id = '{}'", doc_id.replace('\'', "''"));
        let mut stream = table
            .query()
            .only_if(&filter)
            .execute()
            .await
            .map_err(|e| anyhow!("read chunks: {}", e))?;
        let mut out = Vec::new();
        while let Some(batch) = stream
            .try_next()
            .await
            .map_err(|e| anyhow!("read chunks batch: {}", e))?
        {
            let n = batch.num_rows();
            if n == 0 {
                continue;
            }
            let ids = col::<StringArray>(&batch, "id");
            let doc_ids = col::<StringArray>(&batch, "doc_id");
            let doc_names = col::<StringArray>(&batch, "doc_name");
            let chunk_idx = col::<Int64Array>(&batch, "chunk_index");
            let texts = col::<StringArray>(&batch, "chunk_text");
            let embs = col::<FixedSizeListArray>(&batch, "embedding");
            for i in 0..n {
                let cell = embs.value(i);
                let embedding = cell
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .map(|a| a.values().to_vec())
                    .unwrap_or_default();
                out.push(ChunkRecord {
                    chunk_id: ids.value(i).to_string(),
                    doc_id: doc_ids.value(i).to_string(),
                    doc_name: doc_names.value(i).to_string(),
                    chunk_index: chunk_idx.value(i),
                    chunk_text: texts.value(i).to_string(),
                    embedding,
                });
            }
        }
        Ok(out)
    }

    /// Delete all chunks belonging to `doc_id`.
    pub async fn delete_by_doc(&self, doc_id: &str) -> Result<()> {
        let table = self
            .conn
            .open_table(TABLE_NAME)
            .execute()
            .await
            .map_err(|e| anyhow!("open table: {}", e))?;
        let filter = format!("doc_id = '{}'", doc_id.replace('\'', "''"));
        table
            .delete(&filter)
            .await
            .map_err(|e| anyhow!("delete chunks: {}", e))?;
        Ok(())
    }

    /// Keyword search: return chunks whose `chunk_text` contains any of the
    /// query terms (case-insensitive LIKE). `distance` is 0.0 (the service
    /// scores keyword hits by term frequency, not by vector distance).
    pub async fn keyword_search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let terms: Vec<&str> = query
            .split_whitespace()
            .filter(|t| !t.is_empty())
            .collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        // Build a case-insensitive OR filter, one LIKE per term. Single-quote
        // is escaped; %/_ are left as-is (rare in search terms).
        let clauses: Vec<String> = terms
            .iter()
            .map(|t| format!("lower(chunk_text) LIKE lower('%{}%')", t.replace('\'', "''")))
            .collect();
        let filter = clauses.join(" OR ");

        let table = self
            .conn
            .open_table(TABLE_NAME)
            .execute()
            .await
            .map_err(|e| anyhow!("open table: {}", e))?;
        let mut stream = table
            .query()
            .only_if(&filter)
            .limit(limit)
            .execute()
            .await
            .map_err(|e| anyhow!("execute keyword search: {}", e))?;

        let mut hits = Vec::new();
        while let Some(batch) = stream
            .try_next()
            .await
            .map_err(|e| anyhow!("read keyword batch: {}", e))?
        {
            let n = batch.num_rows();
            if n == 0 {
                continue;
            }
            let doc_ids = col::<StringArray>(&batch, "doc_id");
            let doc_names = col::<StringArray>(&batch, "doc_name");
            let chunk_idx = col::<Int64Array>(&batch, "chunk_index");
            let texts = col::<StringArray>(&batch, "chunk_text");
            let tags = col::<ListArray>(&batch, "tags");
            for i in 0..n {
                hits.push(SearchHit {
                    doc_id: doc_ids.value(i).to_string(),
                    doc_name: doc_names.value(i).to_string(),
                    chunk_index: chunk_idx.value(i),
                    chunk_text: texts.value(i).to_string(),
                    distance: 0.0,
                    tags: extract_tags(tags.value(i)),
                });
            }
        }
        Ok(hits)
    }

    /// Reclaim disk space held by deleted rows. LanceDB is append-only, so
    /// `delete` only creates a new dataset version that omits the rows - the
    /// old versions, which still contain the deleted embeddings, stay on disk
    /// until a prune runs. This prunes everything except the current version
    /// (`older_than = 0`), so the bytes are actually freed right away rather
    /// than lingering for the default 7-day window.
    ///
    /// `delete_unverified = true` is safe here: the desktop app is
    /// single-process and the caller holds the runtime lock (or a dedicated
    /// short-lived connection), so no concurrent transaction can be
    /// referencing the old versions we prune.
    pub async fn optimize(&self) -> Result<()> {
        let table = self
            .conn
            .open_table(TABLE_NAME)
            .execute()
            .await
            .map_err(|e| anyhow!("open table: {}", e))?;
        table
            .optimize(lancedb::table::OptimizeAction::Prune {
                older_than: Some(chrono::Duration::zero()),
                delete_unverified: Some(true),
                error_if_tagged_old_versions: None,
            })
            .await
            .map_err(|e| anyhow!("optimize (prune): {}", e))?;
        Ok(())
    }

    /// Drop the table entirely (used on reset).
    pub async fn drop_table(&self) -> Result<()> {
        self.conn
            .drop_table(TABLE_NAME, &[])
            .await
            .map_err(|e| anyhow!("drop table: {}", e))?;
        Ok(())
    }
}

fn chunk_schema(embed_dim: usize) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("doc_id", DataType::Utf8, false),
        Field::new("doc_name", DataType::Utf8, false),
        Field::new("chunk_index", DataType::Int64, false),
        Field::new("chunk_text", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                embed_dim as i32,
            ),
            false,
        ),
        // tags: List<Utf8>, nullable (empty list when a doc has no tags). The
        // inner Utf8 field MUST be nullable=true to match `ListBuilder::new(StringBuilder)`,
        // whose default inner field is nullable. A non-null inner here makes
        // `RecordBatch::try_new` reject the builder's output with
        // "column types must match schema types, expected List (non-null Utf8)
        // but found List(Utf8)".
        Field::new(
            "tags",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            true,
        ),
    ]))
}

/// Borrow a column as a concrete array type. Panics only if the result schema
/// is malformed (shouldn't happen with our fixed schema).
fn col<'a, T: Array + 'static>(batch: &'a RecordBatch, name: &str) -> &'a T {
    let arr = batch
        .column_by_name(name)
        .unwrap_or_else(|| panic!("missing column {}", name));
    arr.as_any()
        .downcast_ref::<T>()
        .unwrap_or_else(|| panic!("column {} wrong type", name))
}

/// Read a single `List<Utf8>` cell back into a `Vec<String>` (empty if the
/// cell is null/missing). `ListArray::value(i)` returns the cell's inner
/// array - a `StringArray` for `List<Utf8>` - so we downcast to that, not to
/// `ListArray` (the previous code downcasted to `ListArray` and always
/// failed, which silently dropped every doc's tags and broke tag filtering).
fn extract_tags(cell: ArrayRef) -> Vec<String> {
    if let Some(arr) = cell.as_any().downcast_ref::<StringArray>() {
        (0..arr.len()).map(|i| arr.value(i).to_string()).collect()
    } else {
        Vec::new()
    }
}
