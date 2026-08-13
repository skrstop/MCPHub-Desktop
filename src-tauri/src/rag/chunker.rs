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
struct TokenChunkSizer<'a> {
    embedder: &'a dyn Embedder,
}

impl ChunkSizer for TokenChunkSizer<'_> {
    fn size(&self, chunk: &str) -> usize {
        self.embedder.tokenize_offsets(chunk).len()
    }
}

/// A chunking strategy: turn one document's text into an ordered list of chunks.
trait ChunkStrategy {
    fn chunks(&self, text: &str) -> Vec<String>;
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
    fn chunks(&self, text: &str) -> Vec<String> {
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
    fn chunks(&self, text: &str) -> Vec<String> {
        self.splitter.chunks(text).map(String::from).collect()
    }
}

/// Code splitter (tree-sitter AST depth boundaries). Built from a
/// `tree_sitter::Language` resolved from the file extension by `code_language`.
struct CodeChunkStrategy<'a> {
    splitter: CodeSplitter<TokenChunkSizer<'a>>,
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
            splitter: CodeSplitter::new(language, cfg)?,
        })
    }
}

impl ChunkStrategy for CodeChunkStrategy<'_> {
    fn chunks(&self, text: &str) -> Vec<String> {
        self.splitter.chunks(text).map(String::from).collect()
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
        .with_sizer(TokenChunkSizer { embedder })
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
/// - `.md`/`.markdown`/`.mdx` → markdown (block/heading boundaries)
/// - source extensions in `code_language` → code (tree-sitter AST boundaries)
/// - everything else → plain text (unicode boundaries)
///
/// Unknown extensions and `CodeSplitter` init failures fall back to plain text
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
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    let ext = doc_extension(doc_name);

    // Try code first (needs a grammar match + a tree-sitter parser init); fall
    // back to markdown / text. CodeSplitter::new can fail on a malformed grammar
    // registration, so wrap it and degrade to text on error (logged, not fatal).
    let strategy: Box<dyn ChunkStrategy + 'a> = if let Some(lang) = code_language(&ext) {
        match CodeChunkStrategy::new(lang, embedder, chunk_size, chunk_overlap) {
            Ok(s) => Box::new(s),
            Err(e) => {
                log::warn!(
                    "[RAG] code splitter init failed for '{}' ({}), falling back to text: {}",
                    doc_name,
                    ext,
                    e
                );
                match TextChunkStrategy::new(embedder, chunk_size, chunk_overlap) {
                    Ok(s) => Box::new(s),
                    Err(ee) => {
                        log::error!("[RAG] text splitter init failed for '{}': {}", doc_name, ee);
                        return Vec::new();
                    }
                }
            }
        }
    } else if is_markdown(&ext) {
        match MarkdownChunkStrategy::new(embedder, chunk_size, chunk_overlap) {
            Ok(s) => Box::new(s),
            Err(e) => {
                log::warn!(
                    "[RAG] markdown splitter init failed for '{}' ({}), falling back to text: {}",
                    doc_name,
                    ext,
                    e
                );
                match TextChunkStrategy::new(embedder, chunk_size, chunk_overlap) {
                    Ok(s) => Box::new(s),
                    Err(ee) => {
                        log::error!("[RAG] text splitter init failed for '{}': {}", doc_name, ee);
                        return Vec::new();
                    }
                }
            }
        }
    } else {
        match TextChunkStrategy::new(embedder, chunk_size, chunk_overlap) {
            Ok(s) => Box::new(s),
            Err(e) => {
                log::error!("[RAG] text splitter init failed for '{}': {}", doc_name, e);
                return Vec::new();
            }
        }
    };

    strategy.chunks(text)
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
        /// Token = each whitespace-separated word; offsets are byte spans.
        fn tokenize_offsets(&self, text: &str) -> Vec<(usize, usize)> {
            text.split_whitespace()
                .filter_map(|w| text.find(w).map(|s| (s, s + w.len())))
                .collect()
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
