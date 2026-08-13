//! Panel rectification: locate a Star Citizen trade-terminal UI panel by its
//! bright orange border, and perspective-warp it to an upright, high-contrast
//! grayscale image for OCR.
//!
//! The terminal is drawn on a (slightly curved / angled) in-game monitor, so the
//! panel is a mild quadrilateral rather than a perfect rectangle. Flattening it
//! and isolating it from the busy 3D scene behind the screen makes the thin
//! orange text far easier to read.

use image::{GrayImage, Luma, RgbaImage};
use imageproc::geometric_transformations::{warp_into, Interpolation, Projection};
use std::path::{Path, PathBuf};

/// A quadrilateral, corners ordered TL, TR, BR, BL (as `(x, y)` in pixels).
pub type Quad = [(f32, f32); 4];

/// Which side of the screen a panel is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Left panel — holds the location/terminal name.
    Left,
    /// Right panel — holds the commodity list.
    Right,
}

fn rand_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

/// True for a bright, saturated UI pixel of *any* hue. Terminal panels are drawn
/// in different accent colours per location (orange at some, cyan/teal at
/// others), so we key on brightness + saturation rather than a specific colour.
/// White/grey (unsaturated) and the dark 3D scene are excluded.
#[inline]
fn is_ui_pixel(p: &image::Rgba<u8>) -> bool {
    let (r, g, b) = (p.0[0] as i32, p.0[1] as i32, p.0[2] as i32);
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    mx > 95 && (mx - mn) > 32
}

/// Detect a UI panel quadrilateral on the requested side of the image.
///
/// Works on a coarse cell grid of the orange mask, keeps the largest connected
/// blob of "orange" cells on that side (rejecting sparse 3D-scene noise), then
/// takes the blob's four extreme corners.
pub fn detect_panel(img: &RgbaImage, side: Side) -> Option<Quad> {
    let (iw, ih) = img.dimensions();
    // Grid resolution for blob analysis.
    let cols = 160usize;
    let rows = 90usize;
    let cw = (iw as f32 / cols as f32).max(1.0);
    let ch = (ih as f32 / rows as f32).max(1.0);

    // Per cell, count UI (bright, saturated) pixels and dark pixels. The panel is
    // dark background + thin bright accents, so its cells have BOTH; a brightly
    // lit environment wall has UI-coloured pixels but few dark ones.
    let mut ui_counts = vec![0u32; cols * rows];
    let mut dark_counts = vec![0u32; cols * rows];
    for (x, y, p) in img.enumerate_pixels() {
        let c = ((x as f32 / cw) as usize).min(cols - 1);
        let r = ((y as f32 / ch) as usize).min(rows - 1);
        let i = r * cols + c;
        if is_ui_pixel(p) {
            ui_counts[i] += 1;
        }
        let mx = p.0[0].max(p.0[1]).max(p.0[2]);
        if mx < 55 {
            dark_counts[i] += 1;
        }
    }

    let cell_px = (cw * ch) as u32;
    let ui_thresh = (cell_px / 80).max(2);
    let dark_thresh = (cell_px as f32 * 0.30) as u32;
    let x_gate = match side {
        Side::Right => (cols as f32 * 0.55) as usize,
        Side::Left => 0,
    };
    let x_gate_hi = match side {
        Side::Right => cols,
        Side::Left => (cols as f32 * 0.45) as usize,
    };
    let mut on = vec![false; cols * rows];
    for r in 0..rows {
        for c in x_gate..x_gate_hi {
            let i = r * cols + c;
            if ui_counts[i] >= ui_thresh && dark_counts[i] >= dark_thresh {
                on[i] = true;
            }
        }
    }

    // Largest 4-connected blob of "on" cells.
    let blob = largest_blob(&on, cols, rows)?;
    if blob.len() < 20 {
        return None; // not enough structure
    }

    // Extreme corners of the blob in full-res pixel coordinates (cell centres).
    let pts: Vec<(f32, f32)> = blob
        .iter()
        .map(|&(c, r)| ((c as f32 + 0.5) * cw, (r as f32 + 0.5) * ch))
        .collect();
    let tl = *pts.iter().min_by(|a, b| (a.0 + a.1).total_cmp(&(b.0 + b.1)))?;
    let br = *pts.iter().max_by(|a, b| (a.0 + a.1).total_cmp(&(b.0 + b.1)))?;
    let tr = *pts.iter().max_by(|a, b| (a.0 - a.1).total_cmp(&(b.0 - b.1)))?;
    let bl = *pts.iter().min_by(|a, b| (a.0 - a.1).total_cmp(&(b.0 - b.1)))?;

    let quad = [tl, tr, br, bl];
    if !quad_is_sane(&quad, iw, ih) {
        return None;
    }
    Some(quad)
}

