//! Pure parsing of raw `LevelKeys[]` (Relics) out of the raw `inventory.php`
//! JSON body — network/screen-agnostic, following the same convention as
//! [`crate::foundry::parse_foundry`]. Field names are per
//! `docs/research/mobile-inventory-api-coverage.md`'s coverage of
//! `RawInventoryEntry` (`ItemType`/`ItemCount`), the same shape `Recipes[]`
//! uses.
//!
//! This only extracts the raw array — no refinement decoding, no dedup or
//! cross-check against the existing OCR-based Seen/Confirmed relic pipeline
//! (ADR-0009). Replacing that pipeline with this data is an explicit
//! out-of-scope call on this crate's map (issue #55): the research doc flags
//! `LevelKeys[]` as relic-ownership data, but `ItemType`'s exact encoding of
//! relic tier/refinement wasn't pinned down by that research, so this stays
//! raw rather than guessing a shape to interpret.

use serde::Deserialize;

/// One `LevelKeys[]` entry, raw.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelKey {
    /// DE's internal unique name for this relic entry, e.g.
    /// `/Lotus/Types/Game/Projections/MesoA1RelicItem`.
    pub item_type: String,
    pub item_count: u32,
}

/// Parsed `LevelKeys[]` state: every raw entry found, unprocessed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LevelKeyState {
    pub level_keys: Vec<LevelKey>,
}

#[derive(Debug, Deserialize, Default)]
struct RawInventory {
    #[serde(default, rename = "LevelKeys")]
    level_keys: Vec<RawEntry>,
}

#[derive(Debug, Deserialize, Default)]
struct RawEntry {
    #[serde(rename = "ItemType")]
    item_type: Option<String>,
    #[serde(rename = "ItemCount")]
    item_count: Option<i64>,
}

/// Parse raw `LevelKeys[]` state out of a raw `inventory.php` JSON response
/// body. Entries missing `ItemType` are skipped (mirrors `parse_foundry`'s
/// convention) rather than erroring the whole parse. All other top-level
/// inventory fields are ignored — this only ever looks at `LevelKeys`.
pub fn parse_level_keys(raw_json: &str) -> anyhow::Result<LevelKeyState> {
    let raw: RawInventory = serde_json::from_str(raw_json)?;

    let level_keys = raw
        .level_keys
        .into_iter()
        .filter_map(|e| {
            let item_type = e.item_type?;
            Some(LevelKey { item_type, item_count: item_count_or_default(e.item_count) })
        })
        .collect();

    Ok(LevelKeyState { level_keys })
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
            "/tests/fixtures/level_keys_inventory.json"
        ))
        .expect("fixture reads")
    }

    #[test]
    fn parses_level_keys_from_the_fixture() {
        let state = parse_level_keys(&fixture()).expect("parses");

        assert_eq!(state.level_keys.len(), 3);

        assert_eq!(
            state.level_keys[0].item_type,
            "/Lotus/Types/Game/Projections/MesoA1RelicItem"
        );
        assert_eq!(state.level_keys[0].item_count, 4);

        assert_eq!(
            state.level_keys[1].item_type,
            "/Lotus/Types/Game/Projections/AxiN13RelicItem"
        );
        assert_eq!(state.level_keys[1].item_count, 1);

        // No `ItemCount` on this fixture entry — defaults to 1.
        assert_eq!(
            state.level_keys[2].item_type,
            "/Lotus/Types/Game/Projections/LithV1RelicItem"
        );
        assert_eq!(state.level_keys[2].item_count, 1);
    }

    #[test]
    fn skips_an_entry_missing_item_type() {
        let json = r#"{"LevelKeys":[{"ItemCount":1}]}"#;
        let state = parse_level_keys(json).unwrap();
        assert!(state.level_keys.is_empty());
    }

    #[test]
    fn ignores_unrelated_top_level_inventory_fields() {
        let json = r#"{"Suits":[{"ItemType":"/Lotus/Warframe"}],"LevelKeys":[]}"#;
        let state = parse_level_keys(json).unwrap();
        assert!(state.level_keys.is_empty());
    }
}
