//! Debug helper: detect + rectify the commodity panel and save an overlay and
//! the warped panel so the geometry can be checked by eye.
//!
//! Usage: cargo run -p datarunner-core --example rectify -- <image> <out_dir> [left|right]

use datarunner_core::deskew::{self, Side};
use image::{Rgba, RgbaImage};
use std::path::PathBuf;

fn draw_line(img: &mut RgbaImage, a: (f32, f32), b: (f32, f32), color: Rgba<u8>) {
    let steps = (deskew_dist(a, b) as i32).max(1);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = (a.0 + (b.0 - a.0) * t).round() as i32;
        let y = (a.1 + (b.1 - a.1) * t).round() as i32;
        for dx in -3..=3 {
            for dy in -3..=3 {
                let (px, py) = (x + dx, y + dy);
                if px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height() {
                    img.put_pixel(px as u32, py as u32, color);
                }
            }
        }
    }
}

fn deskew_dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let img_path = PathBuf::from(args.get(1).expect("image path"));
    let out_dir = PathBuf::from(args.get(2).cloned().unwrap_or_else(|| ".".into()));
    let side = match args.get(3).map(|s| s.as_str()) {
        Some("left") => Side::Left,
        _ => Side::Right,
    };
    std::fs::create_dir_all(&out_dir)?;
    let stem = img_path.file_stem().unwrap().to_string_lossy().to_string();

    let mut img = image::open(&img_path)?.to_rgba8();
    match deskew::detect_panel(&img, side) {
        Some(quad) => {
            println!("quad = {quad:?}");
            let rect = deskew::rectify_to_gray(&img, &quad, 1.6);
            let rect_path = out_dir.join(format!("{stem}_rect.png"));
            rect.save(&rect_path)?;
            println!("rectified -> {}", rect_path.display());

            let red = Rgba([255, 40, 40, 255]);
            draw_line(&mut img, quad[0], quad[1], red);
            draw_line(&mut img, quad[1], quad[2], red);
            draw_line(&mut img, quad[2], quad[3], red);
            draw_line(&mut img, quad[3], quad[0], red);
            let ov_path = out_dir.join(format!("{stem}_overlay.png"));
            // Downscale overlay for quick viewing.
            let small = image::imageops::resize(&img, 1200, 675, image::imageops::FilterType::Triangle);
            small.save(&ov_path)?;
            println!("overlay -> {}", ov_path.display());
        }
        None => println!("no panel detected (would use fixed-region crop)"),
    }

    // Always also emit the fixed-region crop (the fallback OCR input).
    if let Ok(crop) = deskew::crop_region_to_temp(&img_path, side, 1.6) {
        let dst = out_dir.join(format!("{stem}_crop.png"));
        std::fs::copy(&crop, &dst)?;
        let _ = std::fs::remove_file(&crop);
        println!("crop -> {}", dst.display());
    }

    // Colour panel (what the in-app preview shows).
    if let Ok(color) = deskew::panel_color(&img_path, side, 1.0) {
        let dst = out_dir.join(format!("{stem}_preview.png"));
        color.save(&dst)?;
        println!("preview -> {}", dst.display());
    }
    Ok(())
}
