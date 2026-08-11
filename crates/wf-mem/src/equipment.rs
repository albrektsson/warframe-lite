//! Pure parsing of raw owned-equipment ownership out of the raw
//! `inventory.php` JSON body — network/screen-agnostic, following the same
//! convention as [`crate::foundry::parse_foundry`]. Field names are per
//! `docs/research/mobile-inventory-api-coverage.md`'s coverage of
//! `RawInventoryData` (`Suits`/`LongGuns`/`Pistols`/`Melee`/`Sentinels`/
//! `SentinelWeapons`/`SpaceSuits`/`SpaceGuns`/`SpaceMelee`/`OperatorAmps`/
//! `MechSuits`), the same `RawInventoryEntry` (`ItemType`/`ItemCount`) shape
//! `Recipes[]`/`LevelKeys[]` use.
//!
//! This is **raw ownership only** — an item's presence in one of these
//! arrays, not its `XP`/affinity. Per `docs/research/mem-scan-ownership-vs-
//! mastery.md` (issue #61), the existing `MasterySet` (sourced from DE's
//! public profile API's `XPInfo`) only sees items with `XP > 0` and has no
//! full-inventory fallback, so a freshly-built still-rank-0 item is invisible
//! to it end to end; this module exists to answer "do I own this at all,"
//! independent of that mastery/affinity pipeline. Re-surfacing per-item `XP`
//! here would just duplicate `MasterySet`'s job with a second, overlapping
//! source, so it's deliberately left out.

use serde::Deserialize;

/// Which owned-equipment array an [`OwnedItem`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquipmentCategory {
    Warframes,
    Primaries,
    Secondaries,
    Melee,
    Sentinels,
    SentinelWeapons,
    Archwings,
    ArchwingGuns,
    ArchwingMelee,
    OperatorAmps,
    Necramechs,
}

impl EquipmentCategory {
    /// Display order mirrors the `RawInventoryData` field order in
    /// `docs/research/mobile-inventory-api-coverage.md`.
    pub const ALL: [EquipmentCategory; 11] = [
        EquipmentCategory::Warframes,
        EquipmentCategory::Primaries,
        EquipmentCategory::Secondaries,
        EquipmentCategory::Melee,
        EquipmentCategory::Sentinels,
        EquipmentCategory::SentinelWeapons,
        EquipmentCategory::Archwings,
        EquipmentCategory::ArchwingGuns,
        EquipmentCategory::ArchwingMelee,
        EquipmentCategory::OperatorAmps,
        EquipmentCategory::Necramechs,
    ];

    pub fn label(self) -> &'static str {
        match self {
            EquipmentCategory::Warframes => "Warframes",
            EquipmentCategory::Primaries => "Primaries",
            EquipmentCategory::Secondaries => "Secondaries",
            EquipmentCategory::Melee => "Melee",
            EquipmentCategory::Sentinels => "Sentinels",
            EquipmentCategory::SentinelWeapons => "Sentinel Weapons",
            EquipmentCategory::Archwings => "Archwings",
            EquipmentCategory::ArchwingGuns => "Archwing Guns",
            EquipmentCategory::ArchwingMelee => "Archwing Melee",
            EquipmentCategory::OperatorAmps => "Operator Amps",
            EquipmentCategory::Necramechs => "Necramechs",
        }
    }
}

/// One owned equipment entry, raw.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnedItem {
    pub category: EquipmentCategory,
    /// DE's internal unique name for this item, e.g.
    /// `/Lotus/Powersuits/Excalibur/ExcaliburPrimeSuit`.
    pub item_type: String,
    pub item_count: u32,
}

/// Parsed owned-equipment state: every raw entry found across all eleven
/// equipment arrays, unprocessed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OwnedEquipment {
    pub items: Vec<OwnedItem>,
}

#[derive(Debug, Deserialize, Default)]
struct RawInventory {
    #[serde(default, rename = "Suits")]
    suits: Vec<RawEntry>,
    #[serde(default, rename = "LongGuns")]
    long_guns: Vec<RawEntry>,
    #[serde(default, rename = "Pistols")]
    pistols: Vec<RawEntry>,
    #[serde(default, rename = "Melee")]
    melee: Vec<RawEntry>,
    #[serde(default, rename = "Sentinels")]
    sentinels: Vec<RawEntry>,
    #[serde(default, rename = "SentinelWeapons")]
    sentinel_weapons: Vec<RawEntry>,
    #[serde(default, rename = "SpaceSuits")]
    space_suits: Vec<RawEntry>,
    #[serde(default, rename = "SpaceGuns")]
    space_guns: Vec<RawEntry>,
    #[serde(default, rename = "SpaceMelee")]
    space_melee: Vec<RawEntry>,
    #[serde(default, rename = "OperatorAmps")]
    operator_amps: Vec<RawEntry>,
    #[serde(default, rename = "MechSuits")]
    mech_suits: Vec<RawEntry>,
}

