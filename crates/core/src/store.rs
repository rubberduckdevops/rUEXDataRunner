//! Local persistence of submitted reports and their status — the feature that
//! lets the user review and re-submit corrections from inside the app instead of
//! going to the UEX website.
//!
//! Each [`SubmittedReport`] captures the exact data sent, the UEX response
//! (status + report ids), and links to any earlier report it supersedes, so the
//! app can show a history and let the user push an update.

use crate::api::SubmitResponse;
use crate::model::{Commodity, Extraction, TerminalType};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Lifecycle state of a saved report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportState {
    /// Successfully submitted to UEX and published.
    Submitted,
    /// Prepared in dry-run mode; nothing was sent.
    DryRun,
    /// Submission failed (see `status_text`).
    Failed,
    /// A later corrected report has replaced this one.
    Updated,
}

impl ReportState {
    pub fn label(&self) -> &'static str {
        match self {
            ReportState::Submitted => "Submitted",
            ReportState::DryRun => "Dry run",
            ReportState::Failed => "Failed",
            ReportState::Updated => "Updated (superseded)",
        }
    }
}

/// A single saved submission.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubmittedReport {
    /// Local unique id (not the UEX report id).
    pub id: String,
    pub terminal_name: String,
    pub id_terminal: u32,
    pub terminal_type: TerminalType,
    pub screenshot: String,
    /// Snapshot of the commodity rows exactly as submitted.
    pub commodities: Vec<Commodity>,
    pub state: ReportState,
    /// Human-readable status/message from the API (or dry-run note).
    pub status_text: String,
    /// UEX report ids returned on success.
    pub ids_reports: Vec<String>,
    pub username: Option<String>,
    pub dry_run: bool,
    /// Unix timestamp when this report was created.
    pub submitted_at: i64,
    /// Local id of an earlier report this one corrects, if any.
    pub supersedes: Option<String>,
}

impl SubmittedReport {
    /// Rebuild an [`Extraction`] from a saved report so it can be edited and
    /// re-submitted.
    pub fn to_extraction(&self) -> Extraction {
        let mut ex = Extraction::new(self.screenshot.clone());
        ex.terminal_name = Some(self.terminal_name.clone());
        ex.id_terminal = Some(self.id_terminal);
        ex.terminal_type = Some(self.terminal_type);
        ex.commodities = self.commodities.clone();
        ex.captured_at = Some(self.submitted_at);
        ex
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn new_local_id() -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = chrono::Utc::now().timestamp_millis();
    format!("r{now}-{n}")
}

/// Build a [`SubmittedReport`] from an extraction and the API response.
pub fn report_from(
    ex: &Extraction,
    resp: &SubmitResponse,
    supersedes: Option<String>,
) -> SubmittedReport {
    let state = if resp.dry_run {
        ReportState::DryRun
    } else if resp.is_ok() {
        ReportState::Submitted
    } else {
        ReportState::Failed
    };
    let status_text = if resp.message.is_empty() {
        resp.status.clone()
    } else {
        format!("{} — {}", resp.status, resp.message)
    };
    SubmittedReport {
        id: new_local_id(),
        terminal_name: ex.terminal_name.clone().unwrap_or_default(),
        id_terminal: ex.id_terminal.unwrap_or(0),
        terminal_type: ex.terminal_type.unwrap_or(TerminalType::Sell),
        screenshot: ex.screenshot.clone(),
        commodities: ex.commodities.clone(),
        state,
        status_text,
        ids_reports: resp.ids_reports.clone(),
        username: resp.username.clone(),
        dry_run: resp.dry_run,
        submitted_at: ex.captured_at.unwrap_or_else(|| chrono::Utc::now().timestamp()),
        supersedes,
    }
}

/// Persistent collection of submitted reports, backed by a JSON file.
#[derive(Debug, Default)]
pub struct ReportStore {
    path: Option<PathBuf>,
    reports: Vec<SubmittedReport>,
}

impl ReportStore {
    /// An in-memory store with no backing file (for tests).
    pub fn in_memory() -> Self {
        ReportStore { path: None, reports: Vec::new() }
    }

