//! Office extraction strategy: DOCX/DOC/XLSX/XLS/PPTX/PPT -> Markdown via
//! `office_oxide`, plus OCR over embedded images.
//!
//! - Text: `Document::from_reader` -> `to_ir()` -> `DocumentIR::to_markdown()`
//!   (the crate's own renderer: sections joined with `---`, headings, tables,
//!   lists, code blocks).
//! - Embedded images: walk the public IR (`sections[].elements[]`, recursing
//!   into Table cells / List items / TextBox / Note bodies) and collect
//!   `Element::Image` bytes. Raster formats (png/jpeg/gif/tiff/bmp) go to the
//!   shared OCR core; Emf/Wmf vector images are skipped (they carry rendered
//!   shapes, not OCR-able text).
//! - OCR text is appended to the document tail as a quote block (conservative
//!   choice: no position mapping). Single-image failures are logged+skipped.

use super::{ocr, ContentExtractor};
use anyhow::{anyhow, Result};
use office_oxide::ir::{DocumentIR, Element, ImageFormat};
use office_oxide::{Document, DocumentFormat};

pub struct OfficeExtractor;

impl ContentExtractor for OfficeExtractor {
    fn can_handle(&self, filename: &str) -> bool {
        ext_format(filename).is_some()
    }

    fn extract(&self, _filename: &str, bytes: Vec<u8>) -> Result<String> {
        let format = ext_format(_filename).ok_or_else(|| anyhow!("UNSUPPORTED_FORMAT: {_filename}"))?;
        let doc = Document::from_reader(std::io::Cursor::new(bytes), format)
            .map_err(|e| anyhow!("EXTRACT_FAILED: office open failed ({format:?}): {e}"))?;
        let ir = doc.to_ir();
        let mut md = ir.to_markdown().trim().to_string();
        if md.is_empty() {
            return Err(anyhow!(
                "EXTRACT_FAILED: {} document contains no extractable text",
                format.extension()
            ));
        }

        if ocr::available() {
            let images = collect_images(&ir);
            let mut ocr_blocks: Vec<String> = Vec::new();
            for (idx, data) in images.iter().enumerate() {
                match ocr::image_bytes(data) {
                    Ok(text) if !text.trim().is_empty() => {
                        ocr_blocks.push(format!(
                            "> OCR(图片{}):\n> {}",
                            ocr_blocks.len() + 1,
                            text.replace('\n', "\n> ")
                        ));
                    },
                    Ok(_) => {},
                    Err(e) => {
                        crate::rag::service::rag_log(
                            "warn",
                            format!("office embedded image OCR ({idx}) skipped: {e:#}"),
                        );
                    },
                }
            }
            if !ocr_blocks.is_empty() {
                md.push_str("\n\n[图片OCR]\n");
                md.push_str(&ocr_blocks.join("\n\n"));
            }
        } else if has_raster_images(&ir) {
            md.push_str("\n\n[图片未识别：当前平台 OCR 引擎不可用]\n");
        }
        Ok(md)
    }

    fn name(&self) -> &'static str {
        "office"
    }
}

/// Map a filename extension to the office_oxide format (the six supported
/// kinds only — .ods/.xlsm/.xlsb etc. fall through to `None` and the text
/// fallback strategy rejects them). Requires a real dot: a dotless file
/// literally named "pdf" must NOT be claimed (rsplit would return the whole
/// name and from_extension would match it).
fn ext_format(filename: &str) -> Option<DocumentFormat> {
    let lower = filename.to_lowercase();
    let dot = lower.rfind('.')?;
    DocumentFormat::from_extension(&lower[dot + 1..])
}

/// Depth-first walk of the IR collecting OCR-able embedded image bytes.
fn collect_images(ir: &DocumentIR) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for section in &ir.sections {
        collect_elements(&section.elements, &mut out);
    }
    out
}

/// Whether any OCR-able raster image exists (no byte cloning — used only to
/// decide the "[图片未识别]" note when OCR is unavailable).
fn has_raster_images(ir: &DocumentIR) -> bool {
    !collect_images(ir).is_empty()
}

fn collect_elements(elements: &[Element], out: &mut Vec<Vec<u8>>) {
    for element in elements {
        match element {
            // Paragraph / Heading carry inline runs only — no nested images.
            Element::Table(t) => {
                for row in &t.rows {
                    for cell in &row.cells {
                        collect_elements(&cell.content, out);
                    }
                }
            },
            Element::List(l) => collect_list(l, out),
            Element::TextBox(tb) => collect_elements(&tb.content, out),
            Element::Footnote(n) | Element::Endnote(n) => collect_elements(&n.content, out),
            Element::Image(img) => {
                if let Some(data) = &img.data {
                    // Skip Emf/Wmf vector images: their text is drawn as
                    // shapes, OCR would return garbage or nothing.
                    if !matches!(img.format, Some(ImageFormat::Emf) | Some(ImageFormat::Wmf)) {
                        out.push(data.clone());
                    }
                }
            },
            // Paragraph / Heading / CodeBlock / ThematicBreak / PageBreak /
            // ColumnBreak / Shape carry no images.
            _ => {},
        }
    }
}

fn collect_list(list: &office_oxide::ir::List, out: &mut Vec<Vec<u8>>) {
    for item in &list.items {
        collect_elements(&item.content, out);
        if let Some(nested) = &item.nested {
            collect_list(nested, out);
        }
    }
}
