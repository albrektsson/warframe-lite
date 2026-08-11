//! Pure parsing of raw owned-relic ownership out of the raw `inventory.php`
//! JSON body — network/screen-agnostic, following the same convention as
//! [`crate::level_keys::parse_level_keys`]. Field names are per
//! `docs/research/mobile-inventory-api-coverage.md` §6 (issue #63,
//! live-verified): owned relics are **not** `LevelKeys[]` (that field holds
//! only legacy mission Keys — issue #60) but `MiscItems[]` entries whose
//! `ItemType` matches `/Lotus/Types/Game/Projections/T{1-5}VoidProjection
//! {RewardPoolName}{Letter}{Refinement}` — DE's internal name for a relic is
//! "VoidProjection", not "Relic" or "LevelKey" — using the same
//! `RawInventoryEntry` (`ItemType`/`ItemCount`) shape `Recipes[]`/
//! `LevelKeys[]` use.
//!
//! This is **raw exposure only**: `MiscItems[]` also carries Endo, Ducats,
//! Aya, Vitus, Forma, and other resources, so entries are filtered down to
//! ones whose `ItemType` matches the `VoidProjection` naming pattern — but
//! that name is not decoded any further. Splitting out tier/reward-pool/
//! letter/refinement, and cross-referencing against WFCD's `warframe-items`
//! relic catalogue (per ADR-0011) to resolve player-facing relic codes (e.g.
//! "Lith V1"), is explicit out-of-scope for issue #64, left for a follow-up
//! ticket. Likewise no dedup or cross-check against the existing OCR-based
//! Seen/Confirmed relic pipeline (ADR-0009) — that pipeline is untouched.

use serde::Deserialize;

/// One owned-relic `MiscItems[]` entry, raw.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnedRelic {
    /// DE's internal unique name for this relic entry, e.g.
    /// `/Lotus/Types/Game/Projections/T3VoidProjectionAtlasPrimeCBronze`.
    pub item_type: String,
    pub item_count: u32,
}

/// Parsed owned-relic state: every `MiscItems[]` entry matching the
/// `VoidProjection` naming pattern, unprocessed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OwnedRelicState {
    pub relics: Vec<OwnedRelic>,
}

#[derive(Debug, Deserialize, Default)]
struct RawInventory {
    #[serde(default, rename = "MiscItems")]
    misc_items: Vec<RawEntry>,
}

#[derive(Debug, Deserialize, Default)]
struct RawEntry {
    #[serde(rename = "ItemType")]
    item_type: Option<String>,
    #[serde(rename = "ItemCount")]
    item_count: Option<i64>,
}

/// Parse raw owned-relic state out of a raw `inventory.php` JSON response
/// body. Entries missing `ItemType` are skipped (mirrors `parse_foundry`'s
/// convention) rather than erroring the whole parse; `MiscItems[]` entries
/// whose `ItemType` doesn't match the `VoidProjection` naming pattern (Endo,
/// Ducats, Aya, Forma, etc.) are dropped, not surfaced. All other top-level
/// inventory fields (`LevelKeys`, `Recipes`, `RegularCredits`, etc.) are
/// ignored — this only ever looks at `MiscItems`.
pub fn parse_owned_relics(raw_json: &str) -> anyhow::Result<OwnedRelicState> {
    let raw: RawInventory = serde_json::from_str(raw_json)?;

    let relics = raw
        .misc_items
        .into_iter()
        .filter_map(|e| {
            let item_type = e.item_type?;
            is_void_projection(&item_type)
                .then(|| OwnedRelic { item_type, item_count: item_count_or_default(e.item_count) })
        })
        .collect();

    Ok(OwnedRelicState { relics })
}

fn item_count_or_default(count: Option<i64>) -> u32 {
    count.and_then(|c| u32::try_from(c).ok()).unwrap_or(1)
}

