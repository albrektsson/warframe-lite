//! The scanned owned-relic set: what the OCR scanner persists and the planners
//! consume.
//!
//! A relic's identity on the Void Relics screen includes its **refinement** —
//! `Meso Z4 Relic`, `Meso Z4 Relic [Exceptional]`, `[Flawless]`, `[Radiant]` are
//! four distinct cards. Each `(code, refinement)` entry tracks two independent
//! trust tiers (see ADR-0009): **Seen** ([`mark_seen`]), set from a single
//! clean identity read, and a **Confirmed count** ([`apply_confirmed_count`]),
//! which still requires two agreeing frames (ADR-0005 unchanged). The fissure
//! planners only ever consume the confirmed-count projection ([`owned_counts`]), and
//! only entries with a confirmed count — a Seen-but-uncounted relic doesn't
//! feed those yet, because they need an actual number to rank by. Confirmed
//! counts are summed across every refinement ([`owned_counts`]) since a
//! relic's reward *set* doesn't depend on refinement — only drop chances do —
//! so a Radiant-only confirmed copy counts exactly like an Intact one.
//!
//! A count also carries a [`Source`] (ADR-0009's revision): `wf-mem`'s
//! mem-scan gives an exact, 100%-accurate reading ([`apply_exact_snapshot`]),
//! while the continuous OCR scan loop only ever produces a noisier
//! frame-agreement estimate ([`apply_confirmed_count`]). OCR needs a much
//! higher agreement bar to overwrite an exact `MemScan` count than to update
//! its own `Ocr` ones — see `src/main.rs`'s `RELIC_AGREEMENT_MEMSCAN_OVERRIDE`
//! — so a lucky pair of misread frames can't casually clobber a value read
//! directly from game memory. Once OCR does clear that higher bar, the entry
//! reverts to plain `Ocr` provenance and the normal, lower bar applies again.

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

impl Refinement {
    /// Parse a `wf-mem`-decoded refinement label (`RelicIdentity::refinement`,
    /// one of `"Intact"`/`"Exceptional"`/`"Flawless"`/`"Radiant"` per
    /// [`crate::relic_names::RelicNameIndex`]'s own fixed word list) into this
    /// enum. `None` for anything else — defensive; that index never emits any
    /// other word.
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "Intact" => Some(Refinement::Intact),
            "Exceptional" => Some(Refinement::Exceptional),
            "Flawless" => Some(Refinement::Flawless),
            "Radiant" => Some(Refinement::Radiant),
            _ => None,
        }
    }
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
/// than leave `{seen: false, count: None}` behind. `source` names where
/// `count`'s current value came from; it's meaningless while `count` is
/// `None` (defaults to [`Source::Ocr`] in that case, but nothing reads it).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OwnedEntry {
    pub seen: bool,
    pub count: Option<OwnedCount>,
    pub source: Source,
}

/// Where a relic entry's current confirmed `count` came from (ADR-0009's
/// revision). `wf-mem`'s mem-scan reads the game's own inventory payload
/// directly and is exact; the OCR scan loop only ever reaches agreement
/// across noisy frame reads. Used to raise the OCR overwrite bar against an
/// exact reading — see [`apply_confirmed_count`] and `src/main.rs`'s
/// `RELIC_AGREEMENT_MEMSCAN_OVERRIDE`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    #[default]
    Ocr,
    MemScan,
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

/// Project the owned set to a `display → count` map the planners
/// ([`crate::sell_picks`], [`crate::farm_picks`]) consume — summing the
/// confirmed count across every refinement of a relic (a relic's reward *set*
/// is refinement-independent; only drop chances differ), and dropping codes
/// with no confirmed count anywhere (they need an actual number to rank by; a
/// Seen-but-uncounted relic doesn't feed these — see [`owned_evidence`] for
/// the richer view [`crate::mastery_plan`]/[`crate::bom::buy_or_farm_plan`]
/// use instead).
pub fn owned_counts(owned: &OwnedRelics) -> HashMap<String, u32> {
    owned_evidence(owned)
        .into_iter()
        .filter_map(|(code, evidence)| match evidence {
            RelicEvidence::Confirmed(n) => Some((code, n)),
            RelicEvidence::SeenOnly => None,
        })
        .collect()
}

/// A relic's ownership trust tier for planning purposes (see ADR-0009):
/// either a confirmed total count (summed across every refinement) or, absent
/// any confirmed count, that the relic has merely been seen at least once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelicEvidence {
    SeenOnly,
    Confirmed(u32),
}

