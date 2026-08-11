//! Pure parsing of raw owned built-component ("Prime Part") ownership out of
//! the raw `inventory.php` JSON body — network/screen-agnostic, following
//! the same convention as [`crate::relics::parse_owned_relics`]. Live-verified
//! on issue #79: a built-but-unassembled Warframe/weapon/Sentinel/Archwing
//! component is a `MiscItems[]` entry (same `ItemType`/`ItemCount` shape as
//! `Recipes[]`/owned relics), under two distinct naming patterns:
//!
//! - Warframe parts: `/Lotus/Types/Recipes/WarframeRecipes/{FrameName}{Part}
//!   Component` — DE's internal label for the head component is `Helmet`,
//!   not `Neuroptics`. Not Prime-specific: the same shape covers non-Prime
//!   frames too (`AshChassisComponent`, `HydroidHelmetComponent`, both
//!   observed live).
//! - Weapon/Sentinel/Archwing parts: `/Lotus/Types/Recipes/Weapons/
//!   WeaponParts/{WeaponName}{Part}` — no `Component`/`Blueprint` suffix.
//!   `Part` is weapon-type-dependent (`Barrel`/`Receiver`/`Stock`/`Blade`/
//!   `Grip`/`Link`/`Guard`/`Chassis`/`Handle`/`Hilt`/`Disc`/`Chain` observed
//!   live) and the set isn't exhaustively enumerable — #79 also observed
//!   `ShadePrimeSystems` (Sentinels use `Systems` under this weapon
//!   namespace too, distinct from the Warframe pattern's own `Systems`).
//!   Matching this pattern by prefix alone (rather than an enumerated part
//!   suffix list, which #79 already caught being incomplete) avoids
//!   silently dropping genuinely-owned entries.
//!
//! This is **raw exposure only**, mirroring #64's `parse_owned_relics`
//! pattern: `item_type` is not split into a `(FrameOrWeapon, Part)` pair, the
//! `Helmet -> Neuroptics` rename isn't applied, and no cross-reference
//! against WFCD's `warframe-items` catalogue (ADR-0011) filters this down to
//! Primes specifically — a raw `AshChassisComponent` entry surfaces here the
//! same as a Prime's. All of that is explicit out-of-scope on issue #80,
//! left for a follow-up ticket.

use serde::Deserialize;

/// One owned-part `MiscItems[]` entry, raw.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnedPartRaw {
    /// DE's internal unique name for this component entry, e.g.
    /// `/Lotus/Types/Recipes/WarframeRecipes/VorunaPrimeSystemsComponent` or
    /// `/Lotus/Types/Recipes/Weapons/WeaponParts/RubicoPrimeReceiver`.
    pub item_type: String,
    pub item_count: u32,
}

/// Parsed owned-part state: every `MiscItems[]` entry matching either
/// built-component naming pattern, unprocessed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OwnedPartsState {
    pub parts: Vec<OwnedPartRaw>,
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

const WARFRAME_PART_PREFIX: &str = "/Lotus/Types/Recipes/WarframeRecipes/";
const WEAPON_PART_PREFIX: &str = "/Lotus/Types/Recipes/Weapons/WeaponParts/";

/// Parse raw owned-part state out of a raw `inventory.php` JSON response
/// body. Entries missing `ItemType` are skipped (mirrors `parse_foundry`'s
/// convention) rather than erroring the whole parse; `MiscItems[]` entries
/// matching neither built-component naming pattern (Endo, Ducats, relics,
/// Forma, etc.) are dropped, not surfaced. All other top-level inventory
/// fields (`Recipes`, `LevelKeys`, `RegularCredits`, etc.) are ignored —
/// this only ever looks at `MiscItems`.
pub fn parse_owned_parts(raw_json: &str) -> anyhow::Result<OwnedPartsState> {
    let raw: RawInventory = serde_json::from_str(raw_json)?;

    let parts = raw
        .misc_items
        .into_iter()
        .filter_map(|e| {
            let item_type = e.item_type?;
            is_owned_part(&item_type)
                .then(|| OwnedPartRaw { item_type, item_count: item_count_or_default(e.item_count) })
        })
        .collect();

    Ok(OwnedPartsState { parts })
}

fn item_count_or_default(count: Option<i64>) -> u32 {
    count.and_then(|c| u32::try_from(c).ok()).unwrap_or(1)
}

fn is_owned_part(item_type: &str) -> bool {
    is_warframe_part_component(item_type) || is_weapon_part(item_type)
}

/// `/Lotus/Types/Recipes/WarframeRecipes/{FrameName}{Part}Component` — the
/// `Component` suffix is the structural marker (see module doc for why the
/// `Part` label itself isn't enumerated).
fn is_warframe_part_component(item_type: &str) -> bool {
    item_type
        .strip_prefix(WARFRAME_PART_PREFIX)
        .and_then(|rest| rest.strip_suffix("Component"))
        .is_some_and(|mid| !mid.is_empty())
}

