//! Chunking strategy: split a document into embeddable chunks using the
//! [`text-splitter`](https://crates.io/crates/text-splitter) crate, dispatched
//! by document type (text / markdown / code) — the **strategy pattern**.
//!
//! Three strategies, each backed by a text-splitter type:
//! - `TextChunkStrategy` → `TextSplitter`: unicode word/sentence/newline
//!   boundaries. The default for prose, config, logs, JSON, etc.
//! - `MarkdownChunkStrategy` → `MarkdownSplitter`: CommonMark block/heading
//!   boundaries (better semantic chunks for `.md` uploads).
//! - `CodeChunkStrategy` → `CodeSplitter`: tree-sitter AST depth boundaries
//!   (splits on function/class/statement nodes, not mid-expression).
//!
//! All three are generic over a `ChunkSizer`; we supply `TokenChunkSizer`, which
//! sizes chunks in **tokens** via the loaded model's `Embedder::tokenize_offsets`
//! (so `chunk_size` maps directly to the model's context budget — same unit the
//! old `chunk_text` used). This keeps the sizing token-accurate WITHOUT pulling
//! text-splitter's `tokenizers` feature (which would enable tokenizers' `onig`
//! Oniguruma C dep that this project deliberately avoids — see `Cargo.toml`).
//!
//! The factory `chunk_document` picks the strategy from `doc_name`'s extension
//! and runs it. Unknown extensions fall back to `TextSplitter` (never errors the
//! import over a splitter init). `chunk_overlap` is clamped to `chunk_size-1`
//! (text-splitter rejects `overlap >= capacity`); empty/whitespace chunks are
//! dropped by text-splitter when `trim=true` (parity with the old `chunk_text`).

use anyhow::Result;
use text_splitter::{
    ChunkConfig, ChunkSizer, CodeSplitter, MarkdownSplitter, TextSplitter,
};
use tree_sitter::Language;

use crate::rag::embedder::Embedder;

/// Sizer that sizes a candidate chunk in **tokens**, using the loaded model's
/// own tokenizer. text-splitter calls `size` repeatedly as it walks splitter
/// boundaries; each call tokenizes only the small candidate substring, so the
/// total cost stays roughly linear in the document length.
///
/// Fast-reject: semantic level probing repeatedly sizes prefixes that are
/// orders of magnitude above capacity (a minified JS parses to ONE AST node
/// spanning the whole file, so the depth-1 "first chunk" is megabytes).
/// Tokenizing such prefixes dominates the whole chunking pass (observed:
/// 12+ hours on a 3.3MB file). Since tokens >= len / MAX_TOKEN_BYTES, any
/// prefix longer than `capacity * MAX_TOKEN_BYTES` provably exceeds capacity —
/// return an over-capacity sentinel without tokenizing. Candidate-sized
/// prefixes (the ones that decide real boundaries) are still tokenized
/// exactly, so chunk boundaries are identical to the unoptimized path.
struct TokenChunkSizer<'a> {
    embedder: &'a dyn Embedder,
    /// Chunk capacity in tokens, captured from `make_config` so `size` can
    /// apply the fast-reject bound.
    capacity: usize,
}

/// Ultra-conservative upper bound on the byte length of one tokenizer token.
/// Byte-fallback tokenizers emit 1-byte tokens (lower bound 1 token/byte);
/// real vocab tokens for text/code stay well under ~32 bytes even for
/// multi-byte scripts. 64 makes the "provably over capacity" claim safe for
/// any tokenizer in practical use.
const MAX_TOKEN_BYTES: usize = 64;

/// Upper bound (bytes) on the text handed to `CodeSplitter` in ONE pass.
/// Larger code files are pre-partitioned at AST node boundaries
/// (`CodeChunkStrategy::partition_offsets`) and each partition is chunked
/// independently. Rationale: the splitter's per-chunk search re-walks its
/// whole range tree, so a multi-MB deep AST costs O(n²) size() probes — a
/// real 3.3MB minified bundle (3587 lines, several >100KB lines) ran for
/// HOURS with the production BPE tokenizer (each ≤32KB probe ~ms; the ~µs
/// test stub hid the cost). Partitioning bounds the per-pass range tree while
/// keeping every chunk boundary at an AST node. 64KB keeps a partition's
/// subtree deep enough for meaningful statement-level chunking while cutting
/// a 3.3MB bundle into ~50 fast passes. Release-measured on mermaid.min.js:
/// 256KB → 47.6s, 64KB → 37.3s (overlap=100; 16.5s with overlap=0) —
/// diminishing returns below 256KB because the residual cost is the
/// splitter's fixed per-partition walk, not probe prefix length. 64KB kept:
/// best measured point without shrinking partition subtrees into noise.
const CODE_PARTITION_BYTES: usize = 64 * 1024;

