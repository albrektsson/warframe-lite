//! The scanned owned-relic set: what the OCR scanner persists and the planners
//! consume.
//!
//! A relic's identity on the Void Relics screen includes its **refinement** —
//! `Meso Z4 Relic`, `Meso Z4 Relic [Exceptional]`, `[Flawless]`, `[Radiant]` are
//! four distinct cards. Each `(code, refinement)` entry tracks two independent
//! trust tiers (see ADR-0009): **Seen** ([`mark_seen`]), set from a single
//! clean identity read, and a **Confirmed count** ([`apply_confirmed_count`]),
//! which still requires two agreeing frames (ADR-0005 unchanged). The fissure
//! planners only ever consume the Intact projection ([`intact_counts`]), and
//! only entries with a confirmed count — a Seen-but-uncounted relic doesn't
//! feed those yet, because they need an actual number to rank by.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use wf_cache::Stamped;

use crate::index::{levenshtein, normalize};

/// A Void Relic's refinement state. Intact is the unrefined default (shown with
/// no suffix on the Void Relics screen); the other three appear as a bracketed
/// suffix and improve the relic's rare-drop odds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Refinement {
    Intact,
    Exceptional,
    Flawless,
    Radiant,
}

/// One confirmed owned count, stamped with when a scan last confirmed it: the
/// [`Stamped`] value is the count, its `fetched_at` is the last-seen unix time.
/// Reusing [`Stamped`] gives per-entry age (`.age()`) for the scan-age indicator
/// for free.
pub type OwnedCount = Stamped<u32>;

/// One `(code, refinement)`'s owned-relic state: two independent trust tiers
/// (see ADR-0009). `seen` is set from a single clean identity read and never
/// itself gates or is gated by `count`, which keeps requiring two agreeing
/// frames to confirm (ADR-0005). An entry always has at least one of the two
/// set — [`apply_confirmed_count`] and [`mark_seen`] remove it entirely rather
/// than leave `{seen: false, count: None}` behind.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OwnedEntry {
    pub seen: bool,
    pub count: Option<OwnedCount>,
}

/// The persisted owned-relic set: relic display code → refinement → entry.
/// Serialised to `owned-relics.json` (see [`crate::OWNED_RELICS_FILE`]).
pub type OwnedRelics = HashMap<String, HashMap<Refinement, OwnedEntry>>;

/// Counts older than this are flagged stale in the UI — a hint that the entry
/// hasn't been re-confirmed recently and may no longer match the in-game
/// inventory. Tunable; deliberately generous so relics you simply haven't
/// scrolled past lately aren't nagged about constantly.
pub const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Split an OCR'd relic card label into its base display code and its
/// [`Refinement`], e.g. `"Meso Z4 Relic [Radiant]"` → `("Meso Z4", Radiant)` and
/// `"Neo T2 Relic"` → `("Neo T2", Intact)`.
///
/// A bracketed suffix means the card is refined, full stop — so when one is
/// present we pick the *nearest* of the three refined states (never Intact),
/// which keeps an OCR-garbled `[Radiant]` from silently leaking into the Intact
/// count. Only a card with no suffix at all is treated as Intact.
pub fn parse_refinement(name: &str) -> (String, Refinement) {
    let t = name.trim();
    if let (Some(open), Some(close)) = (t.rfind('['), t.rfind(']')) {
        if close > open {
            let inside = &t[open + 1..close];
            let refinement = nearest_refined(inside);
            return (strip_relic_word(t[..open].trim()), refinement);
        }
    }
    (strip_relic_word(t), Refinement::Intact)
}

/// The refined state (never Intact) whose name is closest to `inside` — a
/// bracket suffix is present, so the card is definitely refined even if the
/// suffix text came through the OCR a little garbled.
fn nearest_refined(inside: &str) -> Refinement {
    let n = normalize(inside);
    [
        (Refinement::Exceptional, "exceptional"),
        (Refinement::Flawless, "flawless"),
        (Refinement::Radiant, "radiant"),
    ]
    .into_iter()
    .min_by_key(|(_, word)| levenshtein(n.as_bytes(), word.as_bytes()))
    .map(|(r, _)| r)
    .unwrap_or(Refinement::Radiant)
}

/// Strip a trailing "Relic"/"Relics" word from an OCR'd relic label so it matches
/// a [`crate::RelicIndex`] code, e.g. `"Neo T2 Relic"` → `"Neo T2"`.
fn strip_relic_word(s: &str) -> String {
    let t = s.trim();
    let lower = t.to_lowercase();
    for suf in [" relics", " relic"] {
        if let Some(stripped) = lower.strip_suffix(suf) {
            return t[..stripped.len()].trim().to_string();
        }
    }
    t.to_string()
}

