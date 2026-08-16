//! The owned-riven set: what `wf-mem`'s mem-scan persists to `rivens.json`
//! and `wf-browse`'s Rivens tab reads.
//!
//! Unlike [`crate::OwnedPrimeParts`]/[`crate::OwnedRelics`], there is no
//! OCR-scanner writer to reconcile against and no Seen/Confirmed trust
//! tiers to track (ADR-0009's concern doesn't apply — see
//! `crates/wf-mem/src/riven.rs`'s module doc and
//! `docs/specs/riven-browse-tab.md` §6: rivens are read-only, mem-scan is
//! the only source). `parse_rivens` already returns every real riven
//! currently in `Upgrades[]` in one pass, so there's nothing to merge —
//! each mem-scan's decoded result *is* the new ground truth, wholesale.

use crate::riven_decode::DecodedRiven;

/// The persisted owned-riven set: every decoded Unveiled riven from the most
/// recent mem-scan. No stable per-riven id exists in the raw mobile
/// inventory payload (`crates/wf-mem/src/riven.rs::Riven` doesn't carry
/// one), so entries aren't keyed — a full list, replaced wholesale on every
/// scan (see [`apply_exact_snapshot`]).
pub type OwnedRivens = Vec<DecodedRiven>;

/// The file `rivens.json` is cached under, via `wf_cache::load_blob`/
/// `save_blob` — written by `wf-mem`'s mem-scan persist path
/// (`write_owned_rivens`), read by `wf-browse`'s Rivens tab.
pub const OWNED_RIVENS_FILE: &str = "rivens.json";

/// Replace the owned-riven set with a fresh mem-scan reading wholesale — no
/// prior entry survives untouched, since (unlike Prime Parts, which also
/// gets narrower per-card OCR updates) a riven mem-scan always covers every
/// currently-owned Unveiled riven in one pass. See the module doc.
pub fn apply_exact_snapshot(owned: &mut OwnedRivens, snapshot: Vec<DecodedRiven>) {
    *owned = snapshot;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::riven_catalogue::RivenModCategory;

    fn riven(weapon_name: &str) -> DecodedRiven {
        DecodedRiven {
            weapon_name: weapon_name.to_string(),
            weapon_unique_name: format!("/Lotus/Weapons/{weapon_name}"),
            mod_category: RivenModCategory::Rifle,
            polarity: None,
            mastery_req: None,
            rank: 8,
            rerolls: 0,
            stats: Vec::new(),
        }
    }

    #[test]
    fn apply_exact_snapshot_replaces_the_whole_set() {
        let mut owned: OwnedRivens = vec![riven("Old Riven")];
        apply_exact_snapshot(&mut owned, vec![riven("New Riven A"), riven("New Riven B")]);
        assert_eq!(owned.len(), 2);
        assert!(!owned.iter().any(|r| r.weapon_name == "Old Riven"));
    }

    #[test]
    fn apply_exact_snapshot_of_an_empty_scan_clears_the_set() {
        let mut owned: OwnedRivens = vec![riven("Old Riven")];
        apply_exact_snapshot(&mut owned, Vec::new());
        assert!(owned.is_empty());
    }

    #[test]
    fn owned_rivens_roundtrips_through_json() {
        let owned: OwnedRivens = vec![riven("Soma Prime")];
        let json = serde_json::to_string(&owned).unwrap();
        let back: OwnedRivens = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].weapon_name, "Soma Prime");
    }
}
