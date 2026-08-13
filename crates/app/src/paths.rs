//! Resolve default locations for the bundled OCR assets and the Tesseract
//! executable, so the app works out-of-the-box on this machine and stays
//! configurable everywhere else.

use std::path::{Path, PathBuf};

/// Directory holding `eng_sc.traineddata` and the user word/pattern files.
///
/// Search order: `RUEX_ASSETS` env, `<exe>/assets/tessdata`,
/// `<exe>/../../assets/tessdata` (dev build), then the compile-time workspace
/// path baked in via `CARGO_MANIFEST_DIR`.
pub fn default_tessdata_dir() -> PathBuf {
    if let Ok(a) = std::env::var("RUEX_ASSETS") {
        let p = PathBuf::from(a).join("tessdata");
        if p.is_dir() {
            return p;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for cand in [dir.join("assets/tessdata"), dir.join("../../assets/tessdata")] {
                if cand.is_dir() {
                    return cand;
                }
            }
        }
    }
    let compiled = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tessdata");
    compiled
}

/// Best-guess Tesseract executable.
///
/// Search order: `RUEX_TESSERACT` env, the bundled reference build's
/// `dep/Tesseract-OCR/tesseract.exe`, then bare `tesseract` (PATH).
pub fn default_tesseract_exe() -> PathBuf {
    if let Ok(t) = std::env::var("RUEX_TESSERACT") {
        let p = PathBuf::from(t);
        if p.is_file() {
            return p;
        }
    }
    // Portable layout: Tesseract-OCR folder next to the exe.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for cand in [
                dir.join("Tesseract-OCR/tesseract.exe"),
                dir.join("dep/Tesseract-OCR/tesseract.exe"),
            ] {
                if cand.is_file() {
                    return cand;
                }
            }
        }
    }
    // Dev fallback: the reference build's bundled copy.
    let bundled = PathBuf::from(
        r"C:\Users\michael\Downloads\SC-Datarunner-UEX-v0.8.1\SC-Datarunner-UEX\dep\Tesseract-OCR\tesseract.exe",
    );
    if bundled.is_file() {
        return bundled;
    }
    PathBuf::from("tesseract")
}

/// When running as a portable install, the base folder (next to the exe) that
/// holds config + data. Enabled by a `portable.txt` marker or an existing
/// `data/` folder beside the exe.
fn portable_base() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    if dir.join("portable.txt").is_file() || dir.join("data").is_dir() {
        Some(dir.to_path_buf())
    } else {
        None
    }
}

/// Directory for app-managed data (reports, pending captures, trade log).
/// Portable install: `<exe>/data`; otherwise `%APPDATA%\uex\rUEXDataRunner\data`.
pub fn data_dir() -> PathBuf {
    if let Some(base) = portable_base() {
        return base.join("data");
    }
    directories::ProjectDirs::from("space", "uex", "rUEXDataRunner")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Config file location. Portable install: `<exe>/config/config.json`;
/// otherwise the OS config dir.
pub fn config_path() -> PathBuf {
    if let Some(base) = portable_base() {
        return base.join("config").join("config.json");
    }
    datarunner_core::config::Config::default_path()
}
