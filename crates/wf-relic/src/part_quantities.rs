//! Per-Prime-Part build quantities (e.g. Afuris Prime needs 2 Barrel) from
//! WFCD `warframe-items` — a second, separate WFCD dataset from
//! `warframe-drop-data` (see ADR-0011): the relic drop tables carry reward
//! names and rarities, but not how many of each a build recipe needs.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::mastery::PrimePart;

const BASE: &str = "https://raw.githubusercontent.com/WFCD/warframe-items/master/data/json";
// `-v3`: non-tradable resource components (Orokin Cell and the like) are no
// longer fetched into entries at all — bump so a stale v2 cache, which still
// has those bogus rows baked in, isn't served for up to a week before this
// fix takes effect.
const CACHE_FILE: &str = "part-quantities-v3.json";

/// Per-category files that carry Prime items today. Fetched individually
/// rather than the much larger combined `All.json` (~54.5MB vs a few MB per
/// category — see ADR-0011). `Pets` and `Arch-Melee` were added for the
/// Equipment category-tree view (issue #42's research): `Pets` is the only
/// source of live Kubrow/Kavat/Moa companions, absent from every other
/// dataset this app fetches.
const CATEGORIES: &[&str] = &[
    "Warframes",
    "Primary",
    "Secondary",
    "Melee",
    "Sentinels",
    "SentinelWeapons",
    "Archwing",
    "Arch-Gun",
    "Arch-Melee",
    "Pets",
];

/// WFinfo's own Equipment-window grouping for a built Prime (issue #42's
/// research). Keyed off which WFCD `warframe-items` source file a Prime was
/// parsed from ([`category_for_file`]), not any in-record field — warframe.market's
/// tags can't cleanly distinguish Companion sub-cases, and `SentinelWeapons.json`
/// mislabels its own in-record category. `Other` is the fallback for a Prime
/// WFCD hasn't filed under any of the six buckets (Deimos modular companions,
/// Necramechs) — never guessed into the wrong bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EquipmentCategory {
    Warframe,
    Primary,
    Secondary,
    Melee,
    Archwing,
    Companion,
    Other,
}

impl EquipmentCategory {
    /// Display label, matching WFinfo's own Equipment window headings.
    pub fn label(self) -> &'static str {
        match self {
            Self::Warframe => "Warframe",
            Self::Primary => "Primary",
            Self::Secondary => "Secondary",
            Self::Melee => "Melee",
            Self::Archwing => "Archwing",
            Self::Companion => "Companion",
            Self::Other => "Other",
        }
    }
}

/// Fixed display order for the Equipment tree's category level — WFinfo's own
/// order, unaffected by the Mastery tab's Alphabetical/Unmastered-first sort
/// (which applies *within* a category instead, per the design ticket).
/// `Other` trails every WFinfo category since it's a fallback bucket WFinfo
/// itself has no equivalent for.
pub const CATEGORY_ORDER: [EquipmentCategory; 7] = [
    EquipmentCategory::Warframe,
    EquipmentCategory::Primary,
    EquipmentCategory::Secondary,
    EquipmentCategory::Melee,
    EquipmentCategory::Archwing,
    EquipmentCategory::Companion,
    EquipmentCategory::Other,
];

/// Which [`EquipmentCategory`] a WFCD `warframe-items` source file's Primes
/// belong to. `Archwing`/`Arch-Gun`/`Arch-Melee` collapse into one Archwing
/// bucket, `Sentinels`/`SentinelWeapons`/`Pets` into one Companion bucket —
/// both confirmed against the reference WFinfo screenshot, which shows no
/// sub-grouping within either.
fn category_for_file(file: &str) -> EquipmentCategory {
    match file {
        "Warframes" => EquipmentCategory::Warframe,
        "Primary" => EquipmentCategory::Primary,
        "Secondary" => EquipmentCategory::Secondary,
        "Melee" => EquipmentCategory::Melee,
        "Archwing" | "Arch-Gun" | "Arch-Melee" => EquipmentCategory::Archwing,
        "Sentinels" | "SentinelWeapons" | "Pets" => EquipmentCategory::Companion,
        _ => EquipmentCategory::Other,
    }
}

/// One resolved (Built prime, Prime Part) build quantity, the cached/testable
/// unit `PartQuantities` is built from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    prime: String,
    part: String,
    quantity: u32,
    category: EquipmentCategory,
}

