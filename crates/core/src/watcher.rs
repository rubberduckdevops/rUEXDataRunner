//! Watches the Star Citizen screenshots folder and reports new image files.

use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

/// A running folder watcher. New screenshot paths arrive on [`Self::rx`].
pub struct ScreenshotWatcher {
    _watcher: notify::RecommendedWatcher,
    pub rx: Receiver<PathBuf>,
}

fn is_screenshot(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("jpg") | Some("jpeg") | Some("png")
    )
}

/// Start watching `dir` for newly created/modified screenshots.
pub fn watch(dir: &Path) -> anyhow::Result<ScreenshotWatcher> {
    let (tx, rx) = channel::<PathBuf>();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res {
            if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                for path in event.paths {
                    if is_screenshot(&path) {
                        let _ = tx.send(path);
                    }
                }
            }
        }
    })?;

    watcher.watch(dir, RecursiveMode::NonRecursive)?;
    Ok(ScreenshotWatcher { _watcher: watcher, rx })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_image_extensions() {
        assert!(is_screenshot(Path::new("a/ScreenShot-1.jpg")));
        assert!(is_screenshot(Path::new("a/ScreenShot-1.PNG")));
        assert!(!is_screenshot(Path::new("a/app.log")));
        assert!(!is_screenshot(Path::new("a/reports.json")));
    }
}
