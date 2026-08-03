//! Screen-agnostic trust core for OCR'd inventory-grid counts.
//!
//! Two pieces the relic scan (and the forthcoming equipment scan) share:
//!
//! * [`parse_badge`] — turn one noisy OCR of a count badge (`"x15"`) into a
//!   number, *rejecting* anything that isn't a single clean integer under a
//!   plausibility cap rather than guessing (see ADR-0005). It never invents a
//!   value: an unparseable badge returns `None` ("abstain"), so it casts no
//!   vote instead of silently defaulting.
//! * [`Tally`] — accumulate many per-frame reads of the same key and only
//!   *confirm* a value once enough frames agree on it, defeating a lone OCR
//!   outlier (the `x145`-for-`x15` misread that the old `max`-merge locked in).

use std::collections::HashMap;
use std::hash::Hash;

/// Parse one count-badge OCR string into its integer value.
///
/// Accepts an optional single leading `x`/`X`/`×` followed by digits only, with
/// the resulting value in `1..=max`. Anything else — an empty/whitespace string,
/// a value of zero or above `max`, embedded non-digits, or two separate tokens
/// (`"x1 x4"`, `"x1 5"`) — returns `None` so the caller abstains rather than
/// trusting a garbled read. A genuinely blank badge (the crop has no ink at all,
/// which for relics means a single copy) is the caller's concern, not this
/// function's: this only sees text that was actually recognised.
pub fn parse_badge(text: &str, max: u32) -> Option<u32> {
    let t = text.trim();
    // Strip a single leading count marker if present; the rest must be digits.
    let digits = t
        .strip_prefix('x')
        .or_else(|| t.strip_prefix('X'))
        .or_else(|| t.strip_prefix('×'))
        .unwrap_or(t);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    (1..=max).contains(&n).then_some(n)
}

/// A per-key histogram of per-frame count reads, resolved by majority vote.
///
/// The scan records every frame's reading for a key (a relic code+refinement,
/// or an equipment slot). [`confirmed`](Self::confirmed) returns the most-read
/// value, but only once it clears a minimum agreement floor — so a value seen on
/// a single frame is never believed, and the true value overtakes a lucky
/// coincidence as the player dwells. Ties (equal frame counts) resolve to the
/// **lower** value: inflation is the failure mode we are guarding against.
#[derive(Debug, Default, Clone)]
pub struct Tally<K: Eq + Hash> {
    reads: HashMap<K, HashMap<u32, u32>>,
}

impl<K: Eq + Hash> Tally<K> {
    pub fn new() -> Self {
        Self { reads: HashMap::new() }
    }

    /// Record one frame's reading of `value` for `key`.
    pub fn record(&mut self, key: K, value: u32) {
        *self.reads.entry(key).or_default().entry(value).or_insert(0) += 1;
    }

    /// The confirmed value for `key`: the most-frequently-read value, provided it
    /// was read on at least `min_agreement` frames. `None` until that floor is
    /// met. On a frame-count tie, the lower value wins (conservative against
    /// over-counting).
    pub fn confirmed(&self, key: &K, min_agreement: u32) -> Option<u32> {
        let hist = self.reads.get(key)?;
        let (&value, &frames) = hist
            .iter()
            // Most frames first; tie-break to the lower value.
            .max_by(|(av, af), (bv, bf)| af.cmp(bf).then(bv.cmp(av)))?;
        (frames >= min_agreement).then_some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_badges_with_and_without_the_x() {
        assert_eq!(parse_badge("x15", 40), Some(15));
        assert_eq!(parse_badge("X7", 40), Some(7));
        assert_eq!(parse_badge("  x15\n", 40), Some(15)); // surrounding whitespace ok
        assert_eq!(parse_badge("15", 40), Some(15)); // dropped 'x' still parses
    }

    #[test]
    fn rejects_over_cap_and_concatenation_artifacts() {
        // The exact real-world failure: a concatenated overcount above the cap.
        assert_eq!(parse_badge("x145", 40), None);
        // Two tokens must never merge into one number.
        assert_eq!(parse_badge("x1 x4", 40), None);
        assert_eq!(parse_badge("x1 5", 40), None);
    }

    #[test]
    fn rejects_junk_and_zero_and_empty() {
        assert_eq!(parse_badge("", 40), None);
        assert_eq!(parse_badge("   ", 40), None);
        assert_eq!(parse_badge("~", 40), None);
        assert_eq!(parse_badge("x", 40), None); // marker but no digits
        assert_eq!(parse_badge("x0", 40), None); // zero is not a real owned count
        assert_eq!(parse_badge("x12a", 40), None); // trailing non-digit
    }

    #[test]
    fn tally_confirms_only_after_the_agreement_floor() {
        let mut t: Tally<&str> = Tally::new();
        t.record("B9", 15);
        // A single read never confirms.
        assert_eq!(t.confirmed(&"B9", 2), None);
        t.record("B9", 15);
        assert_eq!(t.confirmed(&"B9", 2), Some(15));
    }

    #[test]
    fn tally_mode_beats_a_lone_outlier() {
        let mut t: Tally<&str> = Tally::new();
        for _ in 0..5 {
            t.record("B9", 15);
        }
        t.record("B9", 145); // one bad frame
        t.record("B9", 19); // another bad frame
        assert_eq!(t.confirmed(&"B9", 2), Some(15));
    }

    #[test]
    fn tally_breaks_ties_toward_the_lower_value() {
        let mut t: Tally<&str> = Tally::new();
        t.record("B9", 15);
        t.record("B9", 19);
        // 1 frame each, floor of 1 → tie broken to the lower (conservative) value.
        assert_eq!(t.confirmed(&"B9", 1), Some(15));
    }
}
