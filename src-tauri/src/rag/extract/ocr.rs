//! Reusable OCR core for the RAG extract strategies.
//!
//! One async entry (`image_bytes`) + a pre-flight `status()` that callers
//! (and the `get_ocr_status` Tauri command) use to decide whether OCR is
//! possible at all. Three platform backends, all system-provided:
//! - macOS: Apple Vision (`objc2-vision` bindings, links the system framework)
//! - Windows: Windows.Media.Ocr (`windows` crate WinRT bindings, built into
//!   the OS)
//! - Linux: external `tesseract` binary — MANDATORY pre-flight check before
//!   every call (the binary may be missing entirely; `available()` caches the
//!   probe so the hot path never shells out twice).
//!
//! The language pair is fixed to Chinese + English (the app's i18n set).

use anyhow::{anyhow, Result};

/// OCR capability report for the frontend (`get_ocr_status` command).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrStatus {
    /// Whether OCR can run at all on this machine.
    pub available: bool,
    /// OS platform ("macos" | "windows" | "linux").
    pub platform: String,
    /// Linux distro id from /etc/os-release (None elsewhere) — the frontend
    /// uses it to pick the apt/dnf/pacman install command.
    pub distro: Option<String>,
    /// Human-readable engine name for display.
    pub engine: String,
    /// Tesseract language packs that are missing (Linux only).
    pub missing_langs: Vec<String>,
}

/// Fixed recognition languages: Simplified Chinese + English (tesseract pack
/// names on Linux; macOS/Windows system engines auto-detect language).
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const TESSERACT_LANGS: &[&str] = &["chi_sim", "eng"];

// ── public surface ──────────────────────────────────────────────────────────

/// Whether OCR is available on this platform. Cheap (cached) — safe to call
/// before every recognition and from the upload dialog's pre-flight.
pub fn available() -> bool {
    status().available
}

/// Probe the platform's OCR capability. Linux results are cached after the
/// first probe (spawning `tesseract --version` per upload would be wasteful).
pub fn status() -> OcrStatus {
    #[cfg(target_os = "macos")]
    {
        macos::status()
    }
    #[cfg(target_os = "windows")]
    {
        windows_imp::status()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        linux::status()
    }
}

/// Run OCR over raw image bytes (png/jpg/jpeg/gif/bmp/webp/tiff — anything
/// the `image` crate decodes). Returns the recognized text (may be empty).
///
/// Synchronous by design: the only callers are the extract strategies, which
/// already execute inside `extract::run`'s blocking thread — nesting another
/// spawn_blocking would just add a thread hop.
///
/// Errors:
/// - `OCR_MISSING: <detail>` — engine absent (frontend opens the install
///   dialog instead of a toast; never a hard failure of the import itself).
/// - other messages — decode/recognize failures (caller may skip the image).
pub fn image_bytes(bytes: &[u8]) -> Result<String> {
    let status = status();
    if !status.available {
        let detail = if status.missing_langs.is_empty() {
            String::new()
        } else {
            format!(" (missing language packs: {})", status.missing_langs.join(", "))
        };
        return Err(anyhow!(
            "OCR_MISSING: {}{}{}",
            status.platform,
            status.distro.as_deref().map(|d| format!(":{d}")).unwrap_or_default(),
            detail
        ));
    }
    recognize_blocking(bytes)
}

/// Synchronous recognition on an already-validated image payload.
fn recognize_blocking(bytes: &[u8]) -> Result<String> {
    // Validate decodeability first so every backend gets sane input (and the
    // error names the real problem instead of a framework-level mystery).
    let _probe = image::load_from_memory(bytes)
        .map_err(|e| anyhow!("image decode failed: {e}"))?;

    #[cfg(target_os = "macos")]
    {
        macos::recognize(bytes)
    }
    #[cfg(target_os = "windows")]
    {
        windows_imp::recognize(bytes)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        linux::recognize(bytes)
    }
}

