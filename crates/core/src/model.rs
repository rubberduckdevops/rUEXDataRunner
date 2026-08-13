//! Core data model shared across OCR extraction, review UI, and API submission.

use serde::{Deserialize, Serialize};

/// Whether a trade terminal screen is showing the player *buying* from the shop
/// or *selling* to the shop. This determines which UEX price/scu/status fields
/// get populated on submission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalType {
    Buy,
    Sell,
}

impl TerminalType {
    pub fn as_str(&self) -> &'static str {
        match self {
            TerminalType::Buy => "buy",
            TerminalType::Sell => "sell",
        }
    }
}

/// A single commodity row extracted from a terminal screenshot.
///
/// Every OCR-derived field carries a 0-100 confidence so the UI can flag
/// low-confidence values for the user to correct before submitting.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Commodity {
    /// Best-matched UEX commodity name (canonical spelling).
    pub name: String,
    /// UEX commodity id, if the name matched a known commodity.
    pub id_commodity: Option<u32>,
    pub name_confidence: u8,

    /// Available cargo size / shop quantity in SCU.
    pub quantity_scu: Option<u32>,
    pub quantity_confidence: u8,

    /// Human-readable stock label as shown in game (e.g. "max inventory").
    pub stock_label: Option<String>,
    /// UEX inventory status 1..=7 (see [`crate::status`]).
    pub status: Option<u8>,

    /// Unit price in aUEC per SCU.
    pub price: Option<u32>,
    pub price_confidence: u8,

    /// Whether this row is included in a "Send All" batch.
    pub include: bool,
}

impl Commodity {
    pub fn new(name: impl Into<String>) -> Self {
        Commodity {
            name: name.into(),
            id_commodity: None,
            name_confidence: 0,
            quantity_scu: None,
            quantity_confidence: 0,
            stock_label: None,
            status: None,
            price: None,
            price_confidence: 0,
            include: true,
        }
    }

    /// The lowest confidence among the *measured* fields; used by the UI to
    /// decide whether to highlight a row for review. A confidence of 0 means
    /// "not measured / manually entered" and is ignored so it never drags the
    /// row's confidence down artificially.
    pub fn min_confidence(&self) -> u8 {
        let mut c = self.name_confidence;
        if self.quantity_scu.is_some() && self.quantity_confidence > 0 {
            c = c.min(self.quantity_confidence);
        }
        if self.price.is_some() && self.price_confidence > 0 {
            c = c.min(self.price_confidence);
        }
        c
    }
}

/// The full result of processing one screenshot: which terminal it is, buy/sell,
/// and the commodity rows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Extraction {
    /// Source screenshot path (as a string for easy serialization).
    pub screenshot: String,
    /// Matched terminal display name.
    pub terminal_name: Option<String>,
    /// Matched UEX terminal id.
    pub id_terminal: Option<u32>,
    pub terminal_confidence: u8,
    /// Candidate terminals sharing this location, for the disambiguation dropdown.
    pub terminal_candidates: Vec<TerminalRef>,
    pub terminal_type: Option<TerminalType>,
    pub commodities: Vec<Commodity>,
    /// Unix timestamp parsed from the screenshot filename, if available.
    pub captured_at: Option<i64>,
}

impl Extraction {
    pub fn new(screenshot: impl Into<String>) -> Self {
        Extraction {
            screenshot: screenshot.into(),
            terminal_name: None,
            id_terminal: None,
            terminal_confidence: 0,
            terminal_candidates: Vec::new(),
            terminal_type: None,
            commodities: Vec::new(),
            captured_at: None,
        }
    }

    /// Ready to submit only when we know the terminal, its type, and have at
    /// least one commodity with a price.
    pub fn is_submittable(&self) -> bool {
        self.id_terminal.is_some()
            && self.terminal_type.is_some()
            && self.commodities.iter().any(|c| c.id_commodity.is_some())
    }
}

/// Lightweight reference to a UEX terminal (used in dropdowns / candidate lists).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalRef {
    pub id: u32,
    pub name: String,
    pub nickname: String,
    /// Star system the terminal is in (e.g. "Stanton", "Pyro").
    #[serde(default)]
    pub system: String,
    /// Operator/shop prefix (e.g. "Admin", "Platinum Bay"), if any.
    #[serde(default)]
    pub operator: Option<String>,
}
