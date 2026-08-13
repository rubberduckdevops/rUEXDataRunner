//! UEX reference data: the canonical lists of commodities and commodity
//! terminals, with fuzzy lookup by (noisy) name.
//!
//! The lists are embedded at compile time from `assets/reference/*.json`
//! (snapshots of the UEX `commodities` and `terminals` endpoints) so the app
//! and its tests work fully offline. [`Reference::refresh_from_api`] can update
//! them at runtime.

use crate::matching;
use crate::model::TerminalRef;
use serde::Deserialize;

const COMMODITIES_JSON: &str = include_str!("../../../assets/reference/commodities.json");
const TERMINALS_JSON: &str = include_str!("../../../assets/reference/terminals.json");

#[derive(Debug, Deserialize)]
struct ApiEnvelope<T> {
    data: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct RawCommodity {
    id: u32,
    name: String,
    #[serde(default = "one")]
    is_available: i32,
    #[serde(default = "one")]
    is_visible: i32,
}

#[derive(Debug, Deserialize)]
struct RawTerminal {
    id: u32,
    name: String,
    #[serde(default)]
    nickname: String,
    #[serde(default)]
    displayname: String,
    #[serde(default)]
    r#type: String,
    #[serde(default = "one")]
    is_available: i32,
    #[serde(default)]
    star_system_name: String,
}

fn one() -> i32 {
    1
}

/// A commodity in the reference list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommodityRef {
    pub id: u32,
    pub name: String,
}

/// A terminal in the reference list. `name` is the display name shown in game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalEntry {
    pub id: u32,
    /// Display name (what the terminal shows in game, e.g. "Bueno Ravine").
    pub name: String,
    pub nickname: String,
    /// Star system the terminal is physically in (e.g. "Stanton", "Pyro").
    pub system: String,
    /// Operator/shop prefix from the full name (e.g. "Admin", "Platinum Bay"),
    /// which distinguishes co-located shops with the same display name.
    pub operator: Option<String>,
}

impl From<&TerminalEntry> for TerminalRef {
    fn from(t: &TerminalEntry) -> Self {
        TerminalRef {
            id: t.id,
            name: t.name.clone(),
            nickname: t.nickname.clone(),
            system: t.system.clone(),
            operator: t.operator.clone(),
        }
    }
}

/// Result of a commodity lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommodityMatch {
    pub id: u32,
    pub name: String,
    pub score: u8,
}

/// Result of a terminal lookup, including co-located candidates for the
/// disambiguation dropdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalMatch {
    pub id: u32,
    pub name: String,
    pub score: u8,
    pub candidates: Vec<TerminalRef>,
}

pub struct Reference {
    commodities: Vec<CommodityRef>,
    terminals: Vec<TerminalEntry>,
}

impl Reference {
    /// Load the embedded reference snapshots. Panics only if the bundled JSON is
    /// malformed, which is a build-time invariant covered by tests.
    pub fn embedded() -> Self {
        let commodities = parse_commodities(COMMODITIES_JSON).expect("bundled commodities.json");
        let terminals = parse_terminals(TERMINALS_JSON).expect("bundled terminals.json");
        Reference { commodities, terminals }
    }

    /// Build from explicit JSON strings (used by tests / API refresh).
    pub fn from_json(commodities_json: &str, terminals_json: &str) -> anyhow::Result<Self> {
        Ok(Reference {
            commodities: parse_commodities(commodities_json)?,
            terminals: parse_terminals(terminals_json)?,
        })
    }

    pub fn commodities(&self) -> &[CommodityRef] {
        &self.commodities
    }
    pub fn terminals(&self) -> &[TerminalEntry] {
        &self.terminals
    }

    /// Best commodity match for a (possibly noisy) name.
    pub fn best_commodity(&self, query: &str) -> Option<CommodityMatch> {
        let (idx, score) = matching::best(query, self.commodities.iter().map(|c| c.name.as_str()))?;
        let c = &self.commodities[idx];
        Some(CommodityMatch { id: c.id, name: c.name.clone(), score })
    }

