//! Per-Prime-Part build quantities (e.g. Afuris Prime needs 2 Barrel) from
//! WFCD `warframe-items` — a second, separate WFCD dataset from
//! `warframe-drop-data` (see ADR-0011): the relic drop tables carry reward
//! names and rarities, but not how many of each a build recipe needs.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::mastery::PrimePart;

const BASE: &str = "https://raw.githubusercontent.com/WFCD/warframe-items/master/data/json";
const CACHE_FILE: &str = "part-quantities.json";

/// Per-category files that carry Prime items today. Fetched individually
/// rather than the much larger combined `All.json` (~54.5MB vs a few MB per
/// category — see ADR-0011). `Arch-Melee` currently has no Prime weapons, so
/// it's omitted; revisit if that ever changes.
const CATEGORIES: &[&str] = &[
    "Warframes",
    "Primary",
    "Secondary",
    "Melee",
    "Sentinels",
    "SentinelWeapons",
    "Archwing",
    "Arch-Gun",
];

/// One resolved (Built prime, Prime Part) build quantity, the cached/testable
/// unit `PartQuantities` is built from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    prime: String,
    part: String,
    quantity: u32,
}

/// Build-quantity lookup for Prime Parts. An absent entry means "unknown",
/// never "1" — a missing quantity is shown as unknown rather than guessed
/// (see ADR-0011).
pub struct PartQuantities {
    map: HashMap<(String, String), u32>,
}

impl PartQuantities {
    fn new(entries: Vec<Entry>) -> Self {
        let map = entries.into_iter().map(|e| ((e.prime, e.part), e.quantity)).collect();
        Self { map }
    }

    /// An empty lookup — every [`Self::get`] call returns `None`. For tests
    /// and callers with no quantity data available yet.
    pub fn empty() -> Self {
        Self { map: HashMap::new() }
    }

    /// Build a lookup directly from `(prime, part, quantity)` triples, for
    /// tests elsewhere in the crate that need known quantities without going
    /// through a fetch.
    pub fn from_entries_for_test(entries: Vec<(String, String, u32)>) -> Self {
        Self::new(entries.into_iter().map(|(prime, part, quantity)| Entry { prime, part, quantity }).collect())
    }

    /// How many of `part` a full build requires, if known.
    pub fn get(&self, part: &PrimePart) -> Option<u32> {
        self.map.get(&(part.prime.clone(), part.part.clone())).copied()
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
}

/// Every Prime item's component quantities in one category file's JSON.
fn parse_category(body: &str) -> anyhow::Result<Vec<Entry>> {
    let items: Vec<RawItem> = serde_json::from_str(body)?;
    Ok(items
        .into_iter()
        .filter(|i| i.is_prime)
        .flat_map(|i| {
            i.components
                .into_iter()
                .map(move |c| Entry { prime: i.name.clone(), part: c.name, quantity: c.item_count })
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
        match parse_category(&body) {
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
                    {"name": "Blueprint", "itemCount": 1},
                    {"name": "Chassis", "itemCount": 1}
                ]
            },
            {
                "name": "Ash",
                "isPrime": false,
                "components": [{"name": "Blueprint", "itemCount": 1}]
            }
        ]"#;
        let entries = parse_category(body).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&Entry {
            prime: "Ash Prime".to_string(),
            part: "Chassis".to_string(),
            quantity: 1
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
                    {"name": "Blueprint", "itemCount": 1},
                    {"name": "Barrel", "itemCount": 2},
                    {"name": "Receiver", "itemCount": 2},
                    {"name": "Link", "itemCount": 1}
                ]
            }
        ]"#;
        let entries = parse_category(body).unwrap();
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
        let entries = parse_category(body).unwrap();
        assert!(entries.is_empty());
    }
}
