//! Mapping between the in-game stock/inventory labels and UEX's numeric
//! inventory status scale (1..=7).
//!
//! Confirmed from the reference application's logs:
//! `out of stock -> 1`, `low inventory -> 3`, `medium inventory -> 4`,
//! `high inventory -> 5`, `max inventory -> 7`. The intermediate values 2 and 6
//! are UEX's "very low" / "very high" tiers.

/// Canonical (label, status) table, ordered low -> high.
pub const STOCK_LEVELS: &[(&str, u8)] = &[
    ("out of stock", 1),
    ("very low inventory", 2),
    ("low inventory", 3),
    ("medium inventory", 4),
    ("high inventory", 5),
    ("very high inventory", 6),
    ("max inventory", 7),
];

/// Normalise an OCR-derived stock phrase to `(canonical_label, status)`.
///
/// The input is lowercased and matched by keyword so noisy OCR like
/// "Max Inventory" or "MAXINVENTORY" still resolves. Returns `None` when no
/// tier keyword is recognised.
pub fn parse_stock_label(raw: &str) -> Option<(&'static str, u8)> {
    let s = raw.to_ascii_lowercase();
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();

    // "out of stock" / "cannot sell" / "sold out" -> out of stock. "ofstock"
    // also catches the common OCR mangle "0UT OF STOCK" (O read as 0).
    if compact.contains("outofstock")
        || compact.contains("ofstock")
        || compact.contains("cannotsell")
        || compact.contains("soldout")
    {
        return Some(STOCK_LEVELS[0]);
    }
    // Order matters: check the two-word qualifiers ("very low"/"very high")
    // before the bare "low"/"high".
    if compact.contains("verylow") {
        return Some(STOCK_LEVELS[1]);
    }
    if compact.contains("veryhigh") {
        return Some(STOCK_LEVELS[5]);
    }
    if compact.contains("low") {
        return Some(STOCK_LEVELS[2]);
    }
    if compact.contains("medium") {
        return Some(STOCK_LEVELS[3]);
    }
    if compact.contains("max") {
        return Some(STOCK_LEVELS[6]);
    }
    if compact.contains("high") {
        return Some(STOCK_LEVELS[4]);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_confirmed_levels() {
        assert_eq!(parse_stock_label("out of stock"), Some(("out of stock", 1)));
        assert_eq!(parse_stock_label("Low Inventory"), Some(("low inventory", 3)));
        assert_eq!(parse_stock_label("MEDIUM INVENTORY"), Some(("medium inventory", 4)));
        assert_eq!(parse_stock_label("High Inventory"), Some(("high inventory", 5)));
        assert_eq!(parse_stock_label("Max Inventory"), Some(("max inventory", 7)));
    }

    #[test]
    fn very_qualifiers_take_precedence() {
        assert_eq!(parse_stock_label("Very Low Inventory"), Some(("very low inventory", 2)));
        assert_eq!(parse_stock_label("Very High Inventory"), Some(("very high inventory", 6)));
    }

    #[test]
    fn handles_noise_and_unknown() {
        assert_eq!(parse_stock_label("maxinventory"), Some(("max inventory", 7)));
        assert_eq!(parse_stock_label("cannot sell"), Some(("out of stock", 1)));
        assert_eq!(parse_stock_label("garbage"), None);
        assert_eq!(parse_stock_label(""), None);
    }
}