/// Reject implausible quads (too small, or wildly non-rectangular).
fn quad_is_sane(q: &Quad, iw: u32, ih: u32) -> bool {
    let w_top = dist(q[0], q[1]);
    let w_bot = dist(q[3], q[2]);
    let h_left = dist(q[0], q[3]);
    let h_right = dist(q[1], q[2]);
    let w = w_top.max(w_bot);
    let h = h_left.max(h_right);
    // Require a large, tall panel; otherwise we've only found a fragment (e.g. a
    // single highlighted row) and should fall back to the fixed-region crop.
    if w < iw as f32 * 0.12 || h < ih as f32 * 0.45 {
        return false;
    }
    // Opposite sides shouldn't differ by more than 35% (mild perspective only).
    let ratio_ok = |a: f32, b: f32| (a.min(b) / a.max(b)) > 0.65;
    ratio_ok(w_top, w_bot) && ratio_ok(h_left, h_right)
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// Largest 4-connected component of `true` cells; returns its `(col, row)` cells.
fn largest_blob(on: &[bool], cols: usize, rows: usize) -> Option<Vec<(usize, usize)>> {
    let mut seen = vec![false; cols * rows];
    let mut best: Vec<(usize, usize)> = Vec::new();
    for r0 in 0..rows {
        for c0 in 0..cols {
            let idx = r0 * cols + c0;
            if !on[idx] || seen[idx] {
                continue;
            }
            // BFS.
            let mut stack = vec![(c0, r0)];
            let mut comp = Vec::new();
            seen[idx] = true;
            while let Some((c, r)) = stack.pop() {
                comp.push((c, r));
                let neigh = [
                    (c.wrapping_sub(1), r),
                    (c + 1, r),
                    (c, r.wrapping_sub(1)),
                    (c, r + 1),
                ];
                for (nc, nr) in neigh {
                    if nc < cols && nr < rows {
                        let ni = nr * cols + nc;
                        if on[ni] && !seen[ni] {
                            seen[ni] = true;
                            stack.push((nc, nr));
                        }
                    }
                }
            }
            if comp.len() > best.len() {
                best = comp;
            }
        }
    }
    (!best.is_empty()).then_some(best)
}

/// Rectify a detected panel `quad` from `img` into an upright grayscale image,
/// upscaled by `upscale`, with a light contrast stretch. Returns the pixels.
pub fn rectify_to_gray(img: &RgbaImage, quad: &Quad, upscale: f32) -> GrayImage {
    // Output size from the quad's edge lengths.
    let w = dist(quad[0], quad[1]).max(dist(quad[3], quad[2]));
    let h = dist(quad[0], quad[3]).max(dist(quad[1], quad[2]));
    let ow = ((w * upscale).round() as u32).clamp(64, 6000);
    let oh = ((h * upscale).round() as u32).clamp(64, 6000);

    // Grayscale source.
    let gray = image::DynamicImage::ImageRgba8(img.clone()).to_luma8();

    // `warp_into` maps input->output internally, so build the projection that
    // sends the source quad onto the upright destination rectangle.
    let dst = [(0.0, 0.0), (ow as f32, 0.0), (ow as f32, oh as f32), (0.0, oh as f32)];
    let mut out = match Projection::from_control_points(*quad, dst) {
        Some(proj) => {
            let mut o = GrayImage::new(ow, oh);
            warp_into(&gray, &proj, Interpolation::Bilinear, Luma([0]), &mut o);
            o
        }
        // Degenerate quad: fall back to an axis-aligned crop of the quad's bbox.
        None => {
            let x0 = quad.iter().map(|p| p.0).fold(f32::MAX, f32::min).max(0.0) as u32;
            let y0 = quad.iter().map(|p| p.1).fold(f32::MAX, f32::min).max(0.0) as u32;
            let x1 = quad.iter().map(|p| p.0).fold(0.0, f32::max) as u32;
            let y1 = quad.iter().map(|p| p.1).fold(0.0, f32::max) as u32;
            let cw = (x1.saturating_sub(x0)).max(1);
            let chh = (y1.saturating_sub(y0)).max(1);
            let cropped = image::imageops::crop_imm(&gray, x0, y0, cw, chh).to_image();
            image::imageops::resize(&cropped, ow, oh, image::imageops::FilterType::Triangle)
        }
    };
    contrast_stretch(&mut out);
    out
}

/// Simple per-image contrast stretch (2nd..98th percentile -> 0..255).
fn contrast_stretch(img: &mut GrayImage) {
    let mut hist = [0u32; 256];
    for p in img.pixels() {
        hist[p.0[0] as usize] += 1;
    }
    let total: u32 = img.width() * img.height();
    let lo = percentile(&hist, total, 0.02);
    let hi = percentile(&hist, total, 0.98).max(lo + 1);
    let span = (hi - lo) as f32;
    for p in img.pixels_mut() {
        let v = p.0[0] as f32;
        let n = (((v - lo as f32) / span) * 255.0).clamp(0.0, 255.0);
        p.0[0] = n as u8;
    }
}

fn percentile(hist: &[u32; 256], total: u32, q: f32) -> u32 {
    let target = (total as f32 * q) as u32;
    let mut acc = 0u32;
    for (i, &c) in hist.iter().enumerate() {
        acc += c;
        if acc >= target {
            return i as u32;
        }
    }
    255
}

/// Detect and rectify a panel, saving it to a temp PNG for OCR. Returns `None`
/// when no confident panel is found (caller should fall back to
/// [`crop_region_to_temp`]).
pub fn rectify_panel_to_temp(path: &Path, side: Side, upscale: f32) -> anyhow::Result<Option<PathBuf>> {
    let img = image::open(path)?.to_rgba8();
    let Some(quad) = detect_panel(&img, side) else {
        return Ok(None);
    };
    let gray = rectify_to_gray(&img, &quad, upscale);
    let out = temp_png(side);
    gray.save(&out)?;
    Ok(Some(out))
}

/// Fixed-region fallback: crop the side of the screen where the panel lives,
/// grayscale + contrast-stretch + upscale. Hue-agnostic and deterministic — no
/// perspective correction, but the panels are close to upright, so this still
/// isolates the panel from the 3D scene and sharpens the text.
pub fn crop_region_to_temp(path: &Path, side: Side, upscale: f32) -> anyhow::Result<PathBuf> {
    let img = image::open(path)?;
    let (iw, ih) = (img.width(), img.height());
    let (x0f, x1f) = match side {
        Side::Right => (0.58f32, 1.0f32),
        Side::Left => (0.0f32, 0.42f32),
    };
    let (y0f, y1f) = (0.06f32, 0.97f32);
    let x0 = (iw as f32 * x0f) as u32;
    let y0 = (ih as f32 * y0f) as u32;
    let w = ((iw as f32 * x1f) as u32).saturating_sub(x0).max(1);
    let h = ((ih as f32 * y1f) as u32).saturating_sub(y0).max(1);

    let cropped = image::imageops::crop_imm(&img, x0, y0, w, h).to_image();
    let mut gray = image::DynamicImage::ImageRgba8(cropped).to_luma8();
    let scale = upscale.clamp(1.0, 3.0);
    if (scale - 1.0).abs() >= 0.01 {
        let (gw, gh) = gray.dimensions();
        gray = image::imageops::resize(
            &gray,
            ((gw as f32) * scale) as u32,
            ((gh as f32) * scale) as u32,
            image::imageops::FilterType::Lanczos3,
        );
    }
    contrast_stretch(&mut gray);
    let out = temp_png(side);
    gray.save(&out)?;
    Ok(out)
}

/// Produce a **colour** image of the requested panel for display / evidence:
/// the border-detected panel warped flat when confident, otherwise a fixed
/// right-region crop. Unlike [`rectify_to_gray`] this keeps colour and skips the
/// contrast stretch, so it looks natural as a preview.
pub fn panel_color(path: &Path, side: Side, upscale: f32) -> anyhow::Result<RgbaImage> {
    let img = image::open(path)?.to_rgba8();
    if let Some(quad) = detect_panel(&img, side) {
        return Ok(warp_color(&img, &quad, upscale));
    }
    // Fixed-region colour crop fallback.
    let (iw, ih) = img.dimensions();
    let (x0f, x1f) = match side {
        Side::Right => (0.58f32, 1.0f32),
        Side::Left => (0.0f32, 0.42f32),
    };
    let (y0f, y1f) = (0.06f32, 0.97f32);
    let x0 = (iw as f32 * x0f) as u32;
    let y0 = (ih as f32 * y0f) as u32;
    let w = ((iw as f32 * x1f) as u32).saturating_sub(x0).max(1);
    let h = ((ih as f32 * y1f) as u32).saturating_sub(y0).max(1);
    Ok(image::imageops::crop_imm(&img, x0, y0, w, h).to_image())
}

/// Perspective-warp `quad` to an upright colour rectangle.
fn warp_color(img: &RgbaImage, quad: &Quad, upscale: f32) -> RgbaImage {
    let w = dist(quad[0], quad[1]).max(dist(quad[3], quad[2]));
    let h = dist(quad[0], quad[3]).max(dist(quad[1], quad[2]));
    let ow = ((w * upscale).round() as u32).clamp(64, 4000);
    let oh = ((h * upscale).round() as u32).clamp(64, 4000);
    let dst = [(0.0, 0.0), (ow as f32, 0.0), (ow as f32, oh as f32), (0.0, oh as f32)];
    match Projection::from_control_points(*quad, dst) {
        Some(proj) => {
            let mut out = RgbaImage::new(ow, oh);
            warp_into(img, &proj, Interpolation::Bilinear, image::Rgba([0, 0, 0, 255]), &mut out);
            out
        }
        None => {
            let x0 = quad.iter().map(|p| p.0).fold(f32::MAX, f32::min).max(0.0) as u32;
            let y0 = quad.iter().map(|p| p.1).fold(f32::MAX, f32::min).max(0.0) as u32;
            let x1 = quad.iter().map(|p| p.0).fold(0.0, f32::max) as u32;
            let y1 = quad.iter().map(|p| p.1).fold(0.0, f32::max) as u32;
            image::imageops::crop_imm(img, x0, y0, x1.saturating_sub(x0).max(1), y1.saturating_sub(y0).max(1)).to_image()
        }
    }
}

fn temp_png(side: Side) -> PathBuf {
    std::env::temp_dir().join(format!(
        "ruex_panel_{}_{}_{}.png",
        std::process::id(),
        match side {
            Side::Left => "l",
            Side::Right => "r",
        },
        rand_suffix()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn ui_pixel_detector_is_hue_agnostic() {
        assert!(is_ui_pixel(&Rgba([230, 130, 40, 255]))); // orange UI
        assert!(is_ui_pixel(&Rgba([40, 200, 220, 255]))); // cyan/teal UI
        assert!(!is_ui_pixel(&Rgba([20, 20, 20, 255]))); // dark scene
        assert!(!is_ui_pixel(&Rgba([240, 240, 240, 255]))); // white (unsaturated)
        assert!(!is_ui_pixel(&Rgba([150, 150, 150, 255]))); // grey
    }

    #[test]
    fn detects_a_synthetic_panel() {
        // A realistic panel: dark background with a thin bright border and a few
        // thin "text" rows on the right — i.e. dark cells with sparse accents.
        let mut img = RgbaImage::from_pixel(640, 360, Rgba([8, 8, 8, 255]));
        let accent = Rgba([230, 130, 40, 255]);
        let (x0, x1, y0, y1) = (400u32, 580u32, 100u32, 280u32);
        for x in x0..x1 {
            for t in 0..2 {
                img.put_pixel(x, y0 + t, accent);
                img.put_pixel(x, y1 - 1 - t, accent);
            }
        }
        for y in y0..y1 {
            for t in 0..2 {
                img.put_pixel(x0 + t, y, accent);
                img.put_pixel(x1 - 1 - t, y, accent);
            }
        }
        for row in [140u32, 180, 220] {
            for x in (x0 + 10)..(x1 - 10) {
                img.put_pixel(x, row, accent);
            }
        }
        let quad = detect_panel(&img, Side::Right).expect("panel detected");
        // TL corner should be near (x0, y0).
        assert!((quad[0].0 - x0 as f32).abs() < 24.0, "tl.x={}", quad[0].0);
        assert!((quad[0].1 - y0 as f32).abs() < 24.0, "tl.y={}", quad[0].1);
    }
}