/// Whether `item_type` matches `/Lotus/Types/Game/Projections/T{1-5}
/// VoidProjection...` — the naming pattern issue #63 live-verified for owned
/// relics (`{RewardPoolName}{Letter}{Refinement}` following `VoidProjection`
/// is deliberately not parsed further here, see module doc).
fn is_void_projection(item_type: &str) -> bool {
    let Some(rest) = item_type.strip_prefix("/Lotus/Types/Game/Projections/T") else {
        return false;
    };
    let mut chars = rest.chars();
    match chars.next() {
        Some(tier) if tier.is_ascii_digit() && ('1'..='5').contains(&tier) => {}
        _ => return false,
    }
    chars.as_str().starts_with("VoidProjection")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> String {
        fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/relics_inventory.json"))
            .expect("fixture reads")
    }

    #[test]
    fn parses_owned_relics_from_the_fixture() {
        let state = parse_owned_relics(&fixture()).expect("parses");

        assert_eq!(state.relics.len(), 3);

        assert_eq!(
            state.relics[0].item_type,
            "/Lotus/Types/Game/Projections/T3VoidProjectionAtlasPrimeCBronze"
        );
        assert_eq!(state.relics[0].item_count, 52);

        assert_eq!(
            state.relics[1].item_type,
            "/Lotus/Types/Game/Projections/T1VoidProjectionIvaraPrimeABronze"
        );
        assert_eq!(state.relics[1].item_count, 18);

        // No `ItemCount` on this fixture entry — defaults to 1.
        assert_eq!(state.relics[2].item_type, "/Lotus/Types/Game/Projections/T5VoidProjectionImmortalABronze");
        assert_eq!(state.relics[2].item_count, 1);
    }

    #[test]
    fn drops_non_relic_misc_items() {
        // Endo, Ducats, Forma — real `MiscItems[]` residents that aren't relics.
        let json = r#"{"MiscItems":[
            {"ItemType":"/Lotus/Types/Items/MiscItems/EnduringEndo","ItemCount":50000},
            {"ItemType":"/Lotus/Types/Items/MiscItems/PrimeBucks","ItemCount":900},
            {"ItemType":"/Lotus/Types/Recipes/Components/FormaBlueprint","ItemCount":3}
        ]}"#;
        let state = parse_owned_relics(json).unwrap();
        assert!(state.relics.is_empty());
    }

    #[test]
    fn skips_an_entry_missing_item_type() {
        let json = r#"{"MiscItems":[{"ItemCount":1}]}"#;
        let state = parse_owned_relics(json).unwrap();
        assert!(state.relics.is_empty());
    }

    #[test]
    fn ignores_unrelated_top_level_inventory_fields() {
        let json = r#"{"LevelKeys":[{"ItemType":"/Lotus/Types/Game/Projections/MesoA1RelicItem"}],
                        "Recipes":[{"ItemType":"/Lotus/Types/Recipes/Weapons/LatoPrimeBlueprint"}]}"#;
        let state = parse_owned_relics(json).unwrap();
        assert!(state.relics.is_empty());
    }

    #[test]
    fn rejects_a_tier_number_out_of_the_observed_one_to_five_range() {
        let json = r#"{"MiscItems":[
            {"ItemType":"/Lotus/Types/Game/Projections/T6VoidProjectionFooABronze","ItemCount":1},
            {"ItemType":"/Lotus/Types/Game/Projections/T0VoidProjectionFooABronze","ItemCount":1}
        ]}"#;
        let state = parse_owned_relics(json).unwrap();
        assert!(state.relics.is_empty());
    }

    #[test]
    fn requires_the_exact_void_projection_prefix_shape() {
        // Similar-looking but not the real pattern: no tier digit, or a
        // suffix that only starts with something else after "T{n}".
        let json = r#"{"MiscItems":[
            {"ItemType":"/Lotus/Types/Game/Projections/VoidProjectionFooABronze","ItemCount":1},
            {"ItemType":"/Lotus/Types/Game/Projections/T3SomethingElseFooABronze","ItemCount":1}
        ]}"#;
        let state = parse_owned_relics(json).unwrap();
        assert!(state.relics.is_empty());
    }
}
