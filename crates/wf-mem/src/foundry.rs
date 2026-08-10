//! Pure parsing of Foundry state (`PendingRecipes`/`Recipes`) out of the raw
//! `inventory.php` JSON body — network/screen-agnostic, following the same
//! convention as `wf-relic`'s parsing modules (cf. `wf-relic/src/relics.rs`).
//! Field names and shapes are per
//! `docs/research/mobile-inventory-api-coverage.md` (WFHelper's
//! `RawInventoryData`/`foundryResources.ts`/`foundryPending.ts`, read from
//! source) — DE's own inventory-sync field names, not this crate's invention.

use serde::Deserialize;
use time::OffsetDateTime;

/// One in-progress Foundry build, from `PendingRecipes[]`.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingBuild {
    /// DE's internal unique name for the blueprint being built, e.g.
    /// `/Lotus/Types/Recipes/Weapons/LatoPrimeBlueprint`.
    pub item_type: String,
    pub item_count: u32,
    /// `None` when `CompletionDate` was absent or didn't parse into a known shape.
    pub completion: Option<OffsetDateTime>,
}

/// One owned, uncrafted blueprint, from `Recipes[]`.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnedRecipe {
    pub item_type: String,
    pub item_count: u32,
}

/// Parsed Foundry state: builds in progress and blueprints on hand.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FoundryState {
    pub pending: Vec<PendingBuild>,
    pub recipes: Vec<OwnedRecipe>,
}

#[derive(Debug, Deserialize, Default)]
struct RawInventory {
    #[serde(default, rename = "PendingRecipes")]
    pending_recipes: Vec<RawEntry>,
    #[serde(default, rename = "Recipes")]
    recipes: Vec<RawEntry>,
}

#[derive(Debug, Deserialize, Default)]
struct RawEntry {
    #[serde(rename = "ItemType")]
    item_type: Option<String>,
    #[serde(rename = "ItemCount")]
    item_count: Option<i64>,
    #[serde(rename = "CompletionDate")]
    completion_date: Option<serde_json::Value>,
}

/// Parse Foundry state out of a raw `inventory.php` JSON response body.
/// Entries missing `ItemType` are skipped (mirrors WFHelper's own `if
/// (!recipe?.ItemType) continue`) rather than erroring the whole parse — a
/// single malformed entry shouldn't take down the rest of the payload. All
/// other top-level inventory fields (`Suits`, `RegularCredits`, etc.) are
/// ignored — this only ever looks at `PendingRecipes`/`Recipes`.
pub fn parse_foundry(raw_json: &str) -> anyhow::Result<FoundryState> {
    let raw: RawInventory = serde_json::from_str(raw_json)?;

    let pending = raw
        .pending_recipes
        .into_iter()
        .filter_map(|e| {
            let item_type = e.item_type?;
            Some(PendingBuild {
                item_type,
                item_count: item_count_or_default(e.item_count),
                completion: e.completion_date.as_ref().and_then(parse_completion_date),
            })
        })
        .collect();

    let recipes = raw
        .recipes
        .into_iter()
        .filter_map(|e| {
            let item_type = e.item_type?;
            Some(OwnedRecipe { item_type, item_count: item_count_or_default(e.item_count) })
        })
        .collect();

    Ok(FoundryState { pending, recipes })
}

fn item_count_or_default(count: Option<i64>) -> u32 {
    count.and_then(|c| u32::try_from(c).ok()).unwrap_or(1)
}

/// `CompletionDate` arrives in whichever of a few shapes DE's payload uses —
/// mirrors WFHelper's own `parseCompletionDate` (read from source, see the
/// research doc), which handles all of these: a bare epoch-millis number, a
/// numeric or RFC 3339 string, or MongoDB extended JSON (`{"$date": ...}`
/// wrapping either of the above, or a `{"$numberLong": "<ms>"}` object).
fn parse_completion_date(value: &serde_json::Value) -> Option<OffsetDateTime> {
    match value {
        serde_json::Value::Number(n) => n.as_i64().and_then(millis_to_datetime),
        serde_json::Value::String(s) => match s.parse::<i64>() {
            Ok(ms) => millis_to_datetime(ms),
            Err(_) => OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok(),
        },
        serde_json::Value::Object(obj) => obj
            .get("$date")
            .or_else(|| obj.get("$numberLong"))
            .and_then(parse_completion_date),
        _ => None,
    }
}

fn millis_to_datetime(ms: i64) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> String {
        fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/foundry_inventory.json"
        ))
        .expect("fixture reads")
    }

    #[test]
    fn parses_pending_and_owned_recipes_from_the_fixture() {
        let state = parse_foundry(&fixture()).expect("parses");

        assert_eq!(state.pending.len(), 2);
        assert_eq!(state.recipes.len(), 2);

        let lato_prime = &state.pending[0];
        assert_eq!(lato_prime.item_type, "/Lotus/Types/Recipes/Weapons/LatoPrimeBlueprint");
        assert_eq!(lato_prime.item_count, 1);
        assert_eq!(lato_prime.completion.unwrap().unix_timestamp(), 1_735_689_600);

        assert_eq!(state.recipes[0].item_count, 2);
        // No `ItemCount` on this fixture entry — defaults to 1.
        assert_eq!(state.recipes[1].item_count, 1);
    }

    #[test]
    fn skips_an_entry_missing_item_type() {
        let json = r#"{"PendingRecipes":[{"ItemCount":1}],"Recipes":[]}"#;
        let state = parse_foundry(json).unwrap();
        assert!(state.pending.is_empty());
    }

    #[test]
    fn ignores_unrelated_top_level_inventory_fields() {
        let json = r#"{"Suits":[{"ItemType":"/Lotus/Warframe"}],"RegularCredits":1000,
                         "PendingRecipes":[],"Recipes":[]}"#;
        let state = parse_foundry(json).unwrap();
        assert!(state.pending.is_empty() && state.recipes.is_empty());
    }

    #[test]
    fn parses_numberlong_wrapped_completion_date() {
        let json = r#"{"PendingRecipes":[{"ItemType":"/Lotus/Foo","ItemCount":1,
                         "CompletionDate":{"$date":{"$numberLong":"1735689600000"}}}],
                        "Recipes":[]}"#;
        let state = parse_foundry(json).unwrap();
        assert_eq!(state.pending[0].completion.unwrap().unix_timestamp(), 1_735_689_600);
    }

    #[test]
    fn parses_rfc3339_string_completion_date() {
        let json = r#"{"PendingRecipes":[{"ItemType":"/Lotus/Foo","ItemCount":1,
                         "CompletionDate":"2025-01-01T00:00:00Z"}],"Recipes":[]}"#;
        let state = parse_foundry(json).unwrap();
        assert_eq!(state.pending[0].completion.unwrap().unix_timestamp(), 1_735_689_600);
    }

    #[test]
    fn treats_an_unparseable_completion_date_as_absent_rather_than_erroring() {
        let json = r#"{"PendingRecipes":[{"ItemType":"/Lotus/Foo","CompletionDate":"not a date"}],
                        "Recipes":[]}"#;
        let state = parse_foundry(json).unwrap();
        assert!(state.pending[0].completion.is_none());
    }
}