/// Build-quantity lookup for Prime Parts, plus each Prime's WFinfo-style
/// equipment category. An absent quantity entry means "unknown", never "1" —
/// a missing quantity is shown as unknown rather than guessed (see
/// ADR-0011).
pub struct PartQuantities {
    map: HashMap<(String, String), u32>,
    categories: HashMap<String, EquipmentCategory>,
}

impl PartQuantities {
    fn new(entries: Vec<Entry>) -> Self {
        let categories = entries.iter().map(|e| (e.prime.clone(), e.category)).collect();
        let map = entries.into_iter().map(|e| ((e.prime, e.part), e.quantity)).collect();
        Self { map, categories }
    }

    /// An empty lookup — every [`Self::get`] call returns `None`. For tests
    /// and callers with no quantity data available yet.
    pub fn empty() -> Self {
        Self { map: HashMap::new(), categories: HashMap::new() }
    }

    /// Build a lookup directly from `(prime, part, quantity)` triples, for
    /// tests elsewhere in the crate that need known quantities without going
    /// through a fetch. Every entry gets [`EquipmentCategory::Other`] — use
    /// [`Self::from_entries_with_category_for_test`] when the test cares
    /// about category grouping specifically.
    pub fn from_entries_for_test(entries: Vec<(String, String, u32)>) -> Self {
        Self::new(
            entries
                .into_iter()
                .map(|(prime, part, quantity)| Entry {
                    prime,
                    part,
                    quantity,
                    category: EquipmentCategory::Other,
                })
                .collect(),
        )
    }

    /// Like [`Self::from_entries_for_test`], but with an explicit category per
    /// entry — for tests exercising category grouping.
    pub fn from_entries_with_category_for_test(
        entries: Vec<(String, String, u32, EquipmentCategory)>,
    ) -> Self {
        Self::new(
            entries
                .into_iter()
                .map(|(prime, part, quantity, category)| Entry { prime, part, quantity, category })
                .collect(),
        )
    }

    /// How many of `part` a full build requires, if known.
    pub fn get(&self, part: &PrimePart) -> Option<u32> {
        self.map.get(&(part.prime.clone(), part.part.clone())).copied()
    }

    /// Every known `(part label, build quantity)` pair for `prime` — the full
    /// component list independent of any relic drop table (see the module
    /// doc), the basis for a full-BOM view. Empty when `prime` isn't in the
    /// catalogue.
    pub fn parts_for(&self, prime: &str) -> Vec<(String, u32)> {
        self.map
            .iter()
            .filter(|((p, _), _)| p == prime)
            .map(|((_, part), &quantity)| (part.clone(), quantity))
            .collect()
    }

