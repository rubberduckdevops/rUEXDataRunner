//! Turn OCR words into a structured [`Extraction`]: which terminal, buy or sell,
//! and the commodity rows (name/id, quantity, stock status, price) with
//! confidences.
//!
//! The rules here are reverse-engineered from the reference application's logs:
//! * The terminal/location name is the highest-scoring fuzzy match among short
//!   word windows.
//! * A visible "SHOP QUANTITY" label (or an explicit "Buy" tab) means the player
//!   is *buying* from the shop; otherwise it's a *sell* terminal.
//! * Each commodity is anchored by a fuzzy name match; the following few words
//!   are scanned for its stock label, quantity (SCU) and unit price.

use crate::model::{Commodity, Extraction, TerminalType};
use crate::ocr::Word;
use crate::reference::Reference;
use crate::{matching, pricing, status};

/// Resolve a stock/inventory level from a card blob. First tries the exact
/// keyword table ([`status::parse_stock_label`]); if that fails but the blob
/// mentions "inventory", fuzzy-matches the word(s) just before it against the
/// level names, recovering common OCR manglings like "1AX INVENTORY" -> max.
fn resolve_stock(blob: &str) -> Option<(String, u8)> {
    if let Some((label, st)) = status::parse_stock_label(blob) {
        return Some((label.to_string(), st));
    }
    let lower = blob.to_ascii_lowercase();
    let idx = lower.find("inventory")?;
    let before = &blob[..idx];
    let words: Vec<&str> = before.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    // Levels excluding "out of stock" (handled by the keyword table above).
    let levels = [
        ("max inventory", 7u8, "max"),
        ("very high inventory", 6, "very high"),
        ("high inventory", 5, "high"),
        ("medium inventory", 4, "medium"),
        ("low inventory", 3, "low"),
        ("very low inventory", 2, "very low"),
    ];
    let mut best: Option<(&str, u8, u8)> = None;
    for take in [1usize, 2] {
        if words.len() < take {
            continue;
        }
        let phrase = words[words.len() - take..].join(" ");
        for (label, st, key) in levels {
            let s = matching::score(&phrase, key);
            if best.map(|(_, _, bs)| s > bs).unwrap_or(true) {
                best = Some((label, st, s));
            }
        }
    }
    match best {
        Some((label, st, s)) if s >= 62 => Some((label.to_string(), st)),
        _ => None,
    }
}
use regex::Regex;
use std::sync::LazyLock;

static AMMO_SIZE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bi?size\s+([1-7])\b").unwrap());

/// Ship ammunition is listed as "Size 1..7" top-to-bottom, but the size digit
/// OCRs unreliably (often as "ISIZE" with the number lost). Resolve the size
/// from the legible digit when possible, otherwise by sequential position within
/// a consecutive run, so the three sizes become distinct rows (ids 201..207)
/// instead of collapsing onto the generic "Ship Ammunition" (128).
fn resolve_ship_ammo(
    base_id: u32,
    base_name: &str,
    blob: &str,
    last_size: &mut Option<u32>,
) -> (u32, String) {
    if !base_name.starts_with("Ship Ammunition") {
        *last_size = None;
        return (base_id, base_name.to_string());
    }
    let from_name = if base_name.contains("Size ") {
        base_name.rsplit("Size ").next().and_then(|s| s.trim().parse::<u32>().ok())
    } else {
        None
    };
    let explicit = AMMO_SIZE_RE.captures(blob).and_then(|c| c[1].parse::<u32>().ok());
    let size = from_name
        .or(explicit)
        .or_else(|| last_size.map(|s| s + 1))
        .unwrap_or(1)
        .clamp(1, 7);
    *last_size = Some(size);
    (200 + size, format!("Ship Ammunition - Size {size}"))
}

/// Minimum fuzzy score to accept a commodity name anchor.
const NAME_THRESHOLD: u8 = 80;
/// Minimum fuzzy score to accept a terminal match.
const TERMINAL_THRESHOLD: u8 = 55;

fn upper(s: &str) -> String {
    s.to_ascii_uppercase()
}

/// Trim a card's text at the "AVAILABLE CARGO SIZE" / "CARGO SIZE" row so the
/// container-size boxes below it aren't parsed as quantities or prices.
fn truncate_before_cargo_sizes(blob: &str) -> String {
    let lower = blob.to_ascii_lowercase();
    let cut = lower
        .find("available cargo")
        .or_else(|| lower.find("cargo size"))
        .or_else(|| lower.find("cargo siz"));
    match cut {
        Some(idx) => blob[..idx].trim_end().to_string(),
        None => blob.to_string(),
    }
}

