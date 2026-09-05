//! PDF extraction strategy: `pdf_oxide` -> Markdown, plus OCR over the
//! document's embedded images.
//!
//! - Text layer: `PdfDocument::open_from_bytes` + `to_markdown_all` (semantic
//!   mode: heading detection + tables on, images off — images are handled
//!   here instead).
//! - Embedded images: `extract_images(page)` per page -> `to_png_bytes` ->
//!   shared OCR core; text appended to the document tail as a quote block.
//!   A single image failing OCR is logged and skipped, never fatal.
//! - Scanned PDFs (no text layer): if OCR is available the whole document is
//!   recovered page-by-page via OCR; otherwise a hard `EXTRACT_FAILED` with a
//!   clear reason.

use super::{ocr, ContentExtractor};
use anyhow::{anyhow, Result};
use pdf_oxide::converters::ConversionOptions;
use pdf_oxide::PdfDocument;

pub struct PdfExtractor;

impl ContentExtractor for PdfExtractor {
    fn can_handle(&self, filename: &str) -> bool {
        filename.to_lowercase().ends_with(".pdf")
    }

    fn extract(&self, _filename: &str, bytes: Vec<u8>) -> Result<String> {
        let doc = PdfDocument::from_bytes(bytes)
            .map_err(|e| anyhow!("EXTRACT_FAILED: pdf open failed: {e}"))?;
        let page_count = doc
            .page_count()
            .map_err(|e| anyhow!("EXTRACT_FAILED: pdf page count failed: {e}"))?;

        let mut md = doc.to_markdown_all(&ConversionOptions::default())
            .map_err(|e| anyhow!("EXTRACT_FAILED: pdf to markdown failed: {e}"))?;
        md = md.trim().to_string();
        if md.is_empty() {
            // No text layer (typical for scans). Try whole-document OCR.
            if !ocr::available() {
                return Err(anyhow!(
                    "EXTRACT_FAILED: pdf has no text layer (scanned document) and no OCR engine is available"
                ));
            }
            return ocr_scanned_pdf(&doc, page_count);
        }

        // Text layer present — OCR embedded images and append their text.
        if ocr::available() {
            let mut ocr_blocks: Vec<String> = Vec::new();
            for page in 0..page_count {
                let images = match doc.extract_images(page) {
                    Ok(imgs) => imgs,
                    Err(e) => {
                        crate::rag::service::rag_log(
                            "warn",
                            format!("pdf extract_images(page {page}) failed: {e}"),
                        );
                        continue;
                    },
                };
                for (idx, img) in images.iter().enumerate() {
                    match img.to_png_bytes() {
                        Ok(png) => match ocr::image_bytes(&png) {
                            Ok(text) if !text.trim().is_empty() => {
                                ocr_blocks.push(format!("> OCR(图片{}):\n> {}", ocr_blocks.len() + 1, text.replace('\n', "\n> ")));
                            },
                            Ok(_) => {},
                            Err(e) => {
                                crate::rag::service::rag_log(
                                    "warn",
                                    format!("pdf embedded image OCR (page {page} image {idx}) skipped: {e:#}"),
                                );
                            },
                        },
                        Err(e) => {
                            crate::rag::service::rag_log(
                                "warn",
                                format!("pdf embedded image -> png (page {page} image {idx}) failed: {e}"),
                            );
                        },
                    }
                }
            }
            if !ocr_blocks.is_empty() {
                md.push_str("\n\n[图片OCR]\n");
                md.push_str(&ocr_blocks.join("\n\n"));
            }
        } else if has_embedded_images(&doc, page_count) {
            // Text extracted fine but OCR can't read the embedded images —
            // note it in the output so the user knows why images are absent.
            md.push_str("\n\n[图片未识别：当前平台 OCR 引擎不可用]\n");
        }
        Ok(md)
    }

    fn name(&self) -> &'static str {
        "pdf"
    }
}

/// Whole-document OCR for scanned PDFs: render nothing (pdf_oxide doesn't
/// rasterize here) — instead reuse each page's embedded images. A scanned PDF
/// is exactly one full-page image per page, so OCRing the embedded images
/// covers it. Pages with no extractable image are noted.
fn ocr_scanned_pdf(doc: &PdfDocument, page_count: usize) -> Result<String> {
    let mut out = String::from("# Scanned PDF (OCR)\n");
    let mut any_text = false;
    for page in 0..page_count {
        let images = doc.extract_images(page).unwrap_or_default();
        for img in &images {
            if let Ok(png) = img.to_png_bytes() {
                if let Ok(text) = ocr::image_bytes(&png) {
                    if !text.trim().is_empty() {
                        any_text = true;
                        out.push_str(&text);
                        out.push_str("\n\n---\n\n");
                    }
                }
            }
        }
    }
    if !any_text {
        return Err(anyhow!(
            "EXTRACT_FAILED: pdf has no text layer and OCR recovered no text"
        ));
    }
    Ok(out)
}

/// Cheap probe: does ANY page carry an embedded image? Only used to decide
/// whether the "[图片未识别]" note is worth adding when OCR is unavailable.
fn has_embedded_images(doc: &PdfDocument, page_count: usize) -> bool {
    (0..page_count).any(|p| doc.extract_images(p).map(|v| !v.is_empty()).unwrap_or(false))
}