    /// Every distinct Prime name in this catalogue, sorted alphabetically.
    pub fn primes(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.map.keys().map(|(p, _)| p.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Whether `prime` is a known Prime item name in this catalogue — the
    /// authoritative check [`crate::mastery::inventory_prime_part`] uses to
    /// accept or reject an OCR'd Inventory/Sell card label, replacing a raw
    /// `"Prime"` substring heuristic (see issue #37's catalog-matching
    /// decision).
    pub fn has_prime(&self, prime: &str) -> bool {
        self.map.keys().any(|(p, _)| p == prime)
    }

    /// The WFinfo-style equipment category `prime` was filed under, or
    /// [`EquipmentCategory::Other`] if `prime` isn't in this catalogue at all
    /// (never guessed into a real category — see ADR-0011's "unknown never
    /// guessed" precedent, applied here to category instead of quantity).
    pub fn category_for(&self, prime: &str) -> EquipmentCategory {
        self.categories.get(prime).copied().unwrap_or(EquipmentCategory::Other)
    }

    /// Fetch + cache (weekly TTL, stale-served on failure), mirroring
    /// [`crate::RelicIndex::load_cached`].
    pub async fn load_cached(client: &reqwest::Client, ttl: Duration) -> anyhow::Result<Self> {
        if let Some(cached) = wf_cache::load_blob::<Vec<Entry>>(CACHE_FILE) {
            if cached.age() < ttl {
                tracing::info!("part quantities from cache ({} entries)", cached.value.len());
                return Ok(Self::new(cached.value));
            }
            match fetch(client).await {
                Ok(entries) => {
                    let _ = wf_cache::save_blob(CACHE_FILE, &entries);
                    return Ok(Self::new(entries));
                }
                Err(e) => {
                    tracing::warn!("part quantity refresh failed ({e}); using stale cache");
                    return Ok(Self::new(cached.value));
                }
            }
        }
        let entries = fetch(client).await?;
        let _ = wf_cache::save_blob(CACHE_FILE, &entries);
        Ok(Self::new(entries))
    }
}

#[derive(Debug, Deserialize)]
struct RawItem {
    name: String,
    #[serde(default, rename = "isPrime")]
    is_prime: bool,
    #[serde(default)]
    components: Vec<RawComponent>,
}

#[derive(Debug, Deserialize)]
struct RawComponent {
    name: String,
    #[serde(default, rename = "itemCount")]
    item_count: u32,
    /// True for an actual relic-sourced Prime Part (Barrel, Blueprint, …),
    /// false for a plain crafting resource entry (Orokin Cell, Circuits, …)
    /// that a build recipe also lists in `components`. WFCD tags every
    /// resource entry `"type": "Resource"` with empty `drops`, and every real
    /// Prime Part `tradable: true` with populated `drops` — perfectly
    /// correlated across the whole Prime catalogue, so this flag alone
    /// distinguishes them without needing to parse `drops` or `type`.
    #[serde(default)]
    tradable: bool,
}

/// Every Prime item's component quantities in one category file's JSON,
/// tagged with that file's [`EquipmentCategory`] (see [`category_for_file`]).
/// Non-tradable components (plain crafting resources like Orokin Cell) are
/// dropped — they're real Foundry ingredients but never a relic-sourced Prime
/// Part, so they don't belong in a relic-farming BOM.
fn parse_category(body: &str, category: EquipmentCategory) -> anyhow::Result<Vec<Entry>> {
    let items: Vec<RawItem> = serde_json::from_str(body)?;
    Ok(items
        .into_iter()
        .filter(|i| i.is_prime)
        .flat_map(|i| {
            i.components
                .into_iter()
                .filter(|c| c.tradable)
                .map(move |c| Entry {
                    prime: i.name.clone(),
                    part: c.name,
                    quantity: c.item_count,
                    category,
                })
        })
        .collect())
}

/// Fetch every Prime item's component quantities across the per-category
/// files that carry Prime items. A single category's fetch/parse failure is
/// logged and skipped rather than failing the whole load — partial quantity
/// data (shown only where known, see [`PartQuantities::get`]) beats none.
async fn fetch(client: &reqwest::Client) -> anyhow::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for category in CATEGORIES {
        let url = format!("{BASE}/{category}.json");
        tracing::debug!("GET {url}");
        let resp = match client.get(&url).send().await.and_then(|r| r.error_for_status()) {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!("part quantities: fetching {category} failed: {e}");
                continue;
            }
        };
        let body = match resp.text().await {
            Ok(body) => body,
            Err(e) => {
                tracing::warn!("part quantities: reading {category} failed: {e}");
                continue;
            }
        };
        match parse_category(&body, category_for_file(category)) {
            Ok(mut parsed) => entries.append(&mut parsed),
            Err(e) => tracing::warn!("part quantities: parsing {category} failed: {e}"),
        }
    }
    if entries.is_empty() {
        anyhow::bail!("no part quantity data fetched from any category");
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_category_keeps_only_prime_items_and_their_components() {
        let body = r#"[
            {
                "name": "Ash Prime",
                "isPrime": true,
                "components": [
                    {"name": "Blueprint", "itemCount": 1, "tradable": true},
                    {"name": "Chassis", "itemCount": 1, "tradable": true}
                ]
            },
            {
                "name": "Ash",
                "isPrime": false,
                "components": [{"name": "Blueprint", "itemCount": 1, "tradable": true}]
            }
        ]"#;
        let entries = parse_category(body, EquipmentCategory::Warframe).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&Entry {
            prime: "Ash Prime".to_string(),
            part: "Chassis".to_string(),
            quantity: 1,
            category: EquipmentCategory::Warframe,
        }));
        assert!(!entries.iter().any(|e| e.prime == "Ash"));
    }

    #[test]
    fn parse_category_captures_multi_quantity_parts() {
        let body = r#"[
            {
                "name": "Afuris Prime",
                "isPrime": true,
                "components": [
                    {"name": "Blueprint", "itemCount": 1, "tradable": true},
                    {"name": "Barrel", "itemCount": 2, "tradable": true},
                    {"name": "Receiver", "itemCount": 2, "tradable": true},
                    {"name": "Link", "itemCount": 1, "tradable": true}
                ]
            }
        ]"#;
        let entries = parse_category(body, EquipmentCategory::Primary).unwrap();
        let quantities = PartQuantities::new(entries);
        assert_eq!(
            quantities.get(&PrimePart { prime: "Afuris Prime".to_string(), part: "Barrel".to_string() }),
            Some(2)
        );
        assert_eq!(
            quantities.get(&PrimePart { prime: "Afuris Prime".to_string(), part: "Link".to_string() }),
            Some(1)
        );
    }

    #[test]
    fn parse_category_drops_non_tradable_resource_components() {
        // Real WFCD shape for Afentis Prime: Orokin Cell is a plain Foundry
        // ingredient (type: "Resource", tradable: false, empty drops), not a
        // relic-sourced Prime Part — it must not show up as a BOM gap.
        let body = r#"[
            {
                "name": "Afentis Prime",
                "isPrime": true,
                "components": [
                    {"name": "Blueprint", "itemCount": 1, "tradable": true},
                    {"name": "Barrel", "itemCount": 1, "tradable": true},
                    {"name": "Orokin Cell", "itemCount": 10, "tradable": false, "type": "Resource"}
                ]
            }
        ]"#;
        let entries = parse_category(body, EquipmentCategory::Primary).unwrap();
        assert!(!entries.iter().any(|e| e.part == "Orokin Cell"));
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn parts_for_returns_every_component_for_a_known_prime_and_empty_for_unknown() {
        let quantities = PartQuantities::from_entries_for_test(vec![
            ("Afuris Prime".to_string(), "Barrel".to_string(), 2),
            ("Afuris Prime".to_string(), "Link".to_string(), 1),
            ("Loki Prime".to_string(), "Systems".to_string(), 1),
        ]);
        let mut afuris = quantities.parts_for("Afuris Prime");
        afuris.sort();
        assert_eq!(afuris, vec![("Barrel".to_string(), 2), ("Link".to_string(), 1)]);
        assert!(quantities.parts_for("Nonexistent Prime").is_empty());
    }

    #[test]
    fn primes_is_sorted_and_deduped() {
        let quantities = PartQuantities::from_entries_for_test(vec![
            ("Loki Prime".to_string(), "Systems".to_string(), 1),
            ("Afuris Prime".to_string(), "Barrel".to_string(), 2),
            ("Afuris Prime".to_string(), "Link".to_string(), 1),
        ]);
        assert_eq!(quantities.primes(), vec!["Afuris Prime", "Loki Prime"]);
    }

    #[test]
    fn has_prime_true_only_for_a_known_prime() {
        let quantities = PartQuantities::from_entries_for_test(vec![(
            "Afuris Prime".to_string(),
            "Barrel".to_string(),
            2,
        )]);
        assert!(quantities.has_prime("Afuris Prime"));
        assert!(!quantities.has_prime("Nonexistent Prime"));
    }

    #[test]
    fn get_is_none_for_an_unknown_part() {
        let quantities = PartQuantities::new(Vec::new());
        assert_eq!(
            quantities.get(&PrimePart { prime: "Nonexistent Prime".to_string(), part: "Blueprint".to_string() }),
            None
        );
    }

    #[test]
    fn parse_category_ignores_non_component_items_gracefully() {
        // Items without a `components` field (resources, mods, …) default to
        // empty rather than failing the whole file's parse.
        let body = r#"[{"name": "Loki Prime", "isPrime": true}]"#;
        let entries = parse_category(body, EquipmentCategory::Warframe).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn category_for_file_maps_archwing_and_companion_sources_to_one_bucket_each() {
        assert_eq!(category_for_file("Warframes"), EquipmentCategory::Warframe);
        assert_eq!(category_for_file("Archwing"), EquipmentCategory::Archwing);
        assert_eq!(category_for_file("Arch-Gun"), EquipmentCategory::Archwing);
        assert_eq!(category_for_file("Arch-Melee"), EquipmentCategory::Archwing);
        assert_eq!(category_for_file("Sentinels"), EquipmentCategory::Companion);
        assert_eq!(category_for_file("SentinelWeapons"), EquipmentCategory::Companion);
        assert_eq!(category_for_file("Pets"), EquipmentCategory::Companion);
        assert_eq!(category_for_file("Necramechs"), EquipmentCategory::Other);
    }

    #[test]
    fn category_for_returns_the_assigned_category_or_other_for_an_unknown_prime() {
        let quantities = PartQuantities::from_entries_with_category_for_test(vec![(
            "Carrier Prime".to_string(),
            "Blueprint".to_string(),
            1,
            EquipmentCategory::Companion,
        )]);
        assert_eq!(quantities.category_for("Carrier Prime"), EquipmentCategory::Companion);
        assert_eq!(quantities.category_for("Nonexistent Prime"), EquipmentCategory::Other);
    }
}