fn compact_upper(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).flat_map(|c| c.to_uppercase()).collect()
}

/// Detect buy vs sell from the recognised words.
///
/// A commodity terminal shows two tabs: **Buy** and **Local Market Value** (the
/// *sell* view). Both tab *labels* are always on screen, so the visible
/// "BUY"/"LOCAL MARKET VALUE" text can't tell us which is active. The reliable
/// discriminator is the per-commodity label: the Buy tab labels each quantity as
/// "SHOP QUANTITY", while the sell view shows a bare "N SCU". So "SHOP QUANTITY"
/// present => buy; otherwise sell.
fn detect_type(words: &[Word]) -> (TerminalType, usize) {
    let up: Vec<String> = words.iter().map(|w| upper(&w.text)).collect();
    let sq = count_shop_quantity(&up);
    if sq > 0 {
        (TerminalType::Buy, sq)
    } else {
        (TerminalType::Sell, 0)
    }
}

fn count_shop_quantity(up: &[String]) -> usize {
    let mut n = 0;
    for i in 0..up.len() {
        // Single mashed token, or the "SHOP" + "QUANTITY" pair.
        if compact_upper(&up[i]) == "SHOPQUANTITY" {
            n += 1;
        } else if up[i].contains("SHOP")
            && up.get(i + 1).map(|w| w.contains("QUANTITY")).unwrap_or(false)
        {
            n += 1;
        }
    }
    n
}

/// Whether a word is part of a "SHOP QUANTITY" label (to drop before parsing).
fn is_shop_quantity_word(up: &[String], i: usize) -> bool {
    let w = &up[i];
    if compact_upper(w) == "SHOPQUANTITY" {
        return true;
    }
    if w.contains("SHOP") && up.get(i + 1).map(|n| n.contains("QUANTITY")).unwrap_or(false) {
        return true;
    }
    if w.contains("QUANTITY") && i > 0 && up[i - 1].contains("SHOP") {
        return true;
    }
    false
}

/// Find the best terminal match among short, same-line word windows.
fn detect_terminal(reference: &Reference, words: &[Word]) -> Option<crate::reference::TerminalMatch> {
    let mut best: Option<crate::reference::TerminalMatch> = None;
    let n = words.len();
    for i in 0..n {
        let mut phrase = String::new();
        for len in 0..4 {
            let j = i + len;
            if j >= n || words[j].line != words[i].line {
                break;
            }
            if !phrase.is_empty() {
                phrase.push(' ');
            }
            phrase.push_str(&words[j].text);
            if phrase.trim().len() < 3 {
                continue;
            }
            if let Some(m) = reference.best_terminal(phrase.trim()) {
                let better = match &best {
                    Some(b) => m.score > b.score,
                    None => true,
                };
                if better {
                    best = Some(m);
                }
            }
        }
    }
    best.filter(|m| m.score >= TERMINAL_THRESHOLD)
}

/// Extract a full result from a screenshot's OCR words.
pub fn extract(
    reference: &Reference,
    words: &[Word],
    screenshot: impl Into<String>,
    captured_at: Option<i64>,
) -> Extraction {
    let mut ex = Extraction::new(screenshot);
    ex.captured_at = captured_at;

    // Terminal.
    if let Some(t) = detect_terminal(reference, words) {
        ex.terminal_name = Some(t.name.clone());
        ex.id_terminal = Some(t.id);
        ex.terminal_confidence = t.score;
        ex.terminal_candidates = t.candidates;
    }

    // Buy/sell type.
    let (ttype, _sq) = detect_type(words);
    ex.terminal_type = Some(ttype);

    // Drop "SHOP QUANTITY" label words before scanning commodities.
    let up: Vec<String> = words.iter().map(|w| upper(&w.text)).collect();
    let filtered: Vec<&Word> = words
        .iter()
        .enumerate()
        .filter(|(i, _)| !is_shop_quantity_word(&up, *i))
        .map(|(_, w)| w)
        .collect();

    ex.commodities = extract_commodities(reference, &filtered);
    ex
}

