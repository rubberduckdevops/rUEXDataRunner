//! Fuzzy string matching used to resolve noisy OCR text to canonical commodity
//! and terminal names. Modeled on `thefuzz`/`RapidFuzz` ratios used by the
//! reference app: a plain similarity ratio combined with a token-sorted ratio,
//! reported on a 0-100 scale.

use strsim::normalized_levenshtein;

fn normalize(s: &str) -> String {
    s.trim().to_ascii_uppercase()
}

/// Plain similarity ratio (0.0-1.0) after normalization.
fn ratio(a: &str, b: &str) -> f64 {
    let (a, b) = (normalize(a), normalize(b));
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    normalized_levenshtein(&a, &b)
}

/// Ratio after sorting whitespace-separated tokens, so word order and extra
/// spacing don't penalize the match (e.g. "Dung Ranta" ~ "Ranta Dung").
fn token_sort_ratio(a: &str, b: &str) -> f64 {
    let mut ta: Vec<&str> = a.split_whitespace().collect();
    let mut tb: Vec<&str> = b.split_whitespace().collect();
    ta.sort_unstable();
    tb.sort_unstable();
    ratio(&ta.join(" "), &tb.join(" "))
}

/// Combined 0-100 similarity score between a query and a candidate.
pub fn score(query: &str, candidate: &str) -> u8 {
    let r = ratio(query, candidate).max(token_sort_ratio(query, candidate));
    (r * 100.0).round().clamp(0.0, 100.0) as u8
}

/// Pick the best-scoring candidate from `candidates` for `query`.
///
/// Returns `(index, score)` of the winner, or `None` if the list is empty.
pub fn best<'a, I, S>(query: &str, candidates: I) -> Option<(usize, u8)>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str> + 'a,
{
    let mut best: Option<(usize, u8)> = None;
    for (i, cand) in candidates.into_iter().enumerate() {
        let s = score(query, cand.as_ref());
        match best {
            Some((_, bs)) if bs >= s => {}
            _ => best = Some((i, s)),
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_is_100() {
        assert_eq!(score("Iron", "Iron"), 100);
        assert_eq!(score("iron", "IRON"), 100);
    }

    #[test]
    fn close_ocr_scores_high() {
        assert!(score("Bexalile", "Bexalite") >= 80);
        assert!(score("Hephaestanile", "Hephaestanite") >= 85);
        assert!(score("Ranta Dun", "Ranta Dung") >= 85);
    }

    #[test]
    fn token_order_insensitive() {
        assert_eq!(score("Dung Ranta", "Ranta Dung"), 100);
    }

    #[test]
    fn unrelated_scores_low() {
        assert!(score("Iron", "Quantanium") < 50);
    }

    #[test]
    fn best_picks_winner() {
        let cands = ["Iron", "Titanium", "Aluminum"];
        let (i, s) = best("Titaniom", cands).unwrap();
        assert_eq!(i, 1);
        assert!(s >= 80);
    }
}
