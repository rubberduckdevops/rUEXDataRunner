//! Debug helper: OCR a screenshot (raw vs. preprocessed) and print the words,
//! terminal detection, and extraction so we can see why fields are missed.
//!
//! Usage: cargo run -p datarunner-core --example dump_ocr -- <image> [--prep]

use datarunner_core::ocr::{self, OcrConfig};
use datarunner_core::{extract, pipeline, preprocess, Reference};
use std::path::PathBuf;

fn cfg(tessdata: &std::path::Path) -> OcrConfig {
    let exe = std::env::var("RUEX_TESSERACT").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(r"C:\Users\michael\Downloads\SC-Datarunner-UEX-v0.8.1\SC-Datarunner-UEX\dep\Tesseract-OCR\tesseract.exe")
    });
    OcrConfig {
        tesseract_exe: exe,
        tessdata_dir: tessdata.to_path_buf(),
        lang: "eng_sc".into(),
        psm: 6,
        user_words: vec![
            tessdata.join("commodities.user-words"),
            tessdata.join("terminals.user-words"),
        ],
        user_patterns: Some(tessdata.join("sc.patterns")),
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let img = args.get(1).cloned().unwrap_or_else(|| {
        format!(
            "{}/tests/fixtures/screenshots/ScreenShot-2026-08-12_22-00-46-924.jpg",
            env!("CARGO_MANIFEST_DIR")
        )
    });
    let prep = args.iter().any(|a| a == "--prep");
    let psm: u8 = args
        .iter()
        .position(|a| a == "--psm")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);
    let scale: f32 = args
        .iter()
        .position(|a| a == "--scale")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(2.0);

    let tessdata = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/tessdata");
    let mut c = cfg(&tessdata);
    c.psm = psm;
    let reference = Reference::embedded();

    let path = PathBuf::from(&img);
    let ocr_path = if prep {
        let p = preprocess::prepare_for_ocr(&path, scale)?;
        eprintln!("(preprocessed scale={scale} -> {})", p.display());
        p
    } else {
        path.clone()
    };

    let words = ocr::run_ocr(&c, &ocr_path)?;
    println!("== {} words (psm={psm}, prep={prep}) ==", words.len());
    for w in &words {
        println!(
            "  '{}'  conf={} @({},{}) {}x{} line={:?}",
            w.text, w.conf, w.left, w.top, w.width, w.height, w.line
        );
    }

    // Terminal candidates: show top matches by scanning short line windows.
    println!("\n== terminal detection ==");
    let mut best: Vec<(String, u8, u32)> = Vec::new();
    for line in ocr::lines_from_words(&words) {
        if let Some(m) = reference.best_terminal(&line) {
            best.push((line.clone(), m.score, m.id));
        }
    }
    best.sort_by(|a, b| b.1.cmp(&a.1));
    for (line, score, id) in best.iter().take(6) {
        println!("  score={score} id={id}  <= \"{line}\"");
    }

    let ex = extract::extract(&reference, &words, img.clone(), pipeline::parse_capture_time(&path));
    println!("\n== single-pass extraction ==");
    println!("terminal = {:?} (id {:?}, conf {})", ex.terminal_name, ex.id_terminal, ex.terminal_confidence);
    println!("type = {:?}", ex.terminal_type);
    for cm in &ex.commodities {
        println!(
            "  {:<22} id={:?} status={:?} scu={:?} price={:?} name_conf={} price_conf={}",
            cm.name, cm.id_commodity, cm.status, cm.quantity_scu, cm.price, cm.name_confidence, cm.price_confidence
        );
    }

    // Full merged pipeline (raw + cropped panel) — the app's real output.
    let t = std::time::Instant::now();
    let merged = pipeline::process_screenshot(&reference, &c, &path)?;
    println!("\n== MERGED PIPELINE ({}ms) ==", t.elapsed().as_millis());
    println!(
        "terminal = {:?} (id {:?}, conf {}), type={:?}",
        merged.terminal_name, merged.id_terminal, merged.terminal_confidence, merged.terminal_type
    );
    for cm in &merged.commodities {
        println!(
            "  {:<22} id={:?} status={:?} scu={:?} price={:?} name_conf={} price_conf={} rowconf={}",
            cm.name, cm.id_commodity, cm.status, cm.quantity_scu, cm.price,
            cm.name_confidence, cm.price_confidence, cm.min_confidence()
        );
    }
    Ok(())
}
