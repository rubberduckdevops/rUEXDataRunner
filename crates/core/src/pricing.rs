//! Parsing of the numeric tokens on a trade terminal: unit price (aUEC/SCU)
//! and available quantity (SCU).
//!
//! In game, prices render with `K`/`M` magnitude suffixes, e.g. `¤6.561K/SCU`
//! (= 6561 aUEC) or `¤1.22M/SCU` (= 1_220_000). Quantities render as plain
//! integers with optional thousands separators, e.g. `1,050 SCU`.
//!
//! OCR frequently mangles the leading `¤` glyph (into `8`, `£`, `|`, `e`, …) and
//! occasionally drops the `/`, so the matchers below anchor on the number plus
//! the `SCU` unit and treat the currency glyph and slash as optional.

use regex::Regex;
use std::sync::LazyLock;

/// A price hit, with the aUEC-per-SCU value and the exact matched text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PriceMatch {
    pub value: u32,
    pub matched: String,
}

// A price is a number (optionally decimal) followed by an optional K/M suffix and
// the `SCU` unit, with a `/` and/or currency glyph indicating "per SCU".
// We require EITHER a magnitude suffix OR a `/` so we don't swallow a bare
// quantity like "525 SCU".
static PRICE_KM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)([0-9]+(?:[.,][0-9]+)?)\s*([km])\s*/?\s*s?cu").unwrap()
});
static PRICE_SLASH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)([0-9]+(?:[.,][0-9]+)?)\s*/\s*s?cu").unwrap()
});

// A quantity is an integer (optional thousands separators) directly before `SCU`,
// NOT preceded by `/` (which would make it a price) and with no K/M suffix.
static QUANTITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^/0-9a-z])([0-9][0-9.,]*)\s*scu").unwrap()
});

fn scale(suffix: char) -> f64 {
    match suffix.to_ascii_lowercase() {
        'k' => 1_000.0,
        'm' => 1_000_000.0,
        _ => 1.0,
    }
}

/// Convert a captured number + magnitude suffix into an integer aUEC value,
/// rounding to the nearest whole aUEC.
fn amount_with_suffix(num: &str, suffix: char) -> Option<u32> {
    // With a magnitude suffix the '.' is a decimal point and ',' is noise.
    let cleaned: String = num.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
    let f: f64 = cleaned.parse().ok()?;
    let scaled = (f * scale(suffix)).round();
    if scaled < 0.0 || scaled > u32::MAX as f64 {
        return None;
    }
    Some(scaled as u32)
}

/// Convert a captured integer (no magnitude suffix) into a plain value.
fn amount_plain(num: &str) -> Option<u32> {
    let cleaned: String = num.chars().filter(|c| c.is_ascii_digit()).collect();
    cleaned.parse().ok()
}

/// Find the first plausible price in a line of OCR text.
pub fn find_price(text: &str) -> Option<PriceMatch> {
    if let Some(c) = PRICE_KM.captures(text) {
        let num = c.get(1)?.as_str();
        let suffix = c.get(2)?.as_str().chars().next()?;
        if let Some(value) = amount_with_suffix(num, suffix) {
            return Some(PriceMatch { value, matched: c.get(0)?.as_str().trim().to_string() });
        }
    }
    if let Some(c) = PRICE_SLASH.captures(text) {
        let num = c.get(1)?.as_str();
        // No magnitude suffix here, so the number is a whole aUEC value with a
        // thousands separator: "6,561/SCU". OCR frequently reads that comma as a
        // period ("6.561"), so BOTH separators are stripped — never treated as a
        // decimal point (SC prices are integers).
        let value = amount_plain(num)?;
        return Some(PriceMatch { value, matched: c.get(0)?.as_str().trim().to_string() });
    }
    None
}

/// Find the first plausible quantity (SCU count) in a line of OCR text,
/// ignoring any substring that is actually a price.
pub fn find_quantity(text: &str) -> Option<u32> {
    // Blank out price substrings first so their numbers can't be misread as qty.
    let mut masked = text.to_string();
    for re in [&*PRICE_KM, &*PRICE_SLASH] {
        masked = re.replace_all(&masked, " ").into_owned();
    }
    let c = QUANTITY.captures(&masked)?;
    amount_plain(c.get(1)?.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_k_prices_from_logs() {
        // Values taken directly from the reference app's logs.
        assert_eq!(find_price("¤6.561K/SCU").unwrap().value, 6561);
        assert_eq!(find_price("¤23.556K/SCU").unwrap().value, 23556);
        assert_eq!(find_price("¤122.85K/SCU").unwrap().value, 122850);
        assert_eq!(find_price("¤36.26K/SCU").unwrap().value, 36260);
        assert_eq!(find_price("¤4.494K/SCU").unwrap().value, 4494);
        assert_eq!(find_price("¤35K/SCU").unwrap().value, 35000);
    }

    #[test]
    fn tolerates_currency_glyphs_spacing_and_dropped_slash() {
        // The ¤ glyph, extra spaces, and a dropped '/' are all handled. (When OCR
        // mangles ¤ into a digit, the leading digit is indistinguishable from a
        // real one — that's corrected in the review UI, not guessed here.)
        assert_eq!(find_price("¤ 6.561 K /SCU").unwrap().value, 6561);
        assert_eq!(find_price("£122.85K SCU").unwrap().value, 122850);
        assert_eq!(find_price("|4.494K/SCU").unwrap().value, 4494);
    }

    #[test]
    fn parses_comma_thousands_and_misread_periods() {
        // The in-game "¤6,561/SCU" form, and the common OCR misread where the
        // comma becomes a period — both are whole aUEC, never decimals.
        assert_eq!(find_price("¤6,561/SCU").unwrap().value, 6561);
        assert_eq!(find_price("¤6.561/SCU").unwrap().value, 6561);
        assert_eq!(find_price("¤122,850/SCU").unwrap().value, 122850);
        assert_eq!(find_price("¤36.260/SCU").unwrap().value, 36260);
        assert_eq!(find_price("¤67,000/SCU").unwrap().value, 67000);
    }

    #[test]
    fn parses_million_prices() {
        assert_eq!(find_price("¤1.22M/SCU").unwrap().value, 1_220_000);
        assert_eq!(find_price("¤2M/SCU").unwrap().value, 2_000_000);
    }

    #[test]
    fn parses_quantities_with_separators() {
        assert_eq!(find_quantity("Max Inventory 1,050 SCU"), Some(1050));
        assert_eq!(find_quantity("525 SCU"), Some(525));
        assert_eq!(find_quantity("6000 SCU"), Some(6000));
        assert_eq!(find_quantity("6 SCU"), Some(6));
    }

    #[test]
    fn quantity_ignores_price_numbers() {
        // A line with only a price yields no quantity.
        assert_eq!(find_quantity("¤6.561K/SCU"), None);
        // A mixed line returns the quantity, not the price's digits.
        assert_eq!(find_quantity("525 SCU ¤6.561K/SCU"), Some(525));
    }

    #[test]
    fn no_false_positives() {
        assert_eq!(find_price("AVAILABLE CARGO SIZE (SCU)"), None);
        assert_eq!(find_price("SHOP QUANTITY"), None);
    }
}
