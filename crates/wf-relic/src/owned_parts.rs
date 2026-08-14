//! The owned-Prime-Part set: what the Inventory/Sell screen OCR scanner and
//! `wf-mem`'s mem-scan both persist, and the reward panel / Relics & Plan /
//! Sell tabs consume.
//!
//! Unlike [`crate::OwnedRelics`] (which tracks two independent trust tiers
//! per ADR-0009 — Seen and a separately-confirmed count), this screen's badge
//! is always a single-frame passive read: name and count are read together,
//! with no fast-scroll frame-split risk the way the Void Relics grid has. So
//! there's nothing for a Seen/Confirmed split to distinguish here — an entry
//! is a bare count, no Seen wrapper.
//!
//! An entry does carry a [`Source`] (ADR-0009's revision, issue #81): a
//! `wf-mem` mem-scan reading is exact, while the OCR scan loop only ever
//! reaches a frame-agreement estimate — the same asymmetry ADR-0009's
//! revision protects against for relics, reused verbatim here rather than
//! duplicated, since the provenance question ("who wrote the current count,
//! and how sure were they") doesn't depend on whether the count has a Seen
//! tier alongside it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::mastery::PrimePart;
use crate::owned::{OwnedCount, Source};

/// One owned-Prime-Part-component reading: a [`Stamped`](wf_cache::Stamped)
/// count plus which scanner last wrote it (see module doc and
/// [`crate::owned::Source`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartCount {
    pub count: OwnedCount,
    pub source: Source,
}

/// The persisted owned-Prime-Part set: Built Prime → part label → entry.
/// Serialised to `owned-prime-parts.json` (see
/// [`OWNED_PRIME_PARTS_FILE`]), mirroring [`crate::OwnedRelics`]'s
/// `code → refinement → entry` shape one level down (part label instead of
/// refinement).
pub type OwnedPrimeParts = HashMap<String, HashMap<String, PartCount>>;

/// The file `owned-prime-parts.json` is cached under, via
/// `wf_cache::load_blob`/`save_blob` — written by both the Inventory/Sell OCR
/// scanner and `wf-mem`'s mem-scan (ADR-0001, ADR-0003). Shared so every
/// consumer (the overlay's scanner, `wf-browse`) names the same file.
pub const OWNED_PRIME_PARTS_FILE: &str = "owned-prime-parts.json";

/// Marker file recording that a `wf-mem` mem-scan of Prime Part ownership has
/// completed at least once (written unconditionally by the mem-scan persist
/// path right after it successfully replaces [`OWNED_PRIME_PARTS_FILE`] via
/// [`apply_exact_snapshot`] — even when that snapshot was empty, since an
/// all-zero inventory is exactly the case [`get_or_confirmed_zero`] needs to
/// tell apart from "never scanned").
pub const OWNED_PARTS_MEM_SCANNED_MARKER_FILE: &str = "owned-prime-parts-mem-scanned.json";

/// How many of `part` the player owns, if it's ever been scanned. `None`
/// means unknown — no scanner has ever observed this part — never zero (see
/// ADR-0011's "never guess an unknown quantity" precedent, applied here to
/// Prime Part counts rather than build quantities).
pub fn get(owned: &OwnedPrimeParts, part: &PrimePart) -> Option<u32> {
    owned.get(&part.prime)?.get(&part.part).map(|c| c.count.value)
}

/// [`get`], but once the player's Prime Part inventory has been mem-scanned
/// at least once (`mem_scanned` — see [`OWNED_PARTS_MEM_SCANNED_MARKER_FILE`]),
/// a missing entry becomes `Some(0)` instead of `None`. This isn't guessing:
/// [`apply_exact_snapshot`]'s own contract already treats "absent from a
/// mem-scan snapshot" as authoritative proof of zero, not missing data — so
/// surfacing that absence as unknown here would just reintroduce the
/// ambiguity the mem-scan already resolved. With `mem_scanned` false (an
/// account that's only ever used the lower-trust, opt-in OCR scanner, which
/// never proves a zero), this is identical to [`get`].
pub fn get_or_confirmed_zero(owned: &OwnedPrimeParts, part: &PrimePart, mem_scanned: bool) -> Option<u32> {
    get(owned, part).or(mem_scanned.then_some(0))
}