#[derive(Debug, Deserialize, Default)]
struct RawEntry {
    #[serde(rename = "ItemType")]
    item_type: Option<String>,
    #[serde(rename = "ItemCount")]
    item_count: Option<i64>,
}

/// Parse raw owned-equipment state out of a raw `inventory.php` JSON response
/// body. Entries missing `ItemType` are skipped (mirrors `parse_foundry`'s
/// convention) rather than erroring the whole parse. All other top-level
/// inventory fields (`Upgrades`, `LevelKeys`, `RegularCredits`, etc.) are
/// ignored — this only ever looks at the eleven equipment arrays.
pub fn parse_owned_equipment(raw_json: &str) -> anyhow::Result<OwnedEquipment> {
    let raw: RawInventory = serde_json::from_str(raw_json)?;

    let mut items = Vec::new();
    push_category(&mut items, EquipmentCategory::Warframes, raw.suits);
    push_category(&mut items, EquipmentCategory::Primaries, raw.long_guns);
    push_category(&mut items, EquipmentCategory::Secondaries, raw.pistols);
    push_category(&mut items, EquipmentCategory::Melee, raw.melee);
    push_category(&mut items, EquipmentCategory::Sentinels, raw.sentinels);
    push_category(&mut items, EquipmentCategory::SentinelWeapons, raw.sentinel_weapons);
    push_category(&mut items, EquipmentCategory::Archwings, raw.space_suits);
    push_category(&mut items, EquipmentCategory::ArchwingGuns, raw.space_guns);
    push_category(&mut items, EquipmentCategory::ArchwingMelee, raw.space_melee);
    push_category(&mut items, EquipmentCategory::OperatorAmps, raw.operator_amps);
    push_category(&mut items, EquipmentCategory::Necramechs, raw.mech_suits);

    Ok(OwnedEquipment { items })
}

fn push_category(items: &mut Vec<OwnedItem>, category: EquipmentCategory, raw: Vec<RawEntry>) {
    items.extend(raw.into_iter().filter_map(|e| {
        let item_type = e.item_type?;
        Some(OwnedItem { category, item_type, item_count: item_count_or_default(e.item_count) })
    }));
}

fn item_count_or_default(count: Option<i64>) -> u32 {
    count.and_then(|c| u32::try_from(c).ok()).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> String {
        fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/equipment_inventory.json"
        ))
        .expect("fixture reads")
    }

    #[test]
    fn parses_owned_equipment_from_the_fixture() {
        let state = parse_owned_equipment(&fixture()).expect("parses");

        assert_eq!(state.items.len(), 5);

        let warframe = state
            .items
            .iter()
            .find(|i| i.category == EquipmentCategory::Warframes)
            .expect("a warframe entry");
        assert_eq!(warframe.item_type, "/Lotus/Powersuits/Excalibur/ExcaliburPrimeSuit");
        assert_eq!(warframe.item_count, 1);

        let necramech = state
            .items
            .iter()
            .find(|i| i.category == EquipmentCategory::Necramechs)
            .expect("a necramech entry");
        assert_eq!(necramech.item_type, "/Lotus/Powersuits/Bonewidow/BonewidowSuit");

        // No `ItemCount` on this fixture entry — defaults to 1.
        let amp = state
            .items
            .iter()
            .find(|i| i.category == EquipmentCategory::OperatorAmps)
            .expect("an operator amp entry");
        assert_eq!(amp.item_count, 1);
    }

    #[test]
    fn skips_an_entry_missing_item_type() {
        let json = r#"{"Suits":[{"ItemCount":1}]}"#;
        let state = parse_owned_equipment(json).unwrap();
        assert!(state.items.is_empty());
    }

    #[test]
    fn ignores_unrelated_top_level_inventory_fields() {
        let json = r#"{"LevelKeys":[{"ItemType":"/Lotus/Types/Game/Projections/MesoA1RelicItem"}],
                        "Upgrades":[{"ItemType":"/Lotus/Upgrades/Mods/Warframe/VitalityMod"}]}"#;
        let state = parse_owned_equipment(json).unwrap();
        assert!(state.items.is_empty());
    }

    #[test]
    fn does_not_surface_per_item_xp() {
        // `parse_owned_equipment` only extracts ItemType/ItemCount — an `XP`
        // field on an entry (present in real payloads, per the module doc)
        // must not surface anywhere on `OwnedItem`.
        let json = r#"{"Suits":[{"ItemType":"/Lotus/Powersuits/Volt/VoltSuit","XP":50000}]}"#;
        let state = parse_owned_equipment(json).unwrap();
        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].item_type, "/Lotus/Powersuits/Volt/VoltSuit");
    }
}
