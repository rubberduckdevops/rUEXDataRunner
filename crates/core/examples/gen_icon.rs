//! One-off: generate the app icon (`assets/icon.ico` + `assets/icon.png`).
//! A cyan "scanner hexagon" with a downward data-capture arrow, on the app's
//! dark theme. Run: `cargo run -p datarunner-core --example gen_icon`

use image::{Rgba, RgbaImage};
use imageproc::drawing::{draw_filled_rect_mut, draw_polygon_mut};
use imageproc::point::Point;
use imageproc::rect::Rect;
use std::fs::File;

const BG: Rgba<u8> = Rgba([13, 19, 25, 255]);
const CYAN: Rgba<u8> = Rgba([74, 210, 219, 255]);

/// Regular pointy-top hexagon vertices centred at `(cx, cy)` with radius `r`.
fn hexagon(cx: f32, cy: f32, r: f32) -> Vec<Point<i32>> {
    (0..6)
        .map(|i| {
            let a = std::f32::consts::PI / 180.0 * (60.0 * i as f32 - 90.0);
            Point::new((cx + r * a.cos()).round() as i32, (cy + r * a.sin()).round() as i32)
        })
        .collect()
}

fn draw_icon(size: u32) -> RgbaImage {
    // Draw at high res then downscale for crisp edges.
    let s = 256i32;
    let mut img = RgbaImage::from_pixel(s as u32, s as u32, BG);
    let (cx, cy) = (128.0, 130.0);

    // Thick cyan hexagon ring: outer filled, inner punched back to bg.
    draw_polygon_mut(&mut img, &hexagon(cx, cy, 104.0), CYAN);
    draw_polygon_mut(&mut img, &hexagon(cx, cy, 84.0), BG);

    // Downward capture arrow in the centre.
    draw_filled_rect_mut(&mut img, Rect::at(120, 66).of_size(16, 66), CYAN);
    let head = vec![Point::new(96, 118), Point::new(160, 118), Point::new(128, 168)];
    draw_polygon_mut(&mut img, &head, CYAN);

    if size == s as u32 {
        img
    } else {
        image::imageops::resize(&img, size, size, image::imageops::FilterType::Lanczos3)
    }
}

fn main() -> anyhow::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    std::fs::create_dir_all(&root)?;

    let base = draw_icon(256);
    base.save(root.join("icon.png"))?;

    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for sz in [256u32, 64, 48, 32, 16] {
        let img = draw_icon(sz);
        let ii = ico::IconImage::from_rgba_data(sz, sz, img.into_raw());
        dir.add_entry(ico::IconDirEntry::encode(&ii)?);
    }
    dir.write(File::create(root.join("icon.ico"))?)?;
    println!("wrote {} and icon.png", root.join("icon.ico").display());
    Ok(())
}