impl ChunkSizer for TokenChunkSizer<'_> {
    fn size(&self, chunk: &str) -> usize {
        let len = chunk.len();
        if len > self.capacity.saturating_mul(MAX_TOKEN_BYTES) {
            // Provably cannot fit within capacity (tokens >= len/64 > capacity).
            // Any value > capacity works with text-splitter's Ordering-based
            // fits check; keep it far above to also exceed any real capacity.
            // Critical for minified/one-giant-node code files: without this
            // early-out, text-splitter's binary search repeatedly tokenizes
            // multi-MB candidates that can never fit — measured >19 min for a
            // 1.8MB minified file (the "ran overnight" root cause).
            return self.capacity.saturating_mul(1_000_000);
        }
        self.embedder.tokenize_offsets(chunk).len()
    }
}

/// A chunking strategy: turn one document's text into an ordered list of chunks.
///
/// `on_progress(chunking_chars_done, chunking_chars_total)` is fired by
/// strategies that process large inputs in observable stages — currently the
/// code strategy, once per AST partition on the >64KB path (the phase the UI
/// shows as "分片中…"). Single-pass strategies ignore it.
trait ChunkStrategy {
    fn chunks(
        &self,
        text: &str,
        on_progress: Option<&dyn Fn(u64, u64)>,
    ) -> Vec<String>;
}

/// Plain-text splitter (unicode word/sentence/newline boundaries).
struct TextChunkStrategy<'a> {
    splitter: TextSplitter<TokenChunkSizer<'a>>,
}

impl<'a> TextChunkStrategy<'a> {
    fn new(embedder: &'a dyn Embedder, chunk_size: u32, chunk_overlap: u32) -> Result<Self> {
        let cfg = make_config(embedder, chunk_size, chunk_overlap)?;
        Ok(Self {
            splitter: TextSplitter::new(cfg),
        })
    }
}

impl ChunkStrategy for TextChunkStrategy<'_> {
    fn chunks(&self, text: &str, _on_progress: Option<&dyn Fn(u64, u64)>) -> Vec<String> {
        self.splitter.chunks(text).map(String::from).collect()
    }
}

/// Markdown splitter (CommonMark block/heading boundaries).
struct MarkdownChunkStrategy<'a> {
    splitter: MarkdownSplitter<TokenChunkSizer<'a>>,
}

impl<'a> MarkdownChunkStrategy<'a> {
    fn new(embedder: &'a dyn Embedder, chunk_size: u32, chunk_overlap: u32) -> Result<Self> {
        let cfg = make_config(embedder, chunk_size, chunk_overlap)?;
        Ok(Self {
            splitter: MarkdownSplitter::new(cfg),
        })
    }
}

impl ChunkStrategy for MarkdownChunkStrategy<'_> {
    fn chunks(&self, text: &str, _on_progress: Option<&dyn Fn(u64, u64)>) -> Vec<String> {
        self.splitter.chunks(text).map(String::from).collect()
    }
}

/// Code splitter (tree-sitter AST depth boundaries via `text_splitter::CodeSplitter`).
/// Built from a `tree_sitter::Language` resolved from the file extension by
/// `code_language`. Large files (> `CODE_PARTITION_BYTES`) are pre-partitioned
/// at AST node boundaries before the splitter runs — see `CODE_PARTITION_BYTES`
/// — which bounds the library's super-linear per-chunk search cost. Pathological
/// single-giant-node protection (minified JS etc.) comes from `TokenChunkSizer`'s
/// provable over-capacity fast-reject.
struct CodeChunkStrategy<'a> {
    splitter: CodeSplitter<TokenChunkSizer<'a>>,
    language: Language,
}

impl<'a> CodeChunkStrategy<'a> {
    fn new(
        language: Language,
        embedder: &'a dyn Embedder,
        chunk_size: u32,
        chunk_overlap: u32,
    ) -> Result<Self> {
        let cfg = make_config(embedder, chunk_size, chunk_overlap)?;
        Ok(Self {
            splitter: CodeSplitter::new(language.clone(), cfg)?,
            language,
        })
    }

