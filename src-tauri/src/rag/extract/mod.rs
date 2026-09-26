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

/// Whether the extraction product for `filename` is Markdown. PDF and Office
/// strategies produce Markdown (via pdf_oxide / office_oxide `to_markdown`),
/// while the image strategy produces plain OCR text — the chunker routes by
/// the PRODUCED content: Markdown products use the markdown splitter
/// (heading/block boundaries), everything else (incl. OCR text) the plain
/// text splitter (2026-09 requirement).
pub fn produces_markdown(filename: &str) -> bool {
    pdf::PdfExtractor.can_handle(filename) || office::OfficeExtractor.can_handle(filename)
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

#[cfg(test)]
mod live_tests {
    use super::*;

    const TEST_PDF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../doc/test/苏州2.pdf");

    #[tokio::test]
    async fn real_pdf_extracts_to_markdown() {
        if !std::path::Path::new(TEST_PDF).exists() {
            return; // asset not present in CI checkout
        }
        let bytes = std::fs::read(TEST_PDF).unwrap();
        let out = run("苏州2.pdf", bytes).await.expect("pdf extract");
        assert!(!out.trim().is_empty(), "pdf text layer should produce content");
    }

    #[tokio::test]
    async fn minimal_docx_extracts_with_table_and_cjk() {
        // Build a minimal .docx in-memory (zip): [Content_Types].xml +
        // document.xml with a paragraph and a table cell containing CJK.
        use std::io::Write;
        let buf = Vec::new();
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(buf));
        let opts: zip::write::SimpleFileOptions = Default::default();
        w.start_file("[Content_Types].xml", opts).unwrap();
        w.write_all(br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#).unwrap();
        w.start_file("_rels/.rels", opts).unwrap();
        w.write_all(br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#).unwrap();
        w.start_file("word/document.xml", opts).unwrap();
        w.write_all("<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body><w:p><w:r><w:t>\u{9879}\u{76ee}\u{6807}\u{9898}</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>\u{8868}\u{683c}\u{6570}\u{636e}</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>".as_bytes()).unwrap();
        let cursor = w.finish().unwrap();
        let bytes = cursor.into_inner();

        let out = run("report.docx", bytes).await.expect("docx extract");
        assert!(out.contains("项目标题"), "paragraph text extracted: {out}");
        assert!(out.contains("表格数据"), "table cell text extracted: {out}");
    }

    #[tokio::test]
    async fn unsupported_binary_rejected_by_text_fallback() {
        let out = run("blob.bin", vec![0u8, 159, 146, 150, 0, 1, 2]).await;
        assert!(out.is_err(), "binary junk must not silently extract");
    }

    #[test]
    fn can_extract_and_produces_markdown_matrix() {
        assert!(can_extract("a.pdf"));
        assert!(can_extract("a.docx"));
        assert!(can_extract("a.png"));
        assert!(!can_extract("a.bin"));
        assert!(produces_markdown("a.pdf"));
        assert!(produces_markdown("a.docx"));
        assert!(!produces_markdown("a.png"), "OCR output is plain text, not markdown");
        assert!(!produces_markdown("a.txt"));
    }
}