/// Project the owned set to a `display → evidence` map: [`RelicEvidence::Confirmed`]
/// with the confirmed count summed across every refinement when any
/// refinement has one, else [`RelicEvidence::SeenOnly`] when any refinement
/// has been seen without ever being confirmed, else the code is absent
/// entirely. Used by planners that want to surface a part reachable only
/// through a seen-but-unconfirmed relic rather than silently dropping it.
pub fn owned_evidence(owned: &OwnedRelics) -> HashMap<String, RelicEvidence> {
    owned
        .iter()
        .filter_map(|(code, by_ref)| {
            let confirmed: u32 = by_ref.values().filter_map(|e| e.count.as_ref()).map(|c| c.value).sum();
            if confirmed > 0 {
                return Some((code.clone(), RelicEvidence::Confirmed(confirmed)));
            }
            if by_ref.values().any(|e| e.seen) {
                return Some((code.clone(), RelicEvidence::SeenOnly));
            }
            None
        })
        .collect()
}

/// How long ago a relic code's Intact count was last confirmed, if it has
/// one. Intact-scoped: a relic whose only confirmed count is a refined copy
/// (which now still counts toward planning via [`owned_counts`]/
/// [`owned_evidence`]) has no Intact freshness marker here — a known,
/// acceptable gap in the staleness indicator, not a planning-correctness bug.
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
/// (a confirmed count is definitionally also a clean identity read). `source`
/// is stamped onto the entry (ADR-0009's revision) so a later call can tell
/// what this exact count came from — the caller (`src/main.rs`'s OCR scan
/// loop) is expected to have already checked [`count_source`] and raised its
/// agreement bar before calling this with [`Source::Ocr`] over an existing
/// [`Source::MemScan`] entry.
pub fn apply_confirmed_count(
    owned: &mut OwnedRelics,
    code: &str,
    refinement: Refinement,
    value: u32,
    source: Source,
) {
    if value == 0 {
        clear_entry(owned, code, refinement);
        return;
    }
    let entry = owned.entry(code.to_string()).or_default().entry(refinement).or_default();
    entry.seen = true;
    entry.count = Some(Stamped { value, fetched_at: wf_cache::now_unix() });
    entry.source = source;
}

/// The [`Source`] of `(code, refinement)`'s current confirmed count, if it has
/// one — `None` if the entry doesn't exist or has no count yet (Seen-only).
/// Used by the OCR scan loop to pick the agreement bar an incoming reading
/// must clear before [`apply_confirmed_count`] is called (see
/// `src/main.rs`'s `RELIC_AGREEMENT_MEMSCAN_OVERRIDE`).
pub fn count_source(owned: &OwnedRelics, code: &str, refinement: Refinement) -> Option<Source> {
    let entry = owned.get(code)?.get(&refinement)?;
    entry.count.as_ref()?;
    Some(entry.source)
}

/// Replace the owned-relic set with an exact `wf-mem` mem-scan reading
/// (ADR-0009's revision): every `(code, refinement, count)` in `snapshot` is
/// written as a [`Source::MemScan`]-tagged confirmed count (implying Seen),
/// and — critically, the one thing OCR can never itself prove — every
/// existing entry *not* present in `snapshot` is removed outright. A
/// mem-scanned inventory only ever lists relics actually owned (≥1), so
/// absence is authoritative proof of zero, not missing data; this is what
/// self-corrects a stale-high OCR entry for a relic that's since been fully
/// consumed in a Foundry build, which OCR's own scan loop structurally can't
/// do (a refined relic's card just disappears from the grid — see ADR-0010).
pub fn apply_exact_snapshot(owned: &mut OwnedRelics, snapshot: &[(String, Refinement, u32)]) {
    let keys: std::collections::HashSet<(&str, Refinement)> =
        snapshot.iter().map(|(code, r, _)| (code.as_str(), *r)).collect();
    owned.retain(|code, by_ref| {
        by_ref.retain(|r, _| keys.contains(&(code.as_str(), *r)));
        !by_ref.is_empty()
    });
    let fetched_at = wf_cache::now_unix();
    for (code, refinement, value) in snapshot {
        let entry = owned.entry(code.clone()).or_default().entry(*refinement).or_default();
        entry.seen = true;
        entry.count = Some(Stamped { value: *value, fetched_at });
        entry.source = Source::MemScan;
    }
}

