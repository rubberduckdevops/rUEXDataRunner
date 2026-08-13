//! Thin wrapper around the bundled Tesseract binary + custom `eng_sc` model.
//!
//! We shell out to `tesseract.exe` rather than link a C library so the app has
//! no native build dependency. Word-level results (text, confidence, bounding
//! box, reading-order line id) are parsed from Tesseract's TSV output.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One recognised word, with its confidence and layout position.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    /// 0-100 recognition confidence.
    pub conf: u8,
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    /// Reading-order line identity `(block, paragraph, line)`.
    pub line: (i32, i32, i32),
    pub word_num: i32,
}

impl Word {
    pub fn center_y(&self) -> i32 {
        self.top + self.height / 2
    }
}

/// Where the Tesseract engine and language data live.
#[derive(Clone, Debug)]
pub struct OcrConfig {
    pub tesseract_exe: PathBuf,
    pub tessdata_dir: PathBuf,
    pub lang: String,
    pub psm: u8,
    /// Optional user word lists (concatenated) to bias recognition.
    pub user_words: Vec<PathBuf>,
    /// Optional user pattern file (e.g. price patterns).
    pub user_patterns: Option<PathBuf>,
}

impl OcrConfig {
    /// True when the configured Tesseract executable and language model exist.
    pub fn is_available(&self) -> bool {
        self.tesseract_exe.is_file()
            && self.tessdata_dir.join(format!("{}.traineddata", self.lang)).is_file()
    }
}

/// Run Tesseract over `image_path` and return the recognised words.
pub fn run_ocr(cfg: &OcrConfig, image_path: &Path) -> anyhow::Result<Vec<Word>> {
    if !cfg.tesseract_exe.is_file() {
        anyhow::bail!("tesseract executable not found: {}", cfg.tesseract_exe.display());
    }

    // Tesseract writes `<outbase>.tsv`; use a unique temp base so concurrent
    // OCR jobs (one per screenshot) never clobber each other.
    let uniq = unique_token();
    let out_base = std::env::temp_dir().join(format!("ruex_ocr_{uniq}"));
    let tsv_path = out_base.with_extension("tsv");

    let mut cmd = Command::new(&cfg.tesseract_exe);
    cmd.arg(image_path)
        .arg(&out_base)
        .arg("--tessdata-dir")
        .arg(&cfg.tessdata_dir)
        .arg("-l")
        .arg(&cfg.lang)
        .arg("--psm")
        .arg(cfg.psm.to_string())
        .arg("-c")
        .arg("tessedit_create_tsv=1");

    // On Windows, don't flash a console window for each Tesseract invocation.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    // Combine any user word lists into one temp file.
    let mut combined_words: Option<PathBuf> = None;
    if !cfg.user_words.is_empty() {
        let path = std::env::temp_dir().join(format!("ruex_words_{uniq}.txt"));
        let mut f = std::fs::File::create(&path)?;
        for w in &cfg.user_words {
            if let Ok(contents) = std::fs::read_to_string(w) {
                f.write_all(contents.as_bytes())?;
                f.write_all(b"\n")?;
            }
        }
        cmd.arg("--user-words").arg(&path);
        combined_words = Some(path);
    }
    if let Some(pat) = &cfg.user_patterns {
        if pat.is_file() {
            cmd.arg("--user-patterns").arg(pat);
        }
    }

    let output = cmd.output()?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&tsv_path);
        if let Some(w) = &combined_words {
            let _ = std::fs::remove_file(w);
        }
        anyhow::bail!(
            "tesseract failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let tsv = std::fs::read_to_string(&tsv_path)?;
    let _ = std::fs::remove_file(&tsv_path);
    if let Some(w) = &combined_words {
        let _ = std::fs::remove_file(w);
    }

    Ok(parse_tsv(&tsv))
}

/// A process-unique token for temp file names (pid + a monotonic counter).
fn unique_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!("{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))
}

/// Parse Tesseract TSV into words (level-5 rows with non-empty text).
pub fn parse_tsv(tsv: &str) -> Vec<Word> {
    let mut words = Vec::new();
    for (i, line) in tsv.lines().enumerate() {
        if i == 0 && line.starts_with("level") {
            continue; // header
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 12 {
            continue;
        }
        if cols[0] != "5" {
            continue; // only word rows
        }
        let text = cols[11].trim();
        if text.is_empty() {
            continue;
        }
        let conf = cols[10].parse::<f32>().unwrap_or(-1.0);
        if conf < 0.0 {
            continue;
        }
        let parse = |s: &str| s.parse::<i32>().unwrap_or(0);
        words.push(Word {
            text: text.to_string(),
            conf: conf.round().clamp(0.0, 100.0) as u8,
            left: parse(cols[6]),
            top: parse(cols[7]),
            width: parse(cols[8]),
            height: parse(cols[9]),
            line: (parse(cols[2]), parse(cols[3]), parse(cols[4])),
            word_num: parse(cols[5]),
        });
    }
    words
}

/// Group words into lines preserving reading order, joining each line's text.
pub fn lines_from_words(words: &[Word]) -> Vec<String> {
    let mut out: Vec<(i32, i32, i32, String)> = Vec::new();
    for w in words {
        match out.last_mut() {
            Some((b, p, l, s)) if (*b, *p, *l) == w.line => {
                s.push(' ');
                s.push_str(&w.text);
            }
            _ => out.push((w.line.0, w.line.1, w.line.2, w.text.clone())),
        }
    }
    out.into_iter().map(|(_, _, _, s)| s).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TSV: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
5\t1\t1\t1\t1\t1\t10\t20\t30\t12\t92.5\tBUENO\n\
5\t1\t1\t1\t1\t2\t50\t20\t30\t12\t91.0\tRAVINE\n\
5\t1\t1\t1\t1\t3\t90\t20\t10\t12\t-1\t \n\
5\t1\t1\t1\t2\t1\t10\t40\t40\t12\t88.0\tMax\n\
5\t1\t1\t1\t2\t2\t60\t40\t60\t12\t80.0\tInventory";

    #[test]
    fn parses_words_and_skips_empty_and_negative_conf() {
        let words = parse_tsv(SAMPLE_TSV);
        assert_eq!(words.len(), 4);
        assert_eq!(words[0].text, "BUENO");
        assert_eq!(words[0].conf, 93); // 92.5 rounds to 93
        assert_eq!(words[3].text, "Inventory");
    }

    #[test]
    fn groups_lines_in_reading_order() {
        let words = parse_tsv(SAMPLE_TSV);
        let lines = lines_from_words(&words);
        assert_eq!(lines, vec!["BUENO RAVINE".to_string(), "Max Inventory".to_string()]);
    }
}
