//! Personal trade log: records that the user bought or sold a commodity at a
//! terminal for a given price/quantity. Mirrors UEX's user-trades journal and
//! can be pushed to UEX via `user_trades_add`. Persisted locally as JSON.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Buy or sell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TradeOp {
    Buy,
    Sell,
}

impl TradeOp {
    pub fn as_str(&self) -> &'static str {
        match self {
            TradeOp::Buy => "buy",
            TradeOp::Sell => "sell",
        }
    }
}

/// Sync state of a trade entry with UEX.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TradeSync {
    /// Only stored locally.
    Local,
    /// A submission to UEX is in flight.
    Pending,
    /// Prepared in dry-run mode (not sent).
    DryRun,
    /// Accepted by UEX (see `uex_id`).
    Synced,
    /// UEX rejected it (see `status_text`).
    Failed,
}

impl TradeSync {
    pub fn label(&self) -> &'static str {
        match self {
            TradeSync::Local => "Local only",
            TradeSync::Pending => "Sending…",
            TradeSync::DryRun => "Dry run",
            TradeSync::Synced => "Synced to UEX",
            TradeSync::Failed => "Sync failed",
        }
    }
}

/// A single logged buy/sell.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TradeEntry {
    pub id: String,
    pub operation: TradeOp,
    pub id_commodity: u32,
    pub commodity_name: String,
    pub id_terminal: u32,
    pub terminal_name: String,
    #[serde(default)]
    pub terminal_system: String,
    /// Quantity in SCU.
    pub scu: u32,
    /// Price per SCU in aUEC.
    pub price: f64,
    /// Unix timestamp when the trade happened / was logged.
    pub created_at: i64,

    // --- UEX sync ---
    pub sync: TradeSync,
    /// UEX `id_user_trade` returned on a successful sync.
    #[serde(default)]
    pub uex_id: Option<i64>,
    #[serde(default)]
    pub status_text: String,
}

impl TradeEntry {
    /// Total value of the transaction (price × SCU), in aUEC.
    pub fn total(&self) -> f64 {
        self.price * self.scu as f64
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn new_id() -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("t{}-{}", chrono::Utc::now().timestamp_millis(), n)
}

/// Create a new local trade entry (unsynced).
#[allow(clippy::too_many_arguments)]
pub fn new_entry(
    operation: TradeOp,
    id_commodity: u32,
    commodity_name: impl Into<String>,
    id_terminal: u32,
    terminal_name: impl Into<String>,
    terminal_system: impl Into<String>,
    scu: u32,
    price: f64,
    created_at: i64,
) -> TradeEntry {
    TradeEntry {
        id: new_id(),
        operation,
        id_commodity,
        commodity_name: commodity_name.into(),
        id_terminal,
        terminal_name: terminal_name.into(),
        terminal_system: terminal_system.into(),
        scu,
        price,
        created_at,
        sync: TradeSync::Local,
        uex_id: None,
        status_text: String::new(),
    }
}

/// Persistent trade log backed by a JSON file.
#[derive(Debug, Default)]
pub struct TradeLog {
    path: Option<PathBuf>,
    entries: Vec<TradeEntry>,
}

impl TradeLog {
    pub fn in_memory() -> Self {
        TradeLog { path: None, entries: Vec::new() }
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let entries = if path.exists() {
            let txt = std::fs::read_to_string(path)?;
            if txt.trim().is_empty() {
                Vec::new()
            } else {
                serde_json::from_str(&txt)?
            }
        } else {
            Vec::new()
        };
        Ok(TradeLog { path: Some(path.to_path_buf()), entries })
    }

    pub fn entries(&self) -> &[TradeEntry] {
        &self.entries
    }

    /// Entries newest-first.
    pub fn newest_first(&self) -> Vec<&TradeEntry> {
        let mut v: Vec<&TradeEntry> = self.entries.iter().collect();
        v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        v
    }

    pub fn add(&mut self, entry: TradeEntry) -> anyhow::Result<String> {
        let id = entry.id.clone();
        self.entries.push(entry);
        self.save()?;
        Ok(id)
    }

    pub fn remove(&mut self, id: &str) -> anyhow::Result<()> {
        self.entries.retain(|e| e.id != id);
        self.save()
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut TradeEntry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    /// Reset any entries left in the [`TradeSync::Pending`] state (e.g. the app
    /// closed mid-submission) back to local, so they aren't stuck "Sending…".
    pub fn reset_pending(&mut self) {
        let mut changed = false;
        for e in &mut self.entries {
            if e.sync == TradeSync::Pending {
                e.sync = TradeSync::Local;
                changed = true;
            }
        }
        if changed {
            let _ = self.save();
        }
    }

    /// Persist after a mutation to an entry made via [`Self::get_mut`].
    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(path) = &self.path {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, serde_json::to_string_pretty(&self.entries)?)?;
        }
        Ok(())
    }

    /// Totals: (spent on buys, earned on sells, net = earned - spent).
    pub fn totals(&self) -> (f64, f64, f64) {
        let mut spent = 0.0;
        let mut earned = 0.0;
        for e in &self.entries {
            match e.operation {
                TradeOp::Buy => spent += e.total(),
                TradeOp::Sell => earned += e.total(),
            }
        }
        (spent, earned, earned - spent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(op: TradeOp, scu: u32, price: f64) -> TradeEntry {
        new_entry(op, 44, "Iron", 25, "Everus Harbor", "Stanton", scu, price, 1_700_000_000)
    }

    #[test]
    fn total_and_totals() {
        let mut log = TradeLog::in_memory();
        log.add(entry(TradeOp::Buy, 100, 2000.0)).unwrap();
        log.add(entry(TradeOp::Sell, 100, 2760.0)).unwrap();
        let (spent, earned, net) = log.totals();
        assert_eq!(spent, 200_000.0);
        assert_eq!(earned, 276_000.0);
        assert_eq!(net, 76_000.0);
    }

    #[test]
    fn round_trips_through_file() {
        let dir = std::env::temp_dir().join(format!("ruex_trades_{}", std::process::id()));
        let path = dir.join("trades.json");
        let _ = std::fs::remove_file(&path);
        {
            let mut log = TradeLog::load(&path).unwrap();
            log.add(entry(TradeOp::Buy, 50, 1500.0)).unwrap();
        }
        let reloaded = TradeLog::load(&path).unwrap();
        assert_eq!(reloaded.entries().len(), 1);
        assert_eq!(reloaded.entries()[0].scu, 50);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_entry() {
        let mut log = TradeLog::in_memory();
        let id = log.add(entry(TradeOp::Buy, 10, 100.0)).unwrap();
        assert_eq!(log.entries().len(), 1);
        log.remove(&id).unwrap();
        assert!(log.entries().is_empty());
    }
}