/// A recognised commodity name and where it sits on the screen.
struct Anchor {
    m: crate::reference::CommodityMatch,
    /// Word indices making up the name (to exclude from the stats scan).
    name_idx: std::ops::Range<usize>,
    /// Left edge of the name (panel-relative x baseline).
    x: i32,
    /// Vertical centre of the name row.
    y: i32,
    /// Name row height (used to size the scan band).
    h: i32,
}

/// Find commodity name anchors by fuzzy-matching 1..=3 word, same-line phrases.
fn find_anchors(reference: &Reference, words: &[&Word]) -> Vec<Anchor> {
    let n = words.len();
    let mut anchors = Vec::new();
    let mut i = 0usize;
    while i < n {
        let mut best: Option<(usize, crate::reference::CommodityMatch)> = None;
        let mut phrase = String::new();
        // A name may wrap onto a second line (e.g. "SHIP AMMUNITION -" / "SIZE 2"),
        // so scan up to 5 words spanning at most two lines and keep the best match.
        let mut transitions = 0;
        for len in 1..=5 {
            let j = i + len - 1;
            if j >= n {
                break;
            }
            if j > i && words[j].line != words[j - 1].line {
                transitions += 1;
                if transitions > 1 {
                    break;
                }
            }
            if !phrase.is_empty() {
                phrase.push(' ');
            }
            phrase.push_str(&words[j].text);
            let p = phrase.trim();
            if p.len() < 3 {
                continue;
            }
            if let Some(m) = reference.best_commodity(p) {
                // Prefer a strictly better score; ties keep the shorter name.
                if best.as_ref().map(|(_, bm)| m.score > bm.score).unwrap_or(true) {
                    best = Some((len, m));
                }
            }
        }

        match best {
            Some((name_len, m)) if m.score >= NAME_THRESHOLD => {
                let name_words = &words[i..i + name_len];
                let x = name_words.iter().map(|w| w.left).min().unwrap_or(0);
                anchors.push(Anchor {
                    m,
                    name_idx: i..i + name_len,
                    x,
                    y: words[i].center_y(),
                    h: words[i].height.max(8),
                });
                i += name_len; // don't re-anchor inside this name
            }
            _ => i += 1,
        }
    }
    anchors
}

/// Assign each anchor its stock/quantity/price by collecting the words that fall
/// in a vertical band below the name and within the same panel column. This is
/// robust to the interleaved 3D-scene text that reading-order windows trip over.
fn extract_commodities(reference: &Reference, words: &[&Word]) -> Vec<Commodity> {
    let mut anchors = find_anchors(reference, words);
    // Order top-to-bottom so each band can stop at the next commodity.
    anchors.sort_by_key(|a| (a.y, a.x));

    let mut result: Vec<Commodity> = Vec::new();
    let mut last_ammo_size: Option<u32> = None;
    for (idx, a) in anchors.iter().enumerate() {
        let band_top = a.y - a.h;
        let next_y = anchors.get(idx + 1).map(|nx| nx.y - 1);
        let band_bottom = next_y
            .unwrap_or(i32::MAX)
            .min(a.y + a.h * 6); // a commodity card spans a few rows
        let x_floor = a.x - a.h * 3; // ignore the 3D scene to the left of the panel

        // Collect the card's words (excluding this anchor's own name words),
        // sorted into reading order within the card.
        let mut card: Vec<&&Word> = words
            .iter()
            .enumerate()
            .filter(|(wi, w)| {
                !a.name_idx.contains(wi)
                    && w.center_y() >= band_top
                    && w.center_y() <= band_bottom
                    && w.left >= x_floor
            })
            .map(|(_, w)| w)
            .collect();
        // Order by visual ROW then left-to-right. Bucketing y avoids a 1px
        // difference flipping a number and its unit (e.g. "793 SCU" -> "SCU 793",
        // which would hide the quantity).
        let row_h = (a.h as f32 * 0.7).max(6.0);
        card.sort_by_key(|w| ((w.center_y() as f32 / row_h).round() as i32, w.left));
        let full_blob: String =
            card.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
        // The "AVAILABLE CARGO SIZE (SCU)" row lists container sizes (1 2 4 8 …)
        // *below* the shop quantity and price; cut it off so those numbers can't
        // be mistaken for the quantity.
        let blob = truncate_before_cargo_sizes(&full_blob);

        if std::env::var("RUEX_DEBUG").is_ok() {
            eprintln!("[card] name={:?} blob={:?}", a.m.name, blob);
        }

        let (stock_label, stock_status) = match resolve_stock(&blob) {
            Some((label, st)) => (Some(label), Some(st)),
            None => (None, None),
        };
        let quantity = pricing::find_quantity(&blob);
        let price = pricing::find_price(&blob).map(|p| p.value);

        // Require some supporting stat so a stray name match is rejected.
        if stock_status.is_none() && quantity.is_none() && price.is_none() {
            continue;
        }

        // Resolve ship-ammunition sizes (see helper); no-op for other commodities.
        let (res_id, res_name) = resolve_ship_ammo(a.m.id, &a.m.name, &full_blob, &mut last_ammo_size);

        let digit_conf = card
            .iter()
            .find(|w| w.text.chars().any(|c| c.is_ascii_digit()))
            .map(|w| w.conf)
            .unwrap_or(50);

        // Merge consecutive duplicates of the same commodity id.
        if let Some(last) = result.last_mut() {
            if last.id_commodity == Some(res_id) {
                if price.is_some() && last.price.is_none() {
                    last.price = price;
                    last.price_confidence = digit_conf.max(1);
                }
                if quantity.is_some() && last.quantity_scu.is_none() {
                    last.quantity_scu = quantity;
                    last.quantity_confidence = digit_conf.max(1);
                }
                if last.status.is_none() {
                    last.status = stock_status;
                    last.stock_label = stock_label;
                }
                continue;
            }
        }

        let mut c = Commodity::new(res_name);
        c.id_commodity = Some(res_id);
        c.name_confidence = a.m.score;
        c.quantity_scu = quantity;
        c.quantity_confidence = if quantity.is_some() { digit_conf.max(1) } else { 0 };
        c.stock_label = stock_label;
        c.status = stock_status;
        c.price = price;
        c.price_confidence = if price.is_some() { digit_conf.max(1) } else { 0 };
        result.push(c);
    }

    result
}

