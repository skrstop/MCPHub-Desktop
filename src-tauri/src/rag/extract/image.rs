//! Standalone image OCR strategy: png/jpg/jpeg/gif/bmp/webp/tiff/tif files
//! recognized directly through the shared OCR core. An image that yields no
//! text at all is a hard failure (`EXTRACT_FAILED`) — unlike embedded images
//! inside PDFs/Office docs, a standalone image has no other content to fall
//! back on, so the user gets a specific reason instead of an empty doc.

use super::{ocr, ContentExtractor};
use anyhow::{anyhow, Result};

const IMAGE_EXTS: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".gif", ".bmp", ".webp", ".tiff", ".tif",
];

pub struct ImageOcrExtractor;

impl ContentExtractor for ImageOcrExtractor {
    fn can_handle(&self, filename: &str) -> bool {
        let lower = filename.to_lowercase();
        IMAGE_EXTS.iter().any(|ext| lower.ends_with(ext))
    }

    fn extract(&self, _filename: &str, bytes: Vec<u8>) -> Result<String> {
        let text = ocr::image_bytes(&bytes)?;
        if text.trim().is_empty() {
            return Err(anyhow!("EXTRACT_FAILED: no text recognized in image"));
        }
        Ok(text)
    }

    fn name(&self) -> &'static str {
        "image-ocr"
    }
}