    /// Byte ranges tiling `text` at AST node boundaries, each roughly ≤
    /// `CODE_PARTITION_BYTES`. Descends only while a node exceeds the bound,
    /// so atoms are the shallowest subtrees that fit; gaps between children
    /// (whitespace, punctuation) become gap atoms so the ranges tile the
    /// whole input with no byte lost. `None` when the grammar fails to parse.
    fn partition_offsets(&self, text: &str) -> Option<Vec<(usize, usize)>> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&self.language).ok()?;
        let tree = parser.parse(text, None)?;
        let mut atoms: Vec<(usize, usize)> = Vec::new();
        push_atoms(tree.root_node(), CODE_PARTITION_BYTES, &mut atoms);
        if atoms.is_empty() {
            return None;
        }
        // Greedily pack consecutive atoms into partitions that stay under the
        // bound. A single oversized atom (giant leaf, e.g. a huge string
        // literal) becomes its own partition — the splitter's fallback levels
        // + the oversized-chunk safety valve handle it downstream.
        let mut parts: Vec<(usize, usize)> = Vec::new();
        let (mut start, mut end) = atoms[0];
        for &(s, e) in &atoms[1..] {
            if e - start > CODE_PARTITION_BYTES {
                parts.push((start, end));
                start = s;
            }
            end = e;
        }
        parts.push((start, end));
        Some(parts)
    }
}

/// Depth-first atom collection for `partition_offsets`: emit `(start, end)`
/// for the node itself when it fits the bound or has no children; otherwise
/// recurse into children, materializing inter-child gaps as atoms so the
/// ranges cover the parent exactly.
fn push_atoms(node: tree_sitter::Node, bound: usize, out: &mut Vec<(usize, usize)>) {
    let (s, e) = (node.start_byte(), node.end_byte());
    if e <= s {
        return;
    }
    if e - s <= bound || node.child_count() == 0 {
        out.push((s, e));
        return;
    }
    let mut covered = s;
    for i in 0..node.child_count() {
        let Some(child) = node.child(i as u32) else { continue };
        if child.start_byte() > covered {
            out.push((covered, child.start_byte()));
        }
        push_atoms(child, bound, out);
        covered = covered.max(child.end_byte());
    }
    if covered < e {
        out.push((covered, e));
    }
}

impl ChunkStrategy for CodeChunkStrategy<'_> {
    fn chunks(&self, text: &str, on_progress: Option<&dyn Fn(u64, u64)>) -> Vec<String> {
        // Small files: single pass, byte-for-byte the historical behavior.
        if text.len() <= CODE_PARTITION_BYTES {
            let out = self.splitter.chunks(text).map(String::from).collect();
            if let Some(cb) = on_progress {
                let total = text.chars().count() as u64;
                cb(total, total);
            }
            return out;
        }
        // Large files: the splitter's per-chunk search walks its whole range
        // tree repeatedly, so a multi-MB deep AST (minified bundles with many
        // statements on >100KB lines) costs O(n²) size() probes — measured
        // hours for a real 3.3MB bundle with the production BPE tokenizer
        // (each ≤32KB probe costs ~ms, vs ~µs for the test stub that hid this).
        // Pre-partition at AST statement boundaries first: every chunk still
        // splits at AST nodes, and per-pass cost drops by the partition count.
        // Trade-off: chunk_overlap applies within partitions only — partition
        // boundaries are statement ends, the same class as any chunk boundary.
        let parts = match self.partition_offsets(text) {
            Some(p) => p,
            None => {
                // Grammar failed to parse (or produced no atoms): fall back to
                // the historical single pass so an import never errors here.
                log::warn!("[RAG] AST partitioning failed; single-pass code chunking");
                let out = self.splitter.chunks(text).map(String::from).collect();
                if let Some(cb) = on_progress {
                    let total = text.chars().count() as u64;
                    cb(total, total);
                }
                return out;
            }
        };
        let mut out: Vec<String> = Vec::new();
        // Chunking-phase progress: report cumulative scanned chars after each
        // partition. Partitions tile the input exactly (gap atoms included), so
        // the sum reaches `total` on the last tick — the UI's "分片中… N%" fill.
        let total = text.chars().count() as u64;
        let mut done: u64 = 0;
        for (s, e) in parts {
            // tree-sitter byte offsets are always UTF-8 char boundaries.
            debug_assert!(text.is_char_boundary(s) && text.is_char_boundary(e));
            let part = &text[s..e];
            out.extend(self.splitter.chunks(part).map(String::from));
            if let Some(cb) = on_progress {
                done += part.chars().count() as u64;
                cb(done.min(total), total);
            }
        }
        out
    }
}

