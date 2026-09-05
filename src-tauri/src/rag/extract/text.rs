//! Plain-text fallback strategy — the historical upload path, byte-for-byte:
//! NUL-byte sniff (`is_likely_text`) rejects binaries with
//! `UNSUPPORTED_FORMAT`, everything else goes through `decode_text`
//! (chardetng + encoding_rs). Runs last in the registry as the catch-all.

use super::ContentExtractor;
use crate::rag::service::{decode_text, is_likely_text};
use anyhow::{anyhow, Result};

pub struct PlainTextExtractor;

impl ContentExtractor for PlainTextExtractor {
    fn can_handle(&self, _filename: &str) -> bool {
        true // fallback: always willing to try
    }

    fn extract(&self, filename: &str, bytes: Vec<u8>) -> Result<String> {
        if !is_likely_text(&bytes) {
            return Err(anyhow!("UNSUPPORTED_FORMAT: {}", filename));
        }
        let (content, _encoding) = decode_text(&bytes, filename);
        Ok(content)
    }

    fn name(&self) -> &'static str {
        "text"
    }
}
