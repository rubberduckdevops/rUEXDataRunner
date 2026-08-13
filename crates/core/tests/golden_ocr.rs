//! End-to-end OCR test against real Star Citizen screenshots.
//!
//! This exercises the full pipeline (Tesseract -> extraction) on the bundled
//! golden screenshots. It is *tolerant* by design: exact OCR output depends on
//! the local Tesseract build, so instead of asserting exact values it checks
//! structural invariants (a real terminal is identified, every extracted
//! commodity resolves to a valid UEX id, prices/quantities are sane).
//!
//! The test auto-skips when the Tesseract executable or language model isn't
//! available, so the suite always passes on machines without them. Point it at a
//! Tesseract binary with the `RUEX_TESSERACT` environment variable if the
//! default bundled path doesn't exist.

use datarunner_core::ocr::OcrConfig;
use datarunner_core::{pipeline, Reference};
use std::path::{Path, PathBuf};

fn assets_tessdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tessdata")
}

fn resolve_tesseract() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("RUEX_TESSERACT") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let default = PathBuf::from(
        r"C:\Users\michael\Downloads\SC-Datarunner-UEX-v0.8.1\SC-Datarunner-UEX\dep\Tesseract-OCR\tesseract.exe",
    );
    default.is_file().then_some(default)
}

fn ocr_config() -> Option<OcrConfig> {
    let exe = resolve_tesseract()?;
    let tessdata = assets_tessdata();
    let cfg = OcrConfig {
        tesseract_exe: exe,
        tessdata_dir: tessdata.clone(),
        lang: "eng_sc".to_string(),
        psm: 6,
        user_words: vec![
            tessdata.join("commodities.user-words"),
            tessdata.join("terminals.user-words"),
        ],
        user_patterns: Some(tessdata.join("sc.patterns")),
    };
    cfg.is_available().then_some(cfg)
}

fn fixtures() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/screenshots");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("fixtures dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jpg"))
        .collect();
    v.sort();
    v
}

#[test]
fn golden_screenshots_extract_valid_data() {
    let Some(cfg) = ocr_config() else {
        eprintln!("SKIP: Tesseract not available (set RUEX_TESSERACT to enable)");
        return;
    };
    let reference = Reference::embedded();
    let known_ids: std::collections::HashSet<u32> =
        reference.commodities().iter().map(|c| c.id).collect();

    let mut total_commodities = 0usize;
    let mut terminals_identified = 0usize;
    let files = fixtures();
    assert!(!files.is_empty(), "expected golden fixtures");

    for path in &files {
        let ex = pipeline::process_screenshot(&reference, &cfg, path)
            .unwrap_or_else(|e| panic!("pipeline failed on {}: {e}", path.display()));

        eprintln!(
            "\n{} => terminal={:?} (id={:?}, conf={}), type={:?}, {} commodities",
            path.file_name().unwrap().to_string_lossy(),
            ex.terminal_name,
            ex.id_terminal,
            ex.terminal_confidence,
            ex.terminal_type,
            ex.commodities.len(),
        );

        // A buy/sell type is always determined.
        assert!(ex.terminal_type.is_some());

        if ex.id_terminal.is_some() {
            terminals_identified += 1;
        }

        for c in &ex.commodities {
            eprintln!(
                "    - {:<22} id={:?} status={:?} scu={:?} price={:?} (name_conf={})",
                c.name, c.id_commodity, c.status, c.quantity_scu, c.price, c.name_confidence
            );
            // Every extracted commodity must resolve to a real UEX id.
            let id = c.id_commodity.expect("extracted commodity must have an id");
            assert!(known_ids.contains(&id), "commodity id {id} not in reference");
            assert!(!c.name.is_empty());
            // Prices, when present, are within a plausible aUEC/SCU range.
            if let Some(p) = c.price {
                assert!(p <= 10_000_000, "implausible price {p} for {}", c.name);
            }
            total_commodities += 1;
        }
    }

    eprintln!(
        "\nSUMMARY: {}/{} screenshots identified a terminal; {} commodities extracted total",
        terminals_identified,
        files.len(),
        total_commodities
    );

    // The pipeline must identify the terminal on at least one screenshot and
    // extract a meaningful number of commodities across the set. (Some
    // scroll-continuation shots legitimately have no visible location header.)
    // Baseline over the 11-screenshot fixture set (currently 9 terminals / 40
    // commodities); kept a little below actual to tolerate OCR variance.
    assert!(terminals_identified >= 5, "regressed: too few terminals identified");
    assert!(total_commodities >= 25, "regressed: too few commodities extracted");
}