/// Trim + collapse the engine output: drop blank lines, trim per-line
/// whitespace. Empty output is NOT an error here — callers decide whether an
/// empty result matters for their document kind.
fn clean_text(raw: String) -> String {
    raw.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

// ── macOS: Apple Vision via objc2-vision ────────────────────────────────────

#[cfg(target_os = "macos")]
mod macos {
    use super::{clean_text, OcrStatus};
    use anyhow::{anyhow, Result};
    use objc2::rc::Retained;
    use objc2::AnyThread;
    use objc2_foundation::{NSArray, NSData, NSDictionary};
    use objc2_vision::{
        VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizeTextRequestRevision3, VNRequest,
        VNRequestTextRecognitionLevel,
    };

    pub fn status() -> OcrStatus {
        // Vision ships with macOS 10.15+; always available on supported
        // targets (the app's minimum is far above that).
        OcrStatus {
            available: true,
            platform: "macos".into(),
            distro: None,
            engine: "Apple Vision".into(),
            missing_langs: vec![],
        }
    }

    pub fn recognize(bytes: &[u8]) -> Result<String> {
        // Autorelease pool: spawn_blocking threads are pooled and reused, so
        // Vision's autoreleased intermediates must be drained per call.
        objc2::rc::autoreleasepool(|_| recognize_inner(bytes))
    }

    fn recognize_inner(bytes: &[u8]) -> Result<String> {
        let data = NSData::with_bytes(bytes);
        let request = VNRecognizeTextRequest::new();
        // Revision 3 enables `automaticallyDetectsLanguage` (macOS 13+),
        // which covers Chinese + English without picking models manually.
        unsafe { request.setRevision(VNRecognizeTextRequestRevision3) };
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setUsesLanguageCorrection(true);
        request.setAutomaticallyDetectsLanguage(true);

        let request_super: Retained<VNRequest> = unsafe { Retained::cast_unchecked(request.clone()) };
        let requests = NSArray::from_retained_slice(&[request_super]);
        let options = NSDictionary::new();
        let handler = VNImageRequestHandler::initWithData_options(
            VNImageRequestHandler::alloc(),
            &data,
            &options,
        );
        handler
            .performRequests_error(&requests)
            .map_err(|e| anyhow!("Vision performRequests failed: {e}"))?;

        let mut out = String::new();
        if let Some(observations) = request.results() {
            for obs in observations {
                if let Some(candidate) = obs.topCandidates(1).firstObject() {
                    out.push_str(&candidate.string().to_string());
                    out.push('\n');
                }
            }
        }
        Ok(clean_text(out))
    }
}

// ── Windows: Windows.Media.Ocr via the `windows` crate ──────────────────────

#[cfg(target_os = "windows")]
mod windows_imp {
    use super::{clean_text, OcrStatus};
    use anyhow::{anyhow, Result};

    pub fn status() -> OcrStatus {
        OcrStatus {
            available: true,
            platform: "windows".into(),
            distro: None,
            engine: "Windows.Media.Ocr".into(),
            missing_langs: vec![],
        }
    }

    pub fn recognize(bytes: &[u8]) -> Result<String> {
        use windows::Win32::System::Com::{
            CoInitializeEx, CoUninitialize, COINIT, COINIT_MULTITHREADED,
        };

        // WinRT needs an initialized apartment on the calling thread. Tokio's
        // blocking threads are fresh, so initialize (and release) around the
        // work. RPC_E_CHANGED_MODE means the thread already runs another
        // apartment mode — WinRT still works from it, just skip our init.
        // windows 0.62: CoInitializeEx returns HRESULT directly (no Result).
        use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
        unsafe {
            let hr = CoInitializeEx(None, COINIT(COINIT_MULTITHREADED.0));
            if hr.is_ok() {
                let result = recognize_inner(bytes);
                CoUninitialize();
                result
            } else if hr == RPC_E_CHANGED_MODE {
                recognize_inner(bytes)
            } else {
                Err(anyhow!("CoInitializeEx failed: {hr:?}"))
            }
        }
    }

    fn recognize_inner(bytes: &[u8]) -> Result<String> {
        use windows::core::HSTRING;
        use windows::Globalization::Language;
        use windows::Graphics::Imaging::BitmapDecoder;
        use windows::Media::Ocr::OcrEngine;
        use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};

        // Engine: prefer the user's profile languages, then explicit zh/en.
        // windows 0.62: TryCreate* return Result<OcrEngine> (no Option inside).
        let engine = OcrEngine::TryCreateFromUserProfileLanguages()
            .ok()
            .or_else(|| {
                Language::CreateLanguage(&HSTRING::from("zh-Hans"))
                    .ok()
                    .and_then(|l| OcrEngine::TryCreateFromLanguage(&l).ok())
            })
            .or_else(|| {
                Language::CreateLanguage(&HSTRING::from("en-US"))
                    .ok()
                    .and_then(|l| OcrEngine::TryCreateFromLanguage(&l).ok())
            })
            .ok_or_else(|| anyhow!("no OCR language installed on this Windows system"))?;

        let stream = InMemoryRandomAccessStream::new()
            .map_err(|e| anyhow!("stream create failed: {e}"))?;
        let writer = DataWriter::CreateDataWriter(&stream)
            .map_err(|e| anyhow!("DataWriter create failed: {e}"))?;
        writer.WriteBytes(bytes).map_err(|e| anyhow!("write bytes failed: {e}"))?;
        writer.StoreAsync()
            .map_err(|e| anyhow!("store failed: {e}"))?
            .join()
            .map_err(|e| anyhow!("store wait failed: {e}"))?;
        writer.FlushAsync()
            .map_err(|e| anyhow!("flush failed: {e}"))?
            .join()
            .map_err(|e| anyhow!("flush wait failed: {e}"))?;
        writer.DetachStream().map_err(|e| anyhow!("detach failed: {e}"))?;
        stream.Seek(0).map_err(|e| anyhow!("seek failed: {e}"))?;

        let decoder = BitmapDecoder::CreateAsync(&stream)
            .map_err(|e| anyhow!("decoder create failed: {e}"))?
            .join()
            .map_err(|e| anyhow!("decoder wait failed: {e}"))?;
        let bitmap = decoder
            .GetSoftwareBitmapAsync()
            .map_err(|e| anyhow!("decode failed: {e}"))?
            .join()
            .map_err(|e| anyhow!("decode wait failed: {e}"))?;
        let ocr = engine
            .RecognizeAsync(&bitmap)
            .map_err(|e| anyhow!("recognize failed: {e}"))?
            .join()
            .map_err(|e| anyhow!("recognize wait failed: {e}"))?;

        let lines = ocr.Lines().map_err(|e| anyhow!("lines failed: {e}"))?;
        let mut out = String::new();
        for line in lines {
            out.push_str(&line.Text()?.to_string());
            out.push('\n');
        }
        Ok(clean_text(out))
    }
}