/// Build the shared `ChunkConfig` for all three strategies: capacity in tokens
/// (via `TokenChunkSizer`), overlap clamped to `capacity-1`, trim whitespace at
/// chunk edges (parity with the old `chunk_text` which trimmed + skipped empty
/// chunks). `with_overlap` returns `Err` when `overlap >= capacity`; the clamp
/// guarantees it never fires here.
fn make_config<'a>(
    embedder: &'a dyn Embedder,
    chunk_size: u32,
    chunk_overlap: u32,
) -> Result<ChunkConfig<TokenChunkSizer<'a>>> {
    let capacity = chunk_size.max(1) as usize;
    let overlap = clamp_overlap(capacity, chunk_overlap as usize);
    Ok(ChunkConfig::new(capacity)
        .with_sizer(TokenChunkSizer { embedder, capacity })
        .with_overlap(overlap)?
        .with_trim(true))
}

/// `overlap.min(capacity - 1)` so text-splitter's `with_overlap` (which rejects
/// `overlap >= capacity`) never errors. Mirrors the frontend clamp in
/// `RagPage.tsx` (`chunkOverlap > chunkSize - 1` → `chunkSize - 1`).
fn clamp_overlap(capacity: usize, overlap: usize) -> usize {
    overlap.min(capacity.saturating_sub(1))
}

/// Resolve a `tree_sitter::Language` from a lowercased extension (with the
/// leading dot), or `None` if the extension isn't a bundled grammar (caller
/// falls back to `TextSplitter`). Grammars are the `tree-sitter-<lang>` crates
/// from `Cargo.toml`; each exposes a `LANGUAGE` constant (`LanguageFn` →
/// `Language` via `Into`).
fn code_language(ext: &str) -> Option<Language> {
    // Each grammar's `LANGUAGE` is a `tree_sitter::LanguageFn`; `Into<Language>`
    // works in tree-sitter 0.26.
    let lang: Language = match ext {
        ".rs" => tree_sitter_rust::LANGUAGE.into(),
        ".py" => tree_sitter_python::LANGUAGE.into(),
        ".js" | ".mjs" | ".cjs" => tree_sitter_javascript::LANGUAGE.into(),
        ".ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        ".tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        ".go" => tree_sitter_go::LANGUAGE.into(),
        ".java" => tree_sitter_java::LANGUAGE.into(),
        ".c" | ".h" => tree_sitter_c::LANGUAGE.into(),
        ".cpp" | ".cc" | ".cxx" | ".hpp" | ".hh" => tree_sitter_cpp::LANGUAGE.into(),
        _ => return None,
    };
    Some(lang)
}

/// Whether `ext` (lowercased, with dot) is a markdown file.
fn is_markdown(ext: &str) -> bool {
    matches!(ext, ".md" | ".markdown" | ".mdx")
}

/// Lowercase extension (with leading dot) of `doc_name`, or `""` if none.
fn doc_extension(doc_name: &str) -> String {
    let lower = doc_name.to_lowercase();
    match lower.rfind('.') {
        Some(dot) => lower[dot..].to_string(),
        None => String::new(),
    }
}

/// Split `text` into chunks for the document named `doc_name`, sized in tokens
/// (the loaded model's unit) with `chunk_overlap` tokens of overlap. Picks the
/// chunking strategy from the file extension:
/// - extraction products routed by the PRODUCED content: PDF/Office →
///   markdown (block/heading boundaries); image (plain OCR text) → text
/// - `.md`/`.markdown`/`.mdx` → markdown (block/heading boundaries)
/// - source extensions in `code_language` → code (tree-sitter AST boundaries)
/// - everything else → plain text (unicode boundaries)
///
/// `CodeSplitter` init failures fall back to plain text
/// (logged at warn) so an import never errors over a splitter init. Empty /
/// whitespace-only input returns `Vec::new()` (text-splitter drops empties when
/// `trim=true`, matching the old `chunk_text`). Holds a `&'a dyn Embedder`
/// borrow — the caller (`reindex_doc`) holds the runtime lock for the whole
/// batch, so the borrow outlives this call.
pub fn chunk_document<'a>(
    doc_name: &str,
    text: &str,
    embedder: &'a dyn Embedder,
    chunk_size: u32,
    chunk_overlap: u32,
) -> Vec<String> {
    chunk_document_inner(doc_name, text, embedder, chunk_size, chunk_overlap, None)
}