/// The [`Source`] of `part`'s current count, if it has one — `None` if the
/// part has never been scanned. Used by the OCR scan loop to pick the
/// agreement bar an incoming reading must clear before [`apply_count`] is
/// called (see `src/ocr_enabled.rs`'s `INVENTORY_AGREEMENT_MEMSCAN_OVERRIDE`,
/// mirroring `crate::count_source`'s relic equivalent).
pub fn source(owned: &OwnedPrimeParts, part: &PrimePart) -> Option<Source> {
    Some(owned.get(&part.prime)?.get(&part.part)?.source)
}

/// Apply one `(prime, part)` reading: sets the count, refreshes its
/// last-seen stamp, and stamps `source`. The Inventory/Sell screen never
/// lists a 0-owned card (see issue #37's region-calibration research), so
/// unlike [`crate::apply_confirmed_count`] there is no "value == 0 removes
/// the entry" case here for an OCR reading — a confirmed-zero only ever
/// arrives via [`apply_exact_snapshot`]'s absence convention.
pub fn apply_count(owned: &mut OwnedPrimeParts, part: &PrimePart, value: u32, source: Source) {
    owned.entry(part.prime.clone()).or_default().insert(
        part.part.clone(),
        PartCount { count: OwnedCount { value, fetched_at: wf_cache::now_unix() }, source },
    );
}