/// Merge two extractions of the same screenshot (e.g. a raw pass and a
/// preprocessed pass). The terminal is taken from whichever pass identified it
/// with higher confidence; commodity rows are unioned by id, filling each
/// field from whichever pass captured it with higher confidence.
pub fn merge(mut a: Extraction, b: Extraction) -> Extraction {
    // Terminal: prefer the more confident identification.
    let take_b_terminal =
        b.id_terminal.is_some() && (a.id_terminal.is_none() || b.terminal_confidence > a.terminal_confidence);
    if take_b_terminal {
        a.terminal_name = b.terminal_name.clone();
        a.id_terminal = b.id_terminal;
        a.terminal_confidence = b.terminal_confidence;
        a.terminal_candidates = b.terminal_candidates.clone();
    }
    // Type: keep a's if present, else b's.
    if a.terminal_type.is_none() {
        a.terminal_type = b.terminal_type;
    }

    // Commodities: merge the two screen-ordered lists into one, deduping by id.
    // Both `a` (panel pass) and `b` (raw pass) are already ordered top-to-bottom,
    // so we interleave them — syncing on shared ids — to keep the final list in
    // screen order regardless of which pass first found each row.
    let a_ids: std::collections::HashSet<u32> =
        a.commodities.iter().filter_map(|c| c.id_commodity).collect();
    let b_ids: std::collections::HashSet<u32> =
        b.commodities.iter().filter_map(|c| c.id_commodity).collect();

    let a_list = std::mem::take(&mut a.commodities);
    let b_list = b.commodities;
    let mut merged: Vec<Commodity> = Vec::with_capacity(a_list.len() + b_list.len());
    let (mut ai, mut bi) = (0usize, 0usize);
    while ai < a_list.len() || bi < b_list.len() {
        let a_id = a_list.get(ai).and_then(|c| c.id_commodity);
        let b_id = b_list.get(bi).and_then(|c| c.id_commodity);
        match (a_id, b_id) {
            (Some(x), Some(y)) if x == y => {
                let mut m = a_list[ai].clone();
                merge_commodity(&mut m, &b_list[bi]);
                merged.push(m);
                ai += 1;
                bi += 1;
            }
            // b's head is unique to b -> emit it here (screen order).
            (_, Some(y)) if !a_ids.contains(&y) => {
                merged.push(b_list[bi].clone());
                bi += 1;
            }
            // a's head is unique to a -> emit it.
            (Some(x), _) if !b_ids.contains(&x) => {
                merged.push(a_list[ai].clone());
                ai += 1;
            }
            // Both heads are shared but differ (orders diverge): trust a's order,
            // enriching from a's match in b.
            (Some(x), Some(_)) => {
                let mut m = a_list[ai].clone();
                if let Some(bm) = b_list.iter().find(|c| c.id_commodity == Some(x)) {
                    merge_commodity(&mut m, bm);
                }
                merged.push(m);
                ai += 1;
            }
            // Leftovers (one list exhausted, or id-less rows).
            (Some(_), None) => {
                merged.push(a_list[ai].clone());
                ai += 1;
            }
            (None, Some(_)) => {
                merged.push(b_list[bi].clone());
                bi += 1;
            }
            (None, None) => break,
        }
    }
    // Safety net: drop any duplicate id that slipped through (keeps first/enriched).
    let mut seen = std::collections::HashSet::new();
    merged.retain(|c| match c.id_commodity {
        Some(id) => seen.insert(id),
        None => true,
    });
    // If sized ship ammunition (201..=207) is present, the generic "Ship
    // Ammunition" (128) is a less-specific duplicate from a noisier pass — drop it.
    if merged.iter().any(|c| matches!(c.id_commodity, Some(201..=207))) {
        merged.retain(|c| c.id_commodity != Some(128));
    }
    a.commodities = merged;
    a
}