/// `chunk_document` with a chunking-phase progress callback
/// `on_progress(chars_scanned, total_chars)`, fired per code AST partition on
/// the >64KB path (tests / single-pass strategies never see it). The service
/// layer routes this to `rag://upload-progress` so the UI can show a fill for
/// the otherwise-static "分片中…" phase.
pub fn chunk_document_with_progress<'a>(
    doc_name: &str,
    text: &str,
    embedder: &'a dyn Embedder,
    chunk_size: u32,
    chunk_overlap: u32,
    on_progress: &dyn Fn(u64, u64),
) -> Vec<String> {
    chunk_document_inner(
        doc_name,
        text,
        embedder,
        chunk_size,
        chunk_overlap,
        Some(on_progress),
    )
}

fn chunk_document_inner<'a>(
    doc_name: &str,
    text: &str,
    embedder: &'a dyn Embedder,
    chunk_size: u32,
    chunk_overlap: u32,
    on_progress: Option<&dyn Fn(u64, u64)>,
) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    let ext = doc_extension(doc_name);

    let strategy = semantic_strategy(&ext, embedder, chunk_size, chunk_overlap, doc_name)
        .unwrap_or_else(|| {
            // Unknown extension or splitter init failure: plain text fallback
            // (init failures are logged inside `semantic_strategy`).
            match TextChunkStrategy::new(embedder, chunk_size, chunk_overlap) {
                Ok(s) => Box::new(s) as Box<dyn ChunkStrategy + 'a>,
                Err(e) => {
                    log::error!("[RAG] text splitter init failed for '{}': {}", doc_name, e);
                    Box::new(EmptyChunkStrategy) as Box<dyn ChunkStrategy + 'a>
                }
            }
        });
    // Large code files are partitioned at AST boundaries inside the strategy
    // (see CODE_PARTITION_BYTES), which bounds the splitter's super-linear
    // search cost per pass. Keep a traceability log for big code imports.
    if text.len() > 1024 * 1024 && code_language(&ext).is_some() {
        log::info!(
            "[RAG] doc '{}' is {} bytes of source code; AST-partitioned into passes of ≤{} bytes",
            doc_name,
            text.len(),
            CODE_PARTITION_BYTES
        );
    }
    let chunks = strategy.chunks(text, on_progress);
    // Safety valve (NOT a chunking strategy): text-splitter's "at least one
    // section" guarantee can return a single OVERSIZED chunk when a semantic
    // section has no internal boundaries (e.g. a one-giant-node minified line).
    // Such a chunk survives the fast-reject (the sizer only reports over-
    // capacity; the library keeps the whole section). It embeds fine (the
    // embedder truncates to max_context) but would be STORED whole — retrieval
    // would push megabytes into the UI. Hard-split those rare chunks into
    // char-boundary windows of `capacity * MAX_TOKEN_BYTES` bytes.
    split_oversized_chunks(chunks, oversized_window_bytes(chunk_size))
}

/// Window size for the oversized-chunk safety valve.
fn oversized_window_bytes(chunk_size: u32) -> usize {
    chunk_size.max(1) as usize * MAX_TOKEN_BYTES
}

/// Split chunks larger than `max_bytes` into char-boundary windows (keeps all
/// content; windows never split a UTF-8 code point). Normal chunking output
/// (chunks ≤ capacity tokens) is untouched.
fn split_oversized_chunks(chunks: Vec<String>, max_bytes: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        if chunk.len() <= max_bytes {
            out.push(chunk);
            continue;
        }
        let mut start = 0usize;
        while start < chunk.len() {
            let mut end = (start + max_bytes).min(chunk.len());
            while end < chunk.len() && !chunk.is_char_boundary(end) {
                end += 1;
            }
            out.push(chunk[start..end].to_string());
            start = end;
        }
    }
    out
}