/// Project the owned set to the Intact-only `display → count` map the planners
/// ([`crate::mastery_plan`], [`crate::sell_picks`], [`crate::farm_picks`])
/// consume — only entries with a confirmed count (they need an actual number
/// to rank by; a Seen-but-uncounted relic doesn't feed these yet). Non-Intact
/// copies are dropped: relic drop tables are Intact-only.
pub fn intact_counts(owned: &OwnedRelics) -> HashMap<String, u32> {
    owned
        .iter()
        .filter_map(|(code, by_ref)| {
            let count = by_ref.get(&Refinement::Intact)?.count.as_ref()?;
            Some((code.clone(), count.value))
        })
        .collect()
}

/// How long ago a relic code's Intact count was last confirmed, if it has one.
pub fn intact_age(owned: &OwnedRelics, code: &str) -> Option<Duration> {
    owned.get(code)?.get(&Refinement::Intact)?.count.as_ref().map(|s| s.age())
}

/// Per-code age of each Intact count's last-seen stamp — the source for the
/// per-relic freshness markers in the browse UI.
pub fn intact_ages(owned: &OwnedRelics) -> HashMap<String, Duration> {
    owned
        .iter()
        .filter_map(|(code, by_ref)| {
            let count = by_ref.get(&Refinement::Intact)?.count.as_ref()?;
            Some((code.clone(), count.age()))
        })
        .collect()
}

/// The freshest and stalest ages across all Intact entries, for the summary
/// freshness line — `(newest, oldest)`. `None` when there are no Intact entries.
pub fn intact_age_range(owned: &OwnedRelics) -> Option<(Duration, Duration)> {
    let ages: Vec<Duration> = owned
        .values()
        .filter_map(|by_ref| by_ref.get(&Refinement::Intact)?.count.as_ref().map(|s| s.age()))
        .collect();
    Some((*ages.iter().min()?, *ages.iter().max()?))
}

/// Mark `(code, refinement)` as **Seen**: the card's name+refinement matched
/// the catalogue on a single clean read, with no "unowned" eye icon (see
/// ADR-0009). Never touches an existing confirmed count. Returns whether this
/// changed anything (a card that's already Seen is a no-op), so callers only
/// pay for a save/UI refresh when the owned set actually grew.
pub fn mark_seen(owned: &mut OwnedRelics, code: &str, refinement: Refinement) -> bool {
    let entry = owned.entry(code.to_string()).or_default().entry(refinement).or_default();
    if entry.seen {
        return false;
    }
    entry.seen = true;
    true
}