/// Replace the owned-Prime-Part set with an exact `wf-mem` mem-scan reading
/// (ADR-0009's revision, applied here per issue #81): every `(part, count)`
/// in `snapshot` is written as a [`Source::MemScan`]-tagged entry, and every
/// existing entry *not* present in `snapshot` is removed outright. A
/// mem-scanned inventory only ever lists components actually owned (≥1), so
/// absence is authoritative proof of zero, not missing data — the same
/// self-correction `crate::apply_exact_snapshot` gives relics, for the same
/// class of staleness (a part fully consumed in a Foundry build since the
/// last OCR pass) OCR's own scan loop can't itself prove.
pub fn apply_exact_snapshot(owned: &mut OwnedPrimeParts, snapshot: &[(PrimePart, u32)]) {
    let keys: std::collections::HashSet<(&str, &str)> =
        snapshot.iter().map(|(pp, _)| (pp.prime.as_str(), pp.part.as_str())).collect();
    owned.retain(|prime, by_part| {
        by_part.retain(|part, _| keys.contains(&(prime.as_str(), part.as_str())));
        !by_part.is_empty()
    });
    let fetched_at = wf_cache::now_unix();
    for (pp, value) in snapshot {
        owned.entry(pp.prime.clone()).or_default().insert(
            pp.part.clone(),
            PartCount { count: OwnedCount { value: *value, fetched_at }, source: Source::MemScan },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pp(prime: &str, part: &str) -> PrimePart {
        PrimePart { prime: prime.to_string(), part: part.to_string() }
    }

    #[test]
    fn get_is_none_for_a_never_scanned_part() {
        let owned = OwnedPrimeParts::new();
        assert_eq!(get(&owned, &pp("Ember Prime", "Systems")), None);
    }

    #[test]
    fn apply_count_then_get_roundtrips() {
        let mut owned = OwnedPrimeParts::new();
        apply_count(&mut owned, &pp("Ember Prime", "Systems"), 3, Source::Ocr);
        assert_eq!(get(&owned, &pp("Ember Prime", "Systems")), Some(3));
        // A different part of the same prime is untouched.
        assert_eq!(get(&owned, &pp("Ember Prime", "Chassis")), None);
    }

    #[test]
    fn apply_count_overwrites_a_prior_reading() {
        let mut owned = OwnedPrimeParts::new();
        apply_count(&mut owned, &pp("Ember Prime", "Systems"), 3, Source::Ocr);
        apply_count(&mut owned, &pp("Ember Prime", "Systems"), 5, Source::Ocr);
        assert_eq!(get(&owned, &pp("Ember Prime", "Systems")), Some(5));
    }

    #[test]
    fn apply_count_stamps_source() {
        let mut owned = OwnedPrimeParts::new();
        apply_count(&mut owned, &pp("Ember Prime", "Systems"), 3, Source::MemScan);
        assert_eq!(source(&owned, &pp("Ember Prime", "Systems")), Some(Source::MemScan));
        apply_count(&mut owned, &pp("Ember Prime", "Systems"), 4, Source::Ocr);
        assert_eq!(source(&owned, &pp("Ember Prime", "Systems")), Some(Source::Ocr));
    }

    #[test]
    fn source_is_none_for_a_never_scanned_part() {
        let owned = OwnedPrimeParts::new();
        assert_eq!(source(&owned, &pp("Ember Prime", "Systems")), None);
    }

    #[test]
    fn get_or_confirmed_zero_is_none_without_a_mem_scan() {
        let owned = OwnedPrimeParts::new();
        assert_eq!(get_or_confirmed_zero(&owned, &pp("Ember Prime", "Systems"), false), None);
    }

    #[test]
    fn get_or_confirmed_zero_is_zero_after_a_mem_scan_when_absent() {
        let owned = OwnedPrimeParts::new();
        assert_eq!(get_or_confirmed_zero(&owned, &pp("Ember Prime", "Systems"), true), Some(0));
    }

    #[test]
    fn get_or_confirmed_zero_prefers_a_real_reading_over_the_flag() {
        let mut owned = OwnedPrimeParts::new();
        apply_count(&mut owned, &pp("Ember Prime", "Systems"), 3, Source::Ocr);
        assert_eq!(get_or_confirmed_zero(&owned, &pp("Ember Prime", "Systems"), true), Some(3));
    }

    #[test]
    fn owned_prime_parts_roundtrips_through_json() {
        let mut owned = OwnedPrimeParts::new();
        apply_count(&mut owned, &pp("Ember Prime", "Systems"), 3, Source::MemScan);
        let json = serde_json::to_string(&owned).unwrap();
        let back: OwnedPrimeParts = serde_json::from_str(&json).unwrap();
        assert_eq!(get(&back, &pp("Ember Prime", "Systems")), Some(3));
        assert_eq!(source(&back, &pp("Ember Prime", "Systems")), Some(Source::MemScan));
    }

    #[test]
    fn a_pre_source_file_does_not_deserialize_as_the_new_schema() {
        // The pre-#81 schema was a bare Stamped<u32> (`{value, fetched_at}`,
        // no `source` sibling) — it must fail to parse rather than silently
        // default every existing entry to `Ocr` provenance, which would
        // leave a genuinely mem-scanned reading unprotected from casual OCR
        // overwrite (see `crate::owned`'s own equivalent test).
        let pre_source = r#"{"Ember Prime": {"Systems": {"value": 3, "fetched_at": 1000}}}"#;
        assert!(serde_json::from_str::<OwnedPrimeParts>(pre_source).is_err());
    }

    #[test]
    fn apply_exact_snapshot_writes_memscan_sourced_entries() {
        let mut owned = OwnedPrimeParts::new();
        apply_exact_snapshot(
            &mut owned,
            &[(pp("Ember Prime", "Systems"), 3), (pp("Rhino Prime", "Chassis"), 1)],
        );
        assert_eq!(get(&owned, &pp("Ember Prime", "Systems")), Some(3));
        assert_eq!(source(&owned, &pp("Ember Prime", "Systems")), Some(Source::MemScan));
        assert_eq!(get(&owned, &pp("Rhino Prime", "Chassis")), Some(1));
    }

    #[test]
    fn apply_exact_snapshot_clears_entries_absent_from_the_snapshot() {
        let mut owned = OwnedPrimeParts::new();
        apply_count(&mut owned, &pp("Ember Prime", "Systems"), 3, Source::Ocr);
        apply_exact_snapshot(&mut owned, &[(pp("Rhino Prime", "Chassis"), 1)]);
        assert_eq!(get(&owned, &pp("Ember Prime", "Systems")), None);
        assert_eq!(get(&owned, &pp("Rhino Prime", "Chassis")), Some(1));
    }

    #[test]
    fn apply_exact_snapshot_only_clears_the_part_not_the_whole_prime() {
        let mut owned = OwnedPrimeParts::new();
        apply_count(&mut owned, &pp("Ember Prime", "Systems"), 3, Source::Ocr);
        apply_count(&mut owned, &pp("Ember Prime", "Chassis"), 2, Source::Ocr);
        apply_exact_snapshot(&mut owned, &[(pp("Ember Prime", "Systems"), 3)]);
        assert_eq!(get(&owned, &pp("Ember Prime", "Systems")), Some(3));
        assert_eq!(get(&owned, &pp("Ember Prime", "Chassis")), None);
    }
}
