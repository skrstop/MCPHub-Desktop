//! Content extraction strategies for RAG document import.
//!
//! Follows the project's existing strategy pattern (see `chunker.rs`'s
//! `ChunkStrategy` trait + dispatch): a `ContentExtractor` trait, a static
//! priority-ordered registry, and a single dispatch entry (`run`) that
//! `service.rs` calls instead of hard-coding format handling.
//!
//! Strategies:
//! - `pdf`   — PDF -> Markdown via `pdf_oxide`; embedded images OCR'd and
//!   appended as quote blocks.
//! - `office`— DOCX/DOC/XLSX/XLS/PPTX/PPT -> Markdown via `office_oxide`
//!   (IR walk collects embedded images), OCR text appended.
//! - `image` — standalone images (png/jpg/...) via the shared OCR core.
//! - `text`  — plain-text fallback (the historical NUL-sniff + decode path).
//!
//! Error sentinels (checked by the frontend):
//! - `UNSUPPORTED_FORMAT: <name>` — not a supported document kind.
//! - `EXTRACT_FAILED: <reason>` — supported kind but extraction produced
//!   nothing usable (e.g. scanned PDF without a text layer).
//! - `OCR_MISSING: <detail>` — OCR engine unavailable on this platform.

mod image;
pub mod ocr;
mod office;
mod pdf;
mod text;

use anyhow::{anyhow, Result};

/// A content extraction strategy: turn one imported file's bytes into
/// Markdown text for the chunker -> embedding -> lancedb pipeline.
pub trait ContentExtractor: Send + Sync {
    /// Whether this strategy handles the file (by extension / sniffing).
    fn can_handle(&self, filename: &str) -> bool;
    /// Extract the file's content as Markdown. On failure returns Err with a
    /// sentinel-prefixed message (`EXTRACT_FAILED:` / `OCR_MISSING:` /
    /// `UNSUPPORTED_FORMAT:`).
    fn extract(&self, filename: &str, bytes: Vec<u8>) -> Result<String>;
    /// Strategy name (logs only).
    fn name(&self) -> &'static str;
}

/// Static strategy registry, priority order. First `can_handle` hit wins;
/// the plain-text strategy is last (fallback) and accepts text-sniffed bytes.
static EXTRACTORS: &[&dyn ContentExtractor] = &[
    &pdf::PdfExtractor,
    &office::OfficeExtractor,
    &image::ImageOcrExtractor,
    &text::PlainTextExtractor,
];

/// Whether any non-fallback strategy claims this filename. Used by
/// `service.rs` to decide: (a) content-file naming (`{id}.md` for extracted
/// Markdown vs `{id}{ext}` for raw text copies), (b) forced "copy" import
/// method (the content file is a DERIVED Markdown, not the source bytes), and
/// (c) skipping the plain-text sniff in `scan_folder` (PDF headers contain
/// NULs and would be misfiltered).
pub fn can_extract(filename: &str) -> bool {
    EXTRACTORS[..EXTRACTORS.len() - 1]
        .iter()
        .any(|e| e.can_handle(filename))
}

/// Dispatch entry: run the first strategy whose `can_handle` matches.
/// Heavy work (PDF parsing, OCR) runs on a blocking thread so the async
/// runtime's workers stay free. Never fails without a sentinel prefix.
pub async fn run(filename: &str, bytes: Vec<u8>) -> Result<String> {
    let strategy: &dyn ContentExtractor = EXTRACTORS
        .iter()
        .find(|e| e.can_handle(filename))
        .copied()
        .unwrap_or(&text::PlainTextExtractor);
    let name = strategy.name();
    let started = std::time::Instant::now();
    let filename_owned = filename.to_string();
    let result = tauri::async_runtime::spawn_blocking(move || strategy.extract(&filename_owned, bytes))
        .await
        .map_err(|e| anyhow!("extract task join failed: {e}"))?;

    match &result {
        Ok(md) => crate::rag::service::rag_log(
            "info",
            format!(
                "extract '{filename}' via {name}: {} chars in {}ms",
                md.chars().count(),
                started.elapsed().as_millis()
            ),
        ),
        Err(e) => crate::rag::service::rag_log(
            "warn",
            format!(
                "extract '{filename}' via {name} failed in {}ms: {e:#}",
                started.elapsed().as_millis()
            ),
        ),
    }
    result
}