/// Apply a confirmed `(code, refinement)` count (see ADR-0005): a value of `0`
/// (a confirmed "unowned" eye reading) removes the entry entirely — positive
/// proof the player owns none, overriding any prior Seen — while a positive
/// value replaces the count, refreshes its last-seen stamp, and implies Seen
/// (a confirmed count is definitionally also a clean identity read).
pub fn apply_confirmed_count(owned: &mut OwnedRelics, code: &str, refinement: Refinement, value: u32) {
    if value == 0 {
        if let Some(by_ref) = owned.get_mut(code) {
            by_ref.remove(&refinement);
            if by_ref.is_empty() {
                owned.remove(code);
            }
        }
        return;
    }
    let entry = owned.entry(code.to_string()).or_default().entry(refinement).or_default();
    entry.seen = true;
    entry.count = Some(Stamped { value, fetched_at: wf_cache::now_unix() });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_refinement_reads_the_suffix_and_defaults_intact() {
        assert_eq!(parse_refinement("Meso B9 Relic"), ("Meso B9".to_string(), Refinement::Intact));
        assert_eq!(
            parse_refinement("Meso Z4 Relic [Radiant]"),
            ("Meso Z4".to_string(), Refinement::Radiant)
        );
        assert_eq!(
            parse_refinement("Axi H3 Relic [Exceptional]"),
            ("Axi H3".to_string(), Refinement::Exceptional)
        );
        assert_eq!(
            parse_refinement("Lith K1 Relic [Flawless]"),
            ("Lith K1".to_string(), Refinement::Flawless)
        );
    }

    #[test]
    fn parse_refinement_never_leaks_a_garbled_suffix_into_intact() {
        // A present-but-misread suffix is still a refined card, not Intact.
        let (code, r) = parse_refinement("Meso Z4 Relic [Rodiant]");
        assert_eq!(code, "Meso Z4");
        assert_eq!(r, Refinement::Radiant);
    }

    fn counted(value: u32, fetched_at: u64) -> OwnedEntry {
        OwnedEntry { seen: true, count: Some(Stamped { value, fetched_at }) }
    }

    fn seen_only() -> OwnedEntry {
        OwnedEntry { seen: true, count: None }
    }

    #[test]
    fn intact_counts_drops_refined_copies_and_uncounted_seen_entries() {
        let mut owned: OwnedRelics = HashMap::new();
        owned.insert(
            "Meso B9".to_string(),
            HashMap::from([
                (Refinement::Intact, counted(15, 100)),
                (Refinement::Radiant, counted(3, 100)),
            ]),
        );
        // A relic with only refined copies contributes nothing to the Intact plan.
        owned.insert("Axi A1".to_string(), HashMap::from([(Refinement::Radiant, counted(2, 100))]));
        // Seen but never confirmed: doesn't feed the planners yet (ADR-0009).
        owned.insert("Lith K1".to_string(), HashMap::from([(Refinement::Intact, seen_only())]));
        let intact = intact_counts(&owned);
        assert_eq!(intact.get("Meso B9"), Some(&15));
        assert_eq!(intact.get("Axi A1"), None);
        assert_eq!(intact.get("Lith K1"), None);
    }

    #[test]
    fn owned_relics_roundtrips_through_json() {
        let mut owned: OwnedRelics = HashMap::new();
        owned.insert(
            "Meso B9".to_string(),
            HashMap::from([
                (Refinement::Intact, counted(15, 1000)),
                (Refinement::Radiant, seen_only()),
            ]),
        );
        let json = serde_json::to_string(&owned).unwrap();
        let back: OwnedRelics = serde_json::from_str(&json).unwrap();
        assert_eq!(back["Meso B9"][&Refinement::Intact].count.as_ref().unwrap().value, 15);
        assert!(back["Meso B9"][&Refinement::Radiant].seen);
        assert!(back["Meso B9"][&Refinement::Radiant].count.is_none());
    }

    #[test]
    fn a_legacy_flat_u32_file_does_not_deserialize_as_the_new_schema() {
        // The old format was `HashMap<String, u32>`; it must fail to parse as the
        // nested schema so the loader discards (and backs up) it rather than
        // silently misreading counts.
        let legacy = r#"{"Meso B9": 15, "Axi A1": 2}"#;
        assert!(serde_json::from_str::<OwnedRelics>(legacy).is_err());
    }

    #[test]
    fn a_pre_seen_tier_file_does_not_deserialize_as_the_new_schema() {
        // ADR-0005's schema has no `seen` field, so it must also fail to parse.
        let pre_seen = r#"{"Meso B9": {"Intact": {"value": 15, "fetched_at": 1000}}}"#;
        assert!(serde_json::from_str::<OwnedRelics>(pre_seen).is_err());
    }

    #[test]
    fn mark_seen_creates_an_entry_and_is_idempotent() {
        let mut owned: OwnedRelics = HashMap::new();
        assert!(mark_seen(&mut owned, "Meso B9", Refinement::Intact));
        assert!(owned["Meso B9"][&Refinement::Intact].seen);
        assert!(owned["Meso B9"][&Refinement::Intact].count.is_none());
        // Already seen: no change reported, and an existing count untouched.
        apply_confirmed_count(&mut owned, "Meso B9", Refinement::Intact, 15);
        assert!(!mark_seen(&mut owned, "Meso B9", Refinement::Intact));
        assert_eq!(owned["Meso B9"][&Refinement::Intact].count.as_ref().unwrap().value, 15);
    }

    #[test]
    fn apply_confirmed_count_implies_seen() {
        let mut owned: OwnedRelics = HashMap::new();
        apply_confirmed_count(&mut owned, "Meso B9", Refinement::Intact, 15);
        let entry = &owned["Meso B9"][&Refinement::Intact];
        assert!(entry.seen);
        assert_eq!(entry.count.as_ref().unwrap().value, 15);
    }

    #[test]
    fn apply_confirmed_count_zero_removes_the_entry_even_if_seen() {
        let mut owned: OwnedRelics = HashMap::new();
        mark_seen(&mut owned, "Meso B9", Refinement::Intact);
        apply_confirmed_count(&mut owned, "Meso B9", Refinement::Intact, 15);
        apply_confirmed_count(&mut owned, "Meso B9", Refinement::Intact, 0);
        assert!(!owned.contains_key("Meso B9"));
    }
}