    /// Best terminal match for a location name, plus other terminals whose
    /// display name matches closely (co-located kiosks needing disambiguation).
    pub fn best_terminal(&self, query: &str) -> Option<TerminalMatch> {
        // Score every terminal against name and nickname; keep the max.
        let mut scored: Vec<(usize, u8)> = self
            .terminals
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let s = matching::score(query, &t.name).max(matching::score(query, &t.nickname));
                (i, s)
            })
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        let (best_idx, best_score) = *scored.first()?;
        let best = &self.terminals[best_idx];

        // Candidates: any terminal within 8 points of the best score, so the UI
        // can offer alternatives when several kiosks read similarly.
        let candidates: Vec<TerminalRef> = scored
            .iter()
            .filter(|(_, s)| best_score.saturating_sub(*s) <= 8 && *s >= 50)
            .map(|(i, _)| (&self.terminals[*i]).into())
            .collect();

        Some(TerminalMatch {
            id: best.id,
            name: best.name.clone(),
            score: best_score,
            candidates,
        })
    }

    /// Look up a terminal by exact UEX id.
    pub fn terminal_by_id(&self, id: u32) -> Option<&TerminalEntry> {
        self.terminals.iter().find(|t| t.id == id)
    }

    /// Replace the reference lists from freshly fetched API JSON.
    pub fn refresh_from_json(&mut self, commodities_json: &str, terminals_json: &str) -> anyhow::Result<()> {
        self.commodities = parse_commodities(commodities_json)?;
        self.terminals = parse_terminals(terminals_json)?;
        Ok(())
    }
}

fn parse_commodities(json: &str) -> anyhow::Result<Vec<CommodityRef>> {
    let env: ApiEnvelope<RawCommodity> = serde_json::from_str(json)?;
    Ok(env
        .data
        .into_iter()
        .filter(|c| c.is_available != 0 && c.is_visible != 0)
        .map(|c| CommodityRef { id: c.id, name: c.name })
        .collect())
}

fn parse_terminals(json: &str) -> anyhow::Result<Vec<TerminalEntry>> {
    let env: ApiEnvelope<RawTerminal> = serde_json::from_str(json)?;
    Ok(env
        .data
        .into_iter()
        .filter(|t| t.r#type == "commodity" && t.is_available != 0)
        .map(|t| {
            let name = if t.displayname.trim().is_empty() {
                t.name.clone()
            } else {
                t.displayname.clone()
            };
            // Operator prefix, e.g. "Admin - Nyx Gateway (Pyro)" -> "Admin".
            let operator = t
                .name
                .split_once(" - ")
                .map(|(op, _)| op.trim().to_string())
                .filter(|op| !op.is_empty());
            TerminalEntry {
                id: t.id,
                name,
                nickname: t.nickname,
                system: t.star_system_name,
                operator,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_reference_loads() {
        let r = Reference::embedded();
        assert!(r.commodities().len() > 100, "expected many commodities");
        assert!(r.terminals().len() > 50, "expected many terminals");
    }

    #[test]
    fn commodity_ids_match_uex() {
        let r = Reference::embedded();
        // IDs verified against the reference app's logs.
        assert_eq!(r.best_commodity("Bexalite").unwrap().id, 13);
        assert_eq!(r.best_commodity("Iron").unwrap().id, 44);
        assert_eq!(r.best_commodity("Cobalt").unwrap().id, 110);
        assert_eq!(r.best_commodity("Stileron").unwrap().id, 122);
    }

    #[test]
    fn commodity_fuzzy_match_survives_ocr_noise() {
        let r = Reference::embedded();
        let m = r.best_commodity("Hephaestanile").unwrap();
        assert_eq!(m.id, 39);
        assert!(m.score >= 85);
    }

    #[test]
    fn terminals_carry_system_and_operator_for_disambiguation() {
        let r = Reference::embedded();
        // Every terminal should know its star system.
        assert!(r.terminals().iter().all(|t| !t.system.is_empty()));
        // "Nyx Gateway" exists in more than one system; candidates should span them.
        let m = r.best_terminal("Nyx Gateway").expect("match");
        let systems: std::collections::HashSet<&str> =
            m.candidates.iter().map(|c| c.system.as_str()).collect();
        assert!(systems.len() >= 2, "expected multiple systems, got {systems:?}");
        assert!(m.candidates.iter().any(|c| c.operator.is_some()));
    }

    #[test]
    fn terminal_matches_by_display_name() {
        let r = Reference::embedded();
        let m = r.best_terminal("Bueno Ravine").unwrap();
        assert_eq!(m.id, 551);
        assert_eq!(m.score, 100);

        let m2 = r.best_terminal("Rod's Fuel 'N Supplies").unwrap();
        assert_eq!(m2.id, 486);
        assert!(m2.score >= 95);
    }
}