/// Remove one `(code, refinement)` entry entirely. Used both by a confirmed
/// "unowned" eye reading (`apply_confirmed_count`'s `value == 0` case) and by
/// `wf-browse`'s user-initiated clear action, for the case the scanner can
/// never itself resolve: a refined relic's card simply disappears once its
/// count reaches zero (no eye icon), so no future scan can clear it (see
/// ADR-0010).
pub fn clear_entry(owned: &mut OwnedRelics, code: &str, refinement: Refinement) {
    if let Some(by_ref) = owned.get_mut(code) {
        by_ref.remove(&refinement);
        if by_ref.is_empty() {
            owned.remove(code);
        }
    }
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
        OwnedEntry { seen: true, count: Some(Stamped { value, fetched_at }), source: Source::Ocr }
    }

    fn seen_only() -> OwnedEntry {
        OwnedEntry { seen: true, count: None, source: Source::Ocr }
    }

    #[test]
    fn owned_counts_sums_confirmed_counts_across_refinements() {
        let mut owned: OwnedRelics = HashMap::new();
        owned.insert(
            "Meso B9".to_string(),
            HashMap::from([
                (Refinement::Intact, counted(15, 100)),
                (Refinement::Radiant, counted(3, 100)),
            ]),
        );
        // A relic with only refined copies still counts — refinement doesn't
        // change the reward set, only drop chances.
        owned.insert("Axi A1".to_string(), HashMap::from([(Refinement::Radiant, counted(2, 100))]));
        // Seen but never confirmed: doesn't feed owned_counts (ADR-0009) — see
        // owned_evidence for the richer view that does surface this.
        owned.insert("Lith K1".to_string(), HashMap::from([(Refinement::Intact, seen_only())]));
        let counts = owned_counts(&owned);
        assert_eq!(counts.get("Meso B9"), Some(&18));
        assert_eq!(counts.get("Axi A1"), Some(&2));
        assert_eq!(counts.get("Lith K1"), None);
    }

    #[test]
    fn owned_evidence_prefers_confirmed_over_seen_only_and_reports_seen_only_when_never_counted() {
        let mut owned: OwnedRelics = HashMap::new();
        // Confirmed (summed) even though one refinement is only seen.
        owned.insert(
            "Meso B9".to_string(),
            HashMap::from([
                (Refinement::Intact, counted(15, 100)),
                (Refinement::Radiant, seen_only()),
            ]),
        );
        // Seen on one refinement, never confirmed on any.
        owned.insert("Axi A22".to_string(), HashMap::from([(Refinement::Intact, seen_only())]));
        let evidence = owned_evidence(&owned);
        assert_eq!(evidence.get("Meso B9"), Some(&RelicEvidence::Confirmed(15)));
        assert_eq!(evidence.get("Axi A22"), Some(&RelicEvidence::SeenOnly));
        assert_eq!(evidence.get("Nonexistent"), None);
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
    fn a_pre_source_file_does_not_deserialize_as_the_new_schema() {
        // ADR-0009's original schema has no `source` field, so it must also
        // fail to parse rather than silently treat every existing entry as
        // Ocr-sourced (which would leave stale entries permanently immune
        // to nothing, but also never correctly protected as MemScan either).
        let pre_source = r#"{"Meso B9": {"Intact": {"seen": true, "count": {"value": 15, "fetched_at": 1000}}}}"#;
        assert!(serde_json::from_str::<OwnedRelics>(pre_source).is_err());
    }

    #[test]
    fn mark_seen_creates_an_entry_and_is_idempotent() {
        let mut owned: OwnedRelics = HashMap::new();
        assert!(mark_seen(&mut owned, "Meso B9", Refinement::Intact));
        assert!(owned["Meso B9"][&Refinement::Intact].seen);
        assert!(owned["Meso B9"][&Refinement::Intact].count.is_none());
        // Already seen: no change reported, and an existing count untouched.
        apply_confirmed_count(&mut owned, "Meso B9", Refinement::Intact, 15, Source::Ocr);
        assert!(!mark_seen(&mut owned, "Meso B9", Refinement::Intact));
        assert_eq!(owned["Meso B9"][&Refinement::Intact].count.as_ref().unwrap().value, 15);
    }

    #[test]
    fn apply_confirmed_count_implies_seen_and_stamps_source() {
        let mut owned: OwnedRelics = HashMap::new();
        apply_confirmed_count(&mut owned, "Meso B9", Refinement::Intact, 15, Source::Ocr);
        let entry = &owned["Meso B9"][&Refinement::Intact];
        assert!(entry.seen);
        assert_eq!(entry.count.as_ref().unwrap().value, 15);
        assert_eq!(entry.source, Source::Ocr);
    }

    #[test]
    fn apply_confirmed_count_zero_removes_the_entry_even_if_seen() {
        let mut owned: OwnedRelics = HashMap::new();
        mark_seen(&mut owned, "Meso B9", Refinement::Intact);
        apply_confirmed_count(&mut owned, "Meso B9", Refinement::Intact, 15, Source::Ocr);
        apply_confirmed_count(&mut owned, "Meso B9", Refinement::Intact, 0, Source::Ocr);
        assert!(!owned.contains_key("Meso B9"));
    }

    #[test]
    fn clear_entry_removes_only_the_named_refinement() {
        let mut owned: OwnedRelics = HashMap::new();
        apply_confirmed_count(&mut owned, "Meso B9", Refinement::Intact, 15, Source::Ocr);
        apply_confirmed_count(&mut owned, "Meso B9", Refinement::Radiant, 3, Source::Ocr);
        clear_entry(&mut owned, "Meso B9", Refinement::Radiant);
        assert!(owned["Meso B9"].contains_key(&Refinement::Intact));
        assert!(!owned["Meso B9"].contains_key(&Refinement::Radiant));
    }

    #[test]
    fn clear_entry_drops_the_code_once_its_last_refinement_is_gone() {
        let mut owned: OwnedRelics = HashMap::new();
        mark_seen(&mut owned, "Meso B9", Refinement::Intact);
        clear_entry(&mut owned, "Meso B9", Refinement::Intact);
        assert!(!owned.contains_key("Meso B9"));
    }

    #[test]
    fn count_source_reflects_who_wrote_the_current_count() {
        let mut owned: OwnedRelics = HashMap::new();
        assert_eq!(count_source(&owned, "Meso B9", Refinement::Intact), None);
        mark_seen(&mut owned, "Meso B9", Refinement::Intact);
        // Seen-only, no count yet: still no source.
        assert_eq!(count_source(&owned, "Meso B9", Refinement::Intact), None);
        apply_confirmed_count(&mut owned, "Meso B9", Refinement::Intact, 15, Source::MemScan);
        assert_eq!(count_source(&owned, "Meso B9", Refinement::Intact), Some(Source::MemScan));
        apply_confirmed_count(&mut owned, "Meso B9", Refinement::Intact, 16, Source::Ocr);
        assert_eq!(count_source(&owned, "Meso B9", Refinement::Intact), Some(Source::Ocr));
    }

    #[test]
    fn apply_exact_snapshot_writes_memscan_sourced_entries() {
        let mut owned: OwnedRelics = HashMap::new();
        apply_exact_snapshot(
            &mut owned,
            &[("Axi B3".to_string(), Refinement::Intact, 9), ("Meso B9".to_string(), Refinement::Radiant, 2)],
        );
        assert_eq!(owned["Axi B3"][&Refinement::Intact].count.as_ref().unwrap().value, 9);
        assert_eq!(owned["Axi B3"][&Refinement::Intact].source, Source::MemScan);
        assert!(owned["Axi B3"][&Refinement::Intact].seen);
        assert_eq!(owned["Meso B9"][&Refinement::Radiant].count.as_ref().unwrap().value, 2);
    }

    #[test]
    fn apply_exact_snapshot_clears_entries_absent_from_the_snapshot() {
        // A relic OCR previously confirmed, but the mem-scan reading (which
        // only ever lists relics actually owned) doesn't mention at all —
        // proof of zero, so it must be dropped, not left stale.
        let mut owned: OwnedRelics = HashMap::new();
        apply_confirmed_count(&mut owned, "Lith K1", Refinement::Intact, 3, Source::Ocr);
        apply_exact_snapshot(&mut owned, &[("Axi B3".to_string(), Refinement::Intact, 9)]);
        assert!(!owned.contains_key("Lith K1"));
        assert!(owned.contains_key("Axi B3"));
    }

    #[test]
    fn apply_exact_snapshot_only_clears_the_refinement_not_the_whole_code() {
        // Same relic code, but a refinement the snapshot doesn't cover must
        // still be cleared independently — absence is per (code, refinement).
        let mut owned: OwnedRelics = HashMap::new();
        apply_confirmed_count(&mut owned, "Axi B3", Refinement::Intact, 5, Source::Ocr);
        apply_confirmed_count(&mut owned, "Axi B3", Refinement::Radiant, 1, Source::Ocr);
        apply_exact_snapshot(&mut owned, &[("Axi B3".to_string(), Refinement::Intact, 5)]);
        assert!(owned["Axi B3"].contains_key(&Refinement::Intact));
        assert!(!owned["Axi B3"].contains_key(&Refinement::Radiant));
    }

    #[test]
    fn refinement_from_label_parses_the_four_known_words_and_rejects_others() {
        assert_eq!(Refinement::from_label("Intact"), Some(Refinement::Intact));
        assert_eq!(Refinement::from_label("Exceptional"), Some(Refinement::Exceptional));
        assert_eq!(Refinement::from_label("Flawless"), Some(Refinement::Flawless));
        assert_eq!(Refinement::from_label("Radiant"), Some(Refinement::Radiant));
        assert_eq!(Refinement::from_label("Bronze"), None);
    }
}