/// `/Lotus/Types/Recipes/Weapons/WeaponParts/{WeaponName}{Part}` — matched by
/// prefix alone (see module doc for why an enumerated `Part` suffix list
/// isn't used).
fn is_weapon_part(item_type: &str) -> bool {
    item_type.strip_prefix(WEAPON_PART_PREFIX).is_some_and(|rest| !rest.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> String {
        fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/owned_parts_inventory.json"
        ))
        .expect("fixture reads")
    }

    #[test]
    fn parses_owned_parts_from_the_fixture() {
        let state = parse_owned_parts(&fixture()).expect("parses");

        assert_eq!(state.parts.len(), 6);

        assert_eq!(
            state.parts[0].item_type,
            "/Lotus/Types/Recipes/WarframeRecipes/VorunaPrimeSystemsComponent"
        );
        assert_eq!(state.parts[0].item_count, 1);

        assert_eq!(
            state.parts[1].item_type,
            "/Lotus/Types/Recipes/WarframeRecipes/VorunaPrimeHelmetComponent"
        );
        assert_eq!(state.parts[1].item_count, 1);

        // Not Prime-specific — a non-Prime frame's component still surfaces.
        assert_eq!(
            state.parts[2].item_type,
            "/Lotus/Types/Recipes/WarframeRecipes/AshChassisComponent"
        );
        // No `ItemCount` on this fixture entry — defaults to 1.
        assert_eq!(state.parts[2].item_count, 1);

        assert_eq!(
            state.parts[3].item_type,
            "/Lotus/Types/Recipes/Weapons/WeaponParts/RubicoPrimeReceiver"
        );
        assert_eq!(state.parts[3].item_count, 1);

        assert_eq!(
            state.parts[4].item_type,
            "/Lotus/Types/Recipes/Weapons/WeaponParts/GorgonWraithBarrel"
        );
        assert_eq!(state.parts[4].item_count, 9);

        // Sentinels use `Systems` under the weapon namespace too (#79).
        assert_eq!(
            state.parts[5].item_type,
            "/Lotus/Types/Recipes/Weapons/WeaponParts/ShadePrimeSystems"
        );
        assert_eq!(state.parts[5].item_count, 2);
    }

    #[test]
    fn drops_non_part_misc_items() {
        // Endo, Ducats, a relic — real `MiscItems[]` residents that aren't
        // built-component ownership.
        let json = r#"{"MiscItems":[
            {"ItemType":"/Lotus/Types/Items/MiscItems/EnduringEndo","ItemCount":50000},
            {"ItemType":"/Lotus/Types/Items/MiscItems/PrimeBucks","ItemCount":900},
            {"ItemType":"/Lotus/Types/Game/Projections/T3VoidProjectionAtlasPrimeCBronze","ItemCount":1}
        ]}"#;
        let state = parse_owned_parts(json).unwrap();
        assert!(state.parts.is_empty());
    }

    #[test]
    fn skips_an_entry_missing_item_type() {
        let json = r#"{"MiscItems":[{"ItemCount":1}]}"#;
        let state = parse_owned_parts(json).unwrap();
        assert!(state.parts.is_empty());
    }

    #[test]
    fn ignores_unrelated_top_level_inventory_fields() {
        let json = r#"{"Recipes":[{"ItemType":"/Lotus/Types/Recipes/Weapons/LatoPrimeBlueprint"}],
                        "LevelKeys":[{"ItemType":"/Lotus/Types/Game/Projections/MesoA1RelicItem"}]}"#;
        let state = parse_owned_parts(json).unwrap();
        assert!(state.parts.is_empty());
    }

    #[test]
    fn rejects_a_component_suffix_with_no_frame_or_part_before_it() {
        let json = r#"{"MiscItems":[
            {"ItemType":"/Lotus/Types/Recipes/WarframeRecipes/Component","ItemCount":1}
        ]}"#;
        let state = parse_owned_parts(json).unwrap();
        assert!(state.parts.is_empty());
    }

    #[test]
    fn rejects_the_bare_weapon_part_prefix_with_nothing_after_it() {
        let json = r#"{"MiscItems":[
            {"ItemType":"/Lotus/Types/Recipes/Weapons/WeaponParts/","ItemCount":1}
        ]}"#;
        let state = parse_owned_parts(json).unwrap();
        assert!(state.parts.is_empty());
    }

    #[test]
    fn a_weapon_part_blueprint_suffix_still_matches_by_prefix_alone() {
        // Deliberately documents the prefix-only tradeoff (see module doc):
        // this crate only ever reads `MiscItems[]` here, and #79 found no
        // `Blueprint`-suffixed entries under this prefix live — but the
        // parser doesn't special-case a suffix it hasn't observed.
        let json = r#"{"MiscItems":[
            {"ItemType":"/Lotus/Types/Recipes/Weapons/WeaponParts/RubicoPrimeReceiverBlueprint","ItemCount":1}
        ]}"#;
        let state = parse_owned_parts(json).unwrap();
        assert_eq!(state.parts.len(), 1);
    }
}
