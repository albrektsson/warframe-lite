//! The scanned owned-Prime-Part set: what the Inventory/Sell screen OCR
//! scanner persists, and the reward panel / Relics & Plan / Sell tabs
//! consume.
//!
//! Unlike [`crate::OwnedRelics`] (which tracks two independent trust tiers
//! per ADR-0009 — Seen and a separately-confirmed count), this screen's badge
//! is always a single-frame passive read: name and count are read together,
//! with no fast-scroll frame-split risk the way the Void Relics grid has. So
//! there's nothing for a Seen/Confirmed split to distinguish here — an entry
//! is a bare [`OwnedCount`] (a `Stamped<u32>`), no wrapper.

use std::collections::HashMap;

use crate::mastery::PrimePart;
use crate::owned::OwnedCount;

/// The persisted owned-Prime-Part set: Built Prime → part label → owned
/// count. Serialised to `owned-prime-parts.json` (see
/// [`OWNED_PRIME_PARTS_FILE`]), mirroring [`crate::OwnedRelics`]'s
/// `code → refinement → entry` shape one level down (part label instead of
/// refinement).
pub type OwnedPrimeParts = HashMap<String, HashMap<String, OwnedCount>>;

/// The file `owned-prime-parts.json` is cached under, via
/// `wf_cache::load_blob`/`save_blob` — the Inventory/Sell OCR scanner's only
/// persisted record of owned Prime Part counts (ADR-0001, ADR-0003). Shared
/// so every consumer (the overlay's scanner, `wf-browse`) names the same
/// file.
pub const OWNED_PRIME_PARTS_FILE: &str = "owned-prime-parts.json";

/// How many of `part` the player owns, if it's ever been scanned. `None`
/// means unknown — the scanner has never observed this part's card — never
/// zero (see ADR-0011's "never guess an unknown quantity" precedent, applied
/// here to Prime Part counts rather than build quantities).
pub fn get(owned: &OwnedPrimeParts, part: &PrimePart) -> Option<u32> {
    owned.get(&part.prime)?.get(&part.part).map(|c| c.value)
}

/// Apply one confirmed `(prime, part)` reading: sets the count and refreshes
/// its last-seen stamp. The Inventory/Sell screen never lists a 0-owned card
/// (see issue #37's region-calibration research), so unlike
/// [`crate::apply_confirmed_count`] there is no "value == 0 removes the
/// entry" case here.
pub fn apply_count(owned: &mut OwnedPrimeParts, part: &PrimePart, value: u32) {
    owned
        .entry(part.prime.clone())
        .or_default()
        .insert(part.part.clone(), OwnedCount { value, fetched_at: wf_cache::now_unix() });
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
        apply_count(&mut owned, &pp("Ember Prime", "Systems"), 3);
        assert_eq!(get(&owned, &pp("Ember Prime", "Systems")), Some(3));
        // A different part of the same prime is untouched.
        assert_eq!(get(&owned, &pp("Ember Prime", "Chassis")), None);
    }

    #[test]
    fn apply_count_overwrites_a_prior_reading() {
        let mut owned = OwnedPrimeParts::new();
        apply_count(&mut owned, &pp("Ember Prime", "Systems"), 3);
        apply_count(&mut owned, &pp("Ember Prime", "Systems"), 5);
        assert_eq!(get(&owned, &pp("Ember Prime", "Systems")), Some(5));
    }

    #[test]
    fn owned_prime_parts_roundtrips_through_json() {
        let mut owned = OwnedPrimeParts::new();
        apply_count(&mut owned, &pp("Ember Prime", "Systems"), 3);
        let json = serde_json::to_string(&owned).unwrap();
        let back: OwnedPrimeParts = serde_json::from_str(&json).unwrap();
        assert_eq!(get(&back, &pp("Ember Prime", "Systems")), Some(3));
    }
}
