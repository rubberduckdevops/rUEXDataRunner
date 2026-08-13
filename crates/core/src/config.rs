//! User settings, persisted as JSON under the OS config directory
//! (`%APPDATA%\rUEXDataRunner\config.json` on Windows).

use crate::api::DEFAULT_BASE_URL;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Persistent application settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// UEX personal secret key (from the UEX account page).
    #[serde(default)]
    pub secret_key: String,
    /// UEX application API token (Bearer) from https://uexcorp.space/api/apps.
    /// Required by UEX 2.0 to authorize the app; without it submits are rejected
    /// with "not_allowed".
    #[serde(default)]
    pub api_token: String,
    /// Folder watched for new Star Citizen screenshots.
    #[serde(default)]
    pub screenshot_dir: PathBuf,
    /// Game environment label, "LIVE" or "PTU".
    #[serde(default = "default_env")]
    pub environment: String,
    /// Optional explicit game version string sent to UEX (e.g. "LIVE 4.9").
    #[serde(default)]
    pub game_version: String,
    /// When true, submissions are prepared but never sent (safe default).
    #[serde(default = "default_true")]
    pub dry_run: bool,
    /// When actually submitting, publish (`is_production=1`) vs test row (`0`).
    #[serde(default = "default_true")]
    pub is_production: bool,
    /// Delete the source screenshot after a successful send.
    #[serde(default)]
    pub delete_after_send: bool,
    /// Path to the Tesseract executable.
    #[serde(default)]
    pub tesseract_exe: PathBuf,
    /// Directory containing `eng_sc.traineddata` and the user word/pattern files.
    #[serde(default)]
    pub tessdata_dir: PathBuf,
    /// UEX API base URL (override for testing).
    #[serde(default = "default_base_url")]
    pub base_url: String,
}

fn default_env() -> String {
    "LIVE".to_string()
}
fn default_true() -> bool {
    true
}
fn default_base_url() -> String {
    DEFAULT_BASE_URL.to_string()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            secret_key: String::new(),
            api_token: String::new(),
            screenshot_dir: default_screenshot_dir(),
            environment: default_env(),
            game_version: String::new(),
            dry_run: true,
            is_production: true,
            delete_after_send: false,
            tesseract_exe: PathBuf::new(),
            tessdata_dir: PathBuf::new(),
            base_url: default_base_url(),
        }
    }
}

/// Best-guess default Star Citizen LIVE screenshots directory on Windows.
pub fn default_screenshot_dir() -> PathBuf {
    PathBuf::from(
        r"C:\Program Files\Roberts Space Industries\StarCitizen\LIVE\screenshots",
    )
}

impl Config {
    /// Standard config file location for this app.
    pub fn default_path() -> PathBuf {
        if let Some(dirs) = directories::ProjectDirs::from("space", "uex", "rUEXDataRunner") {
            dirs.config_dir().join("config.json")
        } else {
            PathBuf::from("config.json")
        }
    }

    /// Load config from `path`, falling back to defaults if it doesn't exist.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let txt = std::fs::read_to_string(path)?;
        if txt.trim().is_empty() {
            return Ok(Config::default());
        }
        Ok(serde_json::from_str(&txt)?)
    }

    /// Persist config to `path`.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// True when enough is configured to actually run OCR and submit live.
    pub fn is_ready(&self) -> bool {
        self.screenshot_dir.is_dir() && self.tesseract_exe.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_safe() {
        let c = Config::default();
        assert!(c.dry_run, "dry-run must default on for safety");
        assert_eq!(c.base_url, DEFAULT_BASE_URL);
        assert_eq!(c.environment, "LIVE");
    }

    #[test]
    fn round_trips_and_tolerates_missing_fields() {
        let dir = std::env::temp_dir().join(format!("ruex_cfg_{}", std::process::id()));
        let path = dir.join("config.json");
        let mut c = Config::default();
        c.secret_key = "abc123".into();
        c.dry_run = false;
        c.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.secret_key, "abc123");
        assert!(!loaded.dry_run);

        // A partial config file still loads, filling defaults.
        std::fs::write(&path, r#"{"secret_key":"xyz"}"#).unwrap();
        let partial = Config::load(&path).unwrap();
        assert_eq!(partial.secret_key, "xyz");
        assert!(partial.dry_run); // default_true
        assert_eq!(partial.base_url, DEFAULT_BASE_URL);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