/// Fill `base` with any better data from `other` (same commodity id).
fn merge_commodity(base: &mut Commodity, other: &Commodity) {
    if other.name_confidence > base.name_confidence {
        base.name = other.name.clone();
        base.name_confidence = other.name_confidence;
    }
    // Price: take it if base has none, or if other's is more confident.
    if other.price.is_some() && (base.price.is_none() || other.price_confidence > base.price_confidence) {
        base.price = other.price;
        base.price_confidence = other.price_confidence;
    }
    // Quantity: same rule.
    if other.quantity_scu.is_some()
        && (base.quantity_scu.is_none() || other.quantity_confidence > base.quantity_confidence)
    {
        base.quantity_scu = other.quantity_scu;
        base.quantity_confidence = other.quantity_confidence;
    }
    // Status: fill if missing.
    if base.status.is_none() && other.status.is_some() {
        base.status = other.status;
        base.stock_label = other.stock_label.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a word with layout: `line` id, top-left `(x, y)`, confidence.
    fn w(text: &str, line: (i32, i32, i32), x: i32, y: i32, conf: u8) -> Word {
        Word {
            text: text.to_string(),
            conf,
            left: x,
            top: y,
            width: text.len() as i32 * 8,
            height: 14,
            line,
            word_num: 0,
        }
    }

    /// Build a synthetic word stream resembling a "buy" terminal at Bueno Ravine,
    /// with realistic screen coordinates (the panel is on the right; each
    /// commodity card occupies a vertical band).
    fn synthetic_buy() -> Vec<Word> {
        vec![
            w("BUENO", (1, 1, 1), 100, 10, 92),
            w("RAVINE", (1, 1, 1), 180, 10, 91),
            w("Buy", (1, 1, 2), 100, 40, 93),
            w("Local", (1, 1, 2), 140, 40, 92),
            w("Market", (1, 1, 2), 200, 40, 92),
            // Diamond card (y ~100-150)
            w("Diamond", (1, 1, 3), 100, 100, 88),
            w("SHOP", (1, 1, 4), 300, 100, 70),
            w("QUANTITY", (1, 1, 4), 340, 100, 70),
            w("Max", (1, 1, 5), 100, 120, 90),
            w("Inventory", (1, 1, 5), 140, 120, 90),
            w("525", (1, 1, 5), 250, 120, 94),
            w("SCU", (1, 1, 5), 290, 120, 94),
            w("¤6.561K/SCU", (1, 1, 6), 100, 140, 40),
            // Stileron card (y ~200-250)
            w("Stileron", (1, 1, 7), 100, 200, 100),
            w("SHOP", (1, 1, 8), 300, 200, 70),
            w("QUANTITY", (1, 1, 8), 340, 200, 70),
            w("High", (1, 1, 9), 100, 220, 90),
            w("Inventory", (1, 1, 9), 140, 220, 90),
            w("6", (1, 1, 9), 250, 220, 94),
            w("SCU", (1, 1, 9), 290, 220, 94),
            w("¤122.85K/SCU", (1, 1, 10), 100, 240, 75),
        ]
    }

    #[test]
    fn extracts_buy_terminal_and_commodities() {
        let r = Reference::embedded();
        let words = synthetic_buy();
        let ex = extract(&r, &words, "test.jpg", Some(1_700_000_000));

        assert_eq!(ex.terminal_name.as_deref(), Some("Bueno Ravine"));
        assert_eq!(ex.id_terminal, Some(551));
        assert_eq!(ex.terminal_type, Some(TerminalType::Buy));
        assert_eq!(ex.commodities.len(), 2);

        let diamond = &ex.commodities[0];
        assert_eq!(diamond.name, "Diamond");
        assert_eq!(diamond.id_commodity, Some(25));
        assert_eq!(diamond.status, Some(7)); // max inventory
        assert_eq!(diamond.quantity_scu, Some(525));
        assert_eq!(diamond.price, Some(6561));

        let stileron = &ex.commodities[1];
        assert_eq!(stileron.id_commodity, Some(122));
        assert_eq!(stileron.status, Some(5)); // high inventory
        assert_eq!(stileron.quantity_scu, Some(6));
        assert_eq!(stileron.price, Some(122850));
    }

    #[test]
    fn merge_takes_terminal_from_confident_pass_and_best_fields() {
        // Pass A: identified the terminal, but weak commodity data.
        let mut a = Extraction::new("s.jpg");
        a.id_terminal = Some(551);
        a.terminal_name = Some("Bueno Ravine".into());
        a.terminal_confidence = 100;
        a.terminal_type = Some(TerminalType::Buy);
        let mut ca = Commodity::new("Diamond");
        ca.id_commodity = Some(25);
        ca.status = Some(7);
        a.commodities.push(ca);

        // Pass B: missed the terminal, but read price/qty well.
        let mut b = Extraction::new("s.jpg");
        b.terminal_type = Some(TerminalType::Buy);
        let mut cb = Commodity::new("Diamond");
        cb.id_commodity = Some(25);
        cb.price = Some(6561);
        cb.price_confidence = 93;
        cb.quantity_scu = Some(525);
        cb.quantity_confidence = 94;
        b.commodities.push(cb);
        // A commodity only pass B saw.
        let mut cb2 = Commodity::new("Stileron");
        cb2.id_commodity = Some(122);
        cb2.price = Some(122850);
        cb2.price_confidence = 90;
        b.commodities.push(cb2);

        let merged = merge(a, b);
        assert_eq!(merged.id_terminal, Some(551)); // kept from A
        assert_eq!(merged.terminal_confidence, 100);
        assert_eq!(merged.commodities.len(), 2);
        let diamond = &merged.commodities[0];
        assert_eq!(diamond.status, Some(7)); // from A
        assert_eq!(diamond.price, Some(6561)); // filled from B
        assert_eq!(diamond.quantity_scu, Some(525)); // filled from B
        assert_eq!(merged.commodities[1].id_commodity, Some(122)); // added from B
    }

    #[test]
    fn resolve_stock_recovers_mangled_labels() {
        // "MAX" mis-OCR'd as "1AX".
        assert_eq!(resolve_stock("2,100 SCU 1ax Inventory ¤2,990/SCU"), Some(("max inventory".into(), 7)));
        // "OUT" mis-OCR'd as "0UT".
        assert_eq!(resolve_stock("0 SCU 0UT OF STOCK ¤211,000/SCU"), Some(("out of stock".into(), 1)));
        // Level word fully gone -> unrecoverable.
        assert_eq!(resolve_stock("52 SCU Inventory ¤28,928/SCU"), None);
    }

    #[test]
    fn quantity_captured_despite_scu_word_row_ordering() {
        // The SCU unit word sits 1px higher than its number; a naive (y, x) sort
        // would emit "SCU 793" and hide the quantity. Row-bucketing keeps "793 SCU".
        let r = Reference::embedded();
        let words = vec![
            w("BUENO", (1, 1, 1), 100, 10, 92),
            w("RAVINE", (1, 1, 1), 180, 10, 91),
            w("Sell", (1, 1, 2), 100, 40, 93),
            w("Local", (1, 1, 2), 140, 40, 92),
            w("Market", (1, 1, 2), 200, 40, 92),
            w("Iron", (1, 1, 3), 100, 100, 100),
            w("SCU", (1, 1, 3), 290, 99, 94),
            w("793", (1, 1, 3), 250, 100, 94),
        ];
        let ex = extract(&r, &words, "s.jpg", None);
        assert_eq!(ex.commodities[0].id_commodity, Some(44));
        assert_eq!(ex.commodities[0].quantity_scu, Some(793));
    }

    #[test]
    fn cargo_size_boxes_are_not_read_as_quantity() {
        // Shop quantity is above "AVAILABLE CARGO SIZE (SCU)"; the boxes below it
        // (1 2 4 8 16) must be ignored.
        let blob = "Max Inventory 2,162 SCU ¤1,376/SCU AVAILABLE CARGO SIZE (SCU) 1 2 4 8 16";
        let cut = truncate_before_cargo_sizes(blob);
        assert_eq!(pricing::find_quantity(&cut), Some(2162));
        assert_eq!(pricing::find_price(&cut).map(|p| p.value), Some(1376));
    }

    #[test]
    fn ship_ammo_sizes_resolve_distinctly() {
        let mut last = None;
        // Legible digit.
        let (id1, n1) = resolve_ship_ammo(128, "Ship Ammunition", "SIZE 1 12,000 SCU", &mut last);
        assert_eq!((id1, n1.as_str()), (201, "Ship Ammunition - Size 1"));
        // Digit lost ("ISIZE") -> sequential position.
        let (id2, _) = resolve_ship_ammo(128, "Ship Ammunition", "ISIZE 12,000 SCU", &mut last);
        assert_eq!(id2, 202);
        // Explicit "3" honored even after a gap.
        let (id3, _) = resolve_ship_ammo(128, "Ship Ammunition", "SIZE 3 12,000 SCU", &mut last);
        assert_eq!(id3, 203);
        // A non-ammo commodity resets the run.
        let (idx, nx) = resolve_ship_ammo(44, "Iron", "0 SCU", &mut last);
        assert_eq!((idx, nx.as_str()), (44, "Iron"));
        assert_eq!(last, None);
    }

    #[test]
    fn merge_produces_screen_ordered_union() {
        // Panel pass (a) found Iron/Steel/Titanium; raw pass (b) additionally
        // caught Scrap at the TOP. The merged order must be screen order.
        let mk = |id: u32, name: &str| {
            let mut c = Commodity::new(name);
            c.id_commodity = Some(id);
            c.status = Some(3);
            c
        };
        let mut a = Extraction::new("s.jpg");
        a.commodities = vec![mk(44, "Iron"), mk(109, "Steel"), mk(75, "Titanium")];
        let mut b = Extraction::new("s.jpg");
        b.commodities = vec![mk(67, "Scrap"), mk(44, "Iron"), mk(109, "Steel"), mk(75, "Titanium")];

        let merged = merge(a, b);
        let ids: Vec<u32> = merged.commodities.iter().filter_map(|c| c.id_commodity).collect();
        assert_eq!(ids, vec![67, 44, 109, 75]); // Scrap first, then Iron, Steel, Titanium
    }

    #[test]
    fn detects_sell_when_no_shop_quantity() {
        let r = Reference::embedded();
        let words = vec![
            w("BUENO", (1, 1, 1), 100, 10, 92),
            w("RAVINE", (1, 1, 1), 180, 10, 91),
            w("Sell", (1, 1, 2), 100, 40, 93),
            w("Local", (1, 1, 2), 140, 40, 92),
            w("Market", (1, 1, 2), 200, 40, 92),
            w("Iron", (1, 1, 3), 100, 100, 100),
            w("Out", (1, 1, 4), 100, 120, 90),
            w("of", (1, 1, 4), 140, 120, 90),
            w("stock", (1, 1, 4), 170, 120, 90),
            w("0", (1, 1, 4), 250, 120, 94),
            w("SCU", (1, 1, 4), 290, 120, 94),
        ];
        let ex = extract(&r, &words, "s.jpg", None);
        assert_eq!(ex.terminal_type, Some(TerminalType::Sell));
        assert_eq!(ex.commodities.len(), 1);
        assert_eq!(ex.commodities[0].id_commodity, Some(44));
        assert_eq!(ex.commodities[0].status, Some(1)); // out of stock
    }
}
