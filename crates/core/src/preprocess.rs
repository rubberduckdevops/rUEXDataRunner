//! Image helpers: prepare a screenshot for OCR, and encode it for the UEX API
//! (which accepts a base64 JPEG/PNG up to 10 MB as submission evidence).

use base64::Engine;
use image::imageops::FilterType;
use std::path::{Path, PathBuf};

/// UEX accepts screenshots up to 10 MB; we target comfortably under that.
const MAX_SCREENSHOT_BYTES: usize = 8 * 1024 * 1024;

/// Load an image, convert to grayscale and upscale, and write it to a temp PNG
/// suitable for OCR. Returns the temp path (caller may delete it afterwards).
///
/// Upscaling helps Tesseract on the game's thin UI font; grayscale removes the
/// coloured background gradient.
pub fn prepare_for_ocr(path: &Path, scale: f32) -> anyhow::Result<PathBuf> {
    let img = image::open(path)?;
    let gray = img.to_luma8();
    let out = std::env::temp_dir()
        .join(format!("ruex_prep_{}_{}.png", std::process::id(), rand_suffix()));

    // Scale ~1.0 is a no-op resize; skip it (upscaling an already-4K screenshot
    // is very slow and buys nothing).
    if (scale - 1.0).abs() < 0.01 {
        gray.save(&out)?;
        return Ok(out);
    }
    let (w, h) = gray.dimensions();
    let nw = ((w as f32) * scale).round().max(1.0) as u32;
    let nh = ((h as f32) * scale).round().max(1.0) as u32;
    let scaled = image::imageops::resize(&gray, nw, nh, FilterType::Lanczos3);
    scaled.save(&out)?;
    Ok(out)
}

fn rand_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

/// Encode a screenshot as a base64 string for API submission, re-encoding as
/// JPEG and downscaling as needed to stay under the size limit.
pub fn encode_screenshot_base64(path: &Path) -> anyhow::Result<String> {
    // Fast path: small enough to send as-is.
    let raw = std::fs::read(path)?;
    if raw.len() <= MAX_SCREENSHOT_BYTES {
        return Ok(base64::engine::general_purpose::STANDARD.encode(&raw));
    }

    // Otherwise decode and re-encode as progressively smaller JPEGs.
    let img = image::open(path)?;
    for &scale in &[1.0f32, 0.75, 0.5, 0.35] {
        let bytes = encode_jpeg(&img, scale, 80)?;
        if bytes.len() <= MAX_SCREENSHOT_BYTES {
            return Ok(base64::engine::general_purpose::STANDARD.encode(&bytes));
        }
    }
    // Last resort: smallest attempt.
    let bytes = encode_jpeg(&img, 0.25, 70)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

/// Encode an in-memory RGBA image (e.g. the cropped commodity panel) as a
/// base64 JPEG for API submission, downscaling if needed to stay under the limit.
pub fn encode_rgba_base64(img: &image::RgbaImage) -> anyhow::Result<String> {
    let dynimg = image::DynamicImage::ImageRgba8(img.clone());
    for &scale in &[1.0f32, 0.85, 0.7, 0.5, 0.35] {
        let bytes = encode_jpeg(&dynimg, scale, 82)?;
        if bytes.len() <= MAX_SCREENSHOT_BYTES {
            return Ok(base64::engine::general_purpose::STANDARD.encode(&bytes));
        }
    }
    let bytes = encode_jpeg(&dynimg, 0.3, 70)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

fn encode_jpeg(img: &image::DynamicImage, scale: f32, quality: u8) -> anyhow::Result<Vec<u8>> {
    let (w, h) = (img.width(), img.height());
    let resized = if (scale - 1.0).abs() < f32::EPSILON {
        img.clone()
    } else {
        img.resize(
            ((w as f32) * scale).round() as u32,
            ((h as f32) * scale).round() as u32,
            FilterType::Triangle,
        )
    };
    let rgb = resized.to_rgb8();
    let mut buf = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    enc.encode(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    fn write_temp_image(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(name);
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_fn(64, 48, |x, y| Rgb([(x * 4) as u8, (y * 5) as u8, 120]));
        img.save(&p).unwrap();
        p
    }

    #[test]
    fn encodes_small_image_as_base64() {
        let p = write_temp_image("ruex_test_small.png");
        let b64 = encode_screenshot_base64(&p).unwrap();
        assert!(!b64.is_empty());
        // Round-trips as valid base64.
        assert!(base64::engine::general_purpose::STANDARD.decode(&b64).is_ok());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn prepare_for_ocr_scales_and_grays() {
        let p = write_temp_image("ruex_test_prep.png");
        let out = prepare_for_ocr(&p, 2.0).unwrap();
        let prepared = image::open(&out).unwrap();
        assert_eq!(prepared.width(), 128);
        assert_eq!(prepared.height(), 96);
        let _ = std::fs::remove_file(p);
        let _ = std::fs::remove_file(out);
    }
}