/// A strategy that always yields no chunks. Only constructed when even the
/// plain-text splitter fails to init (previously an early `return Vec::new()`).
struct EmptyChunkStrategy;
impl ChunkStrategy for EmptyChunkStrategy {
    fn chunks(&self, _text: &str, _on_progress: Option<&dyn Fn(u64, u64)>) -> Vec<String> {
        Vec::new()
    }
}

/// Build the semantic strategy for an extension: code (tree-sitter) for source
/// extensions, markdown for `.md*`, `None` for everything else. Splitter init
/// failures degrade to `None` with a warn log (caller falls back to text).
fn semantic_strategy<'a>(
    ext: &str,
    embedder: &'a dyn Embedder,
    chunk_size: u32,
    chunk_overlap: u32,
    doc_name: &str,
) -> Option<Box<dyn ChunkStrategy + 'a>> {
    // Extraction products (PDF/Office/image) first: route by the PRODUCED
    // content (2026-09 requirement) — PDF/Office extraction yields Markdown
    // (stored as `{id}.md`) → markdown splitter; image OCR yields plain text
    // → text splitter.
    if super::extract::produces_markdown(doc_name) {
        return match MarkdownChunkStrategy::new(embedder, chunk_size, chunk_overlap) {
            Ok(s) => Some(Box::new(s)),
            Err(e) => {
                log::warn!(
                    "[RAG] markdown splitter init failed for extracted '{}' ({}), falling back to text: {}",
                    doc_name,
                    ext,
                    e
                );
                None
            }
        };
    }
    if let Some(lang) = code_language(ext) {
        return match CodeChunkStrategy::new(lang, embedder, chunk_size, chunk_overlap) {
            Ok(s) => Some(Box::new(s)),
            Err(e) => {
                log::warn!(
                    "[RAG] code splitter init failed for '{}' ({}), falling back to text: {}",
                    doc_name,
                    ext,
                    e
                );
                None
            }
        };
    }
    if is_markdown(ext) {
        return match MarkdownChunkStrategy::new(embedder, chunk_size, chunk_overlap) {
            Ok(s) => Some(Box::new(s)),
            Err(e) => {
                log::warn!(
                    "[RAG] markdown splitter init failed for '{}' ({}), falling back to text: {}",
                    doc_name,
                    ext,
                    e
                );
                None
            }
        };
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rag::embedder::Embedder;

    /// A stub embedder that tokenizes on whitespace boundaries — so `size(chunk)`
    /// == word count. Lets us exercise the splitter pipeline without a real model.
    struct WhitespaceTokenizer;

    impl Embedder for WhitespaceTokenizer {
        fn embed(&mut self, _text: &str) -> anyhow::Result<Vec<f32>> {
            Ok(vec![])
        }
        fn embed_batch(&mut self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| Vec::new()).collect())
        }
        fn embed_dim(&self) -> usize {
            1
        }
        fn max_context(&self) -> u32 {
            2048
        }
        /// Token = fixed 8-char groups with SEQUENTIAL byte offsets. Deliberately
        /// NOT `split_whitespace().find()` — that stub had two flaws that blew up
        /// on boundary-free input: (a) `find` re-scans per word (O(n) per call),
        /// and (b) any whitespace-free candidate (however long) collapses to ONE
        /// token, creating a fits/non-fits cliff at the fast-reject boundary that
        /// sent text-splitter's binary search into char-level thrash (50KB "xxx…"
        /// -> 428s / 17k one-byte chunks in a test). A real tokenizer emits
        /// smoothly-growing token counts; this stub mirrors that (monotonic in
        /// candidate length), so regression tests measure the library, not the stub.
        fn tokenize_offsets(&self, text: &str) -> Vec<(usize, usize)> {
            const GROUP: usize = 8;
            let mut out = Vec::new();
            let mut start = 0usize;
            while start < text.len() {
                let mut end = (start + GROUP).min(text.len());
                while end < text.len() && !text.is_char_boundary(end) {
                    end += 1;
                }
                out.push((start, end));
                start = end;
            }
            out
        }
        fn ep_label(&self) -> &str {
            "stub"
        }
        fn backend(&self) -> &str {
            "stub"
        }
    }

    fn tok() -> &'static dyn Embedder {
        &WhitespaceTokenizer
    }

    /// Shape modeled on the real production hang (mermaid.min.js: 3.3MB,
    /// 3587 lines, 184 lines >1KB, longest 324KB): tens of thousands of
    /// statements packed onto a few very long lines — a DEEP multi-line AST
    /// over the partition bound. This shape is what made the unpartitioned
    /// CodeSplitter run for hours with the production BPE tokenizer (its
    /// per-chunk binary search re-sizes candidates ~ms each, and the
    /// per-chunk walk scales with the whole range tree).
    fn huge_multiline_bundle() -> String {
        let mut text = String::new();
        for line in 0..12 {
            for stmt in 0..2_000 {
                text.push_str(&format!(
                    "var v{}_{}=function(a){{return a+{}}};",
                    line, stmt, stmt
                ));
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn huge_multiline_bundle_partitions_and_tiles() {
        // AST pre-partitioning regression: the bundle is > CODE_PARTITION_BYTES
        // so `CodeChunkStrategy` slices it at statement boundaries first;
        // every chunk must still come out bounded and ALL content must
        // survive (partitions tile the input; edge-trim only drops whitespace).
        let text = huge_multiline_bundle();
        assert!(
            text.len() > CODE_PARTITION_BYTES,
            "test file must engage the partitioned path ({} bytes)",
            text.len()
        );
        let t = std::time::Instant::now();
        let chunks = chunk_document("bundle.min.js", &text, tok(), 512, 0);
        assert!(!chunks.is_empty());
        assert!(
            t.elapsed().as_secs() < 60,
            "partitioned chunking took {:?}",
            t.elapsed()
        );
        let strip = |s: &str| s.split_whitespace().collect::<String>();
        let joined = strip(&chunks.concat());
        let input = strip(&text);
        assert_eq!(joined, input, "content lost across partitions/chunks");
        assert!(chunks.iter().all(|c| c.len() <= oversized_window_bytes(512)));
    }

    #[test]
    fn minified_js_terminates_quickly() {
        // Regression guard for the production hang (3.3MB minified JS took
        // 12+ hours): TokenChunkSizer's fast-reject stops megabyte tokenizing
        // and the library's own fallback levels finish the job — NO
        // fixed-length windowing, NO degradation to plain text (rejected by
        // requirement). Size kept at a realistic scale: per-chunk library
        // work scales with remaining AST nodes (O(n²) worst case for deep
        // multi-line ASTs — 30k lines ≈ 5s; see chunk_documents >1MB warn),
        // while single-line minified bundles (the actual prod case) are fast
        // via the fast-reject path (see giant_single_line test).
        let text = "function a(){return 1};\n".repeat(30_000);
        let t = std::time::Instant::now();
        let chunks = chunk_document("mermaid.min.js", &text, tok(), 512, 100);
        assert!(!chunks.is_empty());
        assert!(t.elapsed().as_secs() < 120, "chunking took {:?}", t.elapsed());
        assert!(
            chunks.iter().all(|c| !c.starts_with("unction")),
            "chunk boundary split mid-token"
        );
    }

    #[test]
    fn giant_single_line_js_terminates_quickly() {
        // A single-line 600KB JS file (worst case for AST chunking: one line,
        // possibly one giant node). Must complete via the fast-reject + the
        // library's own fallback levels, without pathological time and
        // without any windowing.
        let text = "const a=1;".repeat(60_000);
        let t = std::time::Instant::now();
        let chunks = chunk_document("bundle.js", &text, tok(), 512, 100);
        assert!(!chunks.is_empty());
        assert!(t.elapsed().as_secs() < 30, "took {:?}", t.elapsed());
    }

    #[test]
    fn oversized_unsplittable_chunks_are_hard_split() {
        // Safety valve: a one-giant-node minified file can come back as ONE
        // chunk far over the capacity*64-byte window (text-splitter's
        // "at least one section" guarantee). The valve must split it into
        // bounded windows WITHOUT losing content (concatenation == input).
        let text = "x".repeat(200_000);
        // overlap=0 so windows are disjoint and content preservation is exact
        // (with overlap>0 chunks legitimately repeat text).
        let chunks = chunk_document("giant.min.js", &text, tok(), 512, 0);
        assert!(!chunks.is_empty());
        let max_win = 512 * 64; // oversized_window_bytes(512)
        assert!(
            chunks.iter().all(|c| c.len() <= max_win),
            "chunk over window: max={}",
            chunks.iter().map(|c| c.len()).max().unwrap()
        );
        let joined: String = chunks.concat();
        assert_eq!(joined, text, "content must be preserved exactly");
        // Multi-byte content: windows never split a UTF-8 code point.
        let cjk = "数".repeat(40_000); // 120_000 bytes, 3 bytes/char
        let cchunks = chunk_document("giant.min.js", &cjk, tok(), 512, 0);
        assert!(cchunks.iter().all(|c| c.is_char_boundary(0) && c.is_char_boundary(c.len())));
        assert_eq!(cchunks.concat(), cjk);
    }

    #[test]
    fn extracted_products_route_to_markdown_splitter() {
        // 2026-09 requirement: extraction products (PDF/Office) arrive as the
        // Markdown the extract layer PRODUCED — they must chunk on markdown
        // boundaries (each heading starts its own section), not as plain
        // text. Discriminator (asymmetric sections, capacity 12):
        // Stub sizes tokens as 8-char groups (capacity 12 ≈ 96 chars):
        // - markdown route: sections at heading level → a chunk STARTS with
        //   "# Beta" (the b-line is ~179 chars > 96, so section 2 also emits
        //   tail chunks — heading starts remain intact).
        // - plain-text route (same content under .txt): line-level fallback
        //   packs lines to ~96 chars → chunk 1 swallows "# Alpha" + a-line +
        //   "# Beta" + the first b-words → "# Beta" lands MID-chunk, and the
        //   b-line remainder starts chunk 2 — no chunk starts with "# Beta".
        let md = "# Alpha\n\na1 a2 a3 a4 a5 a6 a7 a8\n\n# Beta\n\nb1 b2 b3 b4 b5 b6 b7 b8 b9 b10 b11 b12 b13 b14 b15 b16 b17 b18 b19 b20 b21 b22 b23 b24 b25 b26 b27 b28 b29 b30 b31 b32 b33 b34 b35 b36 b37 b38 b39 b40 b41 b42 b43 b44 b45 b46 b47 b48 b49 b50 b51 b52 b53 b54 b55 b56 b57 b58 b59 b60";
        let extracted = chunk_document("report.pdf", md, tok(), 12, 0);
        assert!(
            extracted.iter().any(|c| c.starts_with("# Beta")),
            "extracted: {extracted:?}"
        );
        let plain = chunk_document("report.txt", md, tok(), 12, 0);
        assert!(
            plain.iter().all(|c| !c.starts_with("# Beta")),
            "plain: {plain:?}"
        );
    }

    #[test]
    fn empty_text_returns_no_chunks() {
        let chunks = chunk_document("foo.txt", "   \n  ", tok(), 4, 1);
        assert!(chunks.is_empty());
    }

    #[test]
    fn unknown_ext_uses_text_splitter_no_panic() {
        let chunks = chunk_document(
            "notes.xyz",
            "alpha beta gamma delta epsilon zeta eta theta",
            tok(),
            3,
            1,
        );
        assert!(!chunks.is_empty());
        // Every chunk is a substring of the input.
        for c in &chunks {
            assert!("alpha beta gamma delta epsilon zeta eta theta".contains(c));
        }
    }

    #[test]
    fn overlap_ge_size_clamps_without_panic() {
        // overlap == size would normally make text-splitter error; the clamp
        // brings it to size-1 so this must produce >=1 chunk, not panic.
        let chunks = chunk_document("x.txt", "one two three four", tok(), 2, 2);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn code_ext_engages_code_splitter() {
        // A rust file with two functions; CodeSplitter should split on AST
        // boundaries (function nodes), producing >1 chunk for a large-enough
        // capacity, and every chunk is a valid substring.
        let src = "fn alpha() { let x = 1; }\nfn beta() { let y = 2; }\nfn gamma() { let z = 3; }\n";
        let chunks = chunk_document("lib.rs", src, tok(), 4, 1);
        assert!(!chunks.is_empty());
        for c in &chunks {
            assert!(src.contains(c), "chunk not a substring: {:?}", c);
        }
    }

    #[test]
    fn markdown_splits_on_headings() {
        let md = "# Title\n\nintro paragraph here\n\n## Section A\n\nbody text\n\n## Section B\n\nmore body\n";
        let chunks = chunk_document("doc.md", md, tok(), 6, 1);
        assert!(!chunks.is_empty());
        for c in &chunks {
            assert!(md.contains(c));
        }
    }
}