    /// Load from `path`, creating an empty store if the file doesn't exist.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let reports = if path.exists() {
            let txt = std::fs::read_to_string(path)?;
            if txt.trim().is_empty() {
                Vec::new()
            } else {
                serde_json::from_str(&txt)?
            }
        } else {
            Vec::new()
        };
        Ok(ReportStore { path: Some(path.to_path_buf()), reports })
    }

    pub fn reports(&self) -> &[SubmittedReport] {
        &self.reports
    }

    pub fn get(&self, id: &str) -> Option<&SubmittedReport> {
        self.reports.iter().find(|r| r.id == id)
    }

    /// Add a report and persist. If it supersedes another, that one is marked
    /// [`ReportState::Updated`]. Returns the new report's local id.
    pub fn add(&mut self, report: SubmittedReport) -> anyhow::Result<String> {
        if let Some(prev_id) = &report.supersedes {
            if let Some(prev) = self.reports.iter_mut().find(|r| &r.id == prev_id) {
                prev.state = ReportState::Updated;
            }
        }
        let id = report.id.clone();
        self.reports.push(report);
        self.save()?;
        Ok(id)
    }

    /// The most recent, non-superseded reports, newest first.
    pub fn active(&self) -> Vec<&SubmittedReport> {
        let mut v: Vec<&SubmittedReport> =
            self.reports.iter().filter(|r| r.state != ReportState::Updated).collect();
        v.sort_by(|a, b| b.submitted_at.cmp(&a.submitted_at));
        v
    }

    fn save(&self) -> anyhow::Result<()> {
        if let Some(path) = &self.path {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let txt = serde_json::to_string_pretty(&self.reports)?;
            std::fs::write(path, txt)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::SubmitResponse;

    fn ok_response(ids: &[&str]) -> SubmitResponse {
        SubmitResponse {
            status: "ok".into(),
            http_code: 200,
            ids_reports: ids.iter().map(|s| s.to_string()).collect(),
            username: Some("tester".into()),
            message: String::new(),
            date_added: Some(1_786_594_856),
            dry_run: false,
        }
    }

    fn sample_extraction() -> Extraction {
        let mut ex = Extraction::new("s.jpg");
        ex.terminal_name = Some("Bueno Ravine".into());
        ex.id_terminal = Some(551);
        ex.terminal_type = Some(TerminalType::Buy);
        ex.captured_at = Some(1_786_594_856);
        let mut c = Commodity::new("Diamond");
        c.id_commodity = Some(25);
        c.price = Some(6561);
        ex.commodities.push(c);
        ex
    }

    #[test]
    fn report_captures_response() {
        let ex = sample_extraction();
        let r = report_from(&ex, &ok_response(&["958660", "958662"]), None);
        assert_eq!(r.state, ReportState::Submitted);
        assert_eq!(r.ids_reports, vec!["958660", "958662"]);
        assert_eq!(r.id_terminal, 551);
        assert_eq!(r.terminal_type, TerminalType::Buy);
    }

    #[test]
    fn supersede_marks_previous_updated() {
        let mut store = ReportStore::in_memory();
        let ex = sample_extraction();
        let first = report_from(&ex, &ok_response(&["1"]), None);
        let first_id = store.add(first).unwrap();

        let mut corrected = report_from(&ex, &ok_response(&["2"]), Some(first_id.clone()));
        corrected.commodities[0].price = Some(7000);
        store.add(corrected).unwrap();

        assert_eq!(store.get(&first_id).unwrap().state, ReportState::Updated);
        // Only the corrected report is "active".
        let active = store.active();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].commodities[0].price, Some(7000));
    }

    #[test]
    fn round_trips_through_file() {
        let dir = std::env::temp_dir().join(format!("ruex_store_{}", std::process::id()));
        let path = dir.join("reports.json");
        let _ = std::fs::remove_file(&path);

        {
            let mut store = ReportStore::load(&path).unwrap();
            let ex = sample_extraction();
            store.add(report_from(&ex, &ok_response(&["77"]), None)).unwrap();
        }
        let reloaded = ReportStore::load(&path).unwrap();
        assert_eq!(reloaded.reports().len(), 1);
        assert_eq!(reloaded.reports()[0].ids_reports, vec!["77"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn to_extraction_round_trip() {
        let ex = sample_extraction();
        let r = report_from(&ex, &ok_response(&["1"]), None);
        let back = r.to_extraction();
        assert_eq!(back.id_terminal, Some(551));
        assert_eq!(back.commodities[0].id_commodity, Some(25));
    }
}