// ── Linux: external tesseract binary ────────────────────────────────────────

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod linux {
    use super::{clean_text, TESSERACT_LANGS, OcrStatus};
    use anyhow::{anyhow, Result};
    use std::sync::OnceLock;

    struct Probe {
        tesseract: bool,
        missing_langs: Vec<String>,
    }

    fn probe() -> &'static Probe {
        static PROBE: OnceLock<Probe> = OnceLock::new();
        PROBE.get_or_init(|| {
            let tesseract = std::process::Command::new("tesseract")
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !tesseract {
                return Probe { tesseract: false, missing_langs: vec![] };
            }
            // List installed language packs; report the ones we need missing.
            let langs = std::process::Command::new("tesseract")
                .arg("--list-langs")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default();
            let have: Vec<String> = langs
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && !l.starts_with("List of"))
                .collect();
            let missing = TESSERACT_LANGS
                .iter()
                .filter(|l| !have.iter().any(|h| h == *l))
                .map(|l| l.to_string())
                .collect();
            Probe { tesseract: true, missing_langs: missing }
        })
    }

    fn distro_id() -> Option<String> {
        let content = std::fs::read_to_string("/etc/os-release").ok()?;
        for line in content.lines() {
            if let Some(id) = line.strip_prefix("ID=") {
                return Some(id.trim_matches('"').to_string());
            }
        }
        None
    }

    pub fn status() -> OcrStatus {
        let p = probe();
        OcrStatus {
            available: p.tesseract && p.missing_langs.is_empty(),
            platform: "linux".into(),
            distro: distro_id(),
            engine: "tesseract".into(),
            missing_langs: p.missing_langs.clone(),
        }
    }

    pub fn recognize(bytes: &[u8]) -> Result<String> {
        let p = probe();
        if !p.tesseract {
            return Err(anyhow!("OCR_MISSING: linux:tesseract not installed"));
        }

        // Re-encode to PNG (tesseract/leptonica reads it everywhere) in a
        // unique temp file, run the CLI, clean up regardless of outcome.
        let img = image::load_from_memory(bytes).map_err(|e| anyhow!("image decode failed: {e}"))?;
        let tmp = std::env::temp_dir().join(format!("mcphub-ocr-{}.png", uuid::Uuid::new_v4()));
        img.save_with_format(&tmp, image::ImageFormat::Png)
            .map_err(|e| anyhow!("png encode failed: {e}"))?;

        let lang = TESSERACT_LANGS.join("+");
        let result = std::process::Command::new("tesseract")
            .arg(&tmp)
            .arg("stdout")
            .arg("-l")
            .arg(&lang)
            .arg("--psm")
            .arg("1")
            .output();
        let _ = std::fs::remove_file(&tmp);

        match result {
            Ok(out) if out.status.success() => {
                Ok(clean_text(String::from_utf8_lossy(&out.stdout).into_owned()))
            },
            Ok(out) => Err(anyhow!(
                "tesseract failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            )),
            Err(e) => Err(anyhow!("tesseract spawn failed: {e}")),
        }
    }
}
