//! End-to-end processing of a single screenshot: OCR then structured extraction.

use crate::deskew;
use crate::extract;
use crate::model::Extraction;
use crate::ocr::{self, OcrConfig};
use crate::reference::Reference;
use chrono::{NaiveDateTime, TimeZone, Utc};
use std::path::Path;

/// Parse the capture time embedded in a Star Citizen screenshot filename,
/// e.g. `ScreenShot-2026-08-12_22-20-24-284.jpg` -> unix seconds.
///
/// The trailing `-284` group is a millisecond/hash suffix and is ignored.
pub fn parse_capture_time(path: &Path) -> Option<i64> {
    let stem = path.file_stem()?.to_str()?;
    let rest = stem.strip_prefix("ScreenShot-").unwrap_or(stem);
    // Expected: YYYY-MM-DD_HH-MM-SS[-suffix]
    // Take the first 19 chars: "YYYY-MM-DD_HH-MM-SS".
    let core: String = rest.chars().take(19).collect();
    let dt = NaiveDateTime::parse_from_str(&core, "%Y-%m-%d_%H-%M-%S").ok()?;
    Some(Utc.from_utc_datetime(&dt).timestamp())
}

/// Run OCR + extraction on a screenshot file.
///
/// Two passes are merged:
/// 1. **Raw full frame** — reads the (often coloured) terminal header best.
/// 2. **Cropped panel, grayscale, upscaled** — the commodity list is isolated
///    from the 3D scene and enlarged, which reads the thin price/quantity text
///    far more accurately and is *faster* (much less pixel area to OCR).
///
/// [`extract::merge`] combines them: terminal from pass 1, commodity fields from
/// whichever pass captured each with higher confidence.
pub fn process_screenshot(
    reference: &Reference,
    ocr_cfg: &OcrConfig,
    path: &Path,
) -> anyhow::Result<Extraction> {
    let captured_at = parse_capture_time(path);
    let name = path.to_string_lossy().to_string();

    // Pass 1: raw full frame — reads the (often coloured) terminal header, which
    // sits in the left panel and is best read in context.
    let words_raw = ocr::run_ocr(ocr_cfg, path)?;
    let ex_raw = extract::extract(reference, &words_raw, name.clone(), captured_at);

    // Pass 2: read the right-hand commodity panel from an isolated, sharpened
    // image — the big accuracy win for prices/quantities. First try to detect
    // the panel by its border and perspective-warp it flat; if that isn't
    // confident, fall back to a fixed-region crop with the same grayscale +
    // contrast + upscale (hue-agnostic, works on any terminal colour). The
    // terminal is taken from the raw pass; the commodities from this panel pass.
    let panel_path = match deskew::rectify_panel_to_temp(path, deskew::Side::Right, 1.6) {
        Ok(Some(p)) => Some(p),
        _ => deskew::crop_region_to_temp(path, deskew::Side::Right, 1.6).ok(),
    };
    if let Some(pp) = panel_path {
        let result = ocr::run_ocr(ocr_cfg, &pp).map(|words_panel| {
            let ex_panel = extract::extract(reference, &words_panel, name.clone(), captured_at);
            extract::merge(ex_panel, ex_raw.clone())
        });
        let _ = std::fs::remove_file(&pp);
        if let Ok(merged) = result {
            return Ok(merged);
        }
    }
    Ok(ex_raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parses_capture_time() {
        let p = PathBuf::from("ScreenShot-2026-08-12_22-20-24-284.jpg");
        let ts = parse_capture_time(&p).unwrap();
        // 2026-08-12 22:20:24 UTC
        assert_eq!(ts, 1_786_573_224);
    }

    #[test]
    fn tolerates_unexpected_names() {
        assert_eq!(parse_capture_time(&PathBuf::from("random.jpg")), None);
        assert_eq!(parse_capture_time(&PathBuf::from("ScreenShot-bad.jpg")), None);
    }
}
