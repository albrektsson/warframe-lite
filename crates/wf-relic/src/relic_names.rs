//! Decode a raw owned-relic internal name (e.g.
//! `/Lotus/Types/Game/Projections/T4VoidProjectionIvaraPrimeBBronze`, from
//! [`wf_mem::OwnedRelic::item_type`]) into its player-facing identity — tier,
//! code, and refinement (e.g. "Axi B3", "Intact").
//!
//! The reward-pool letter embedded in the internal name (`...IvaraPrimeB...`
//! above) bears no direct relationship to the public code — live data shows
//! it decodes to "Axi B3", not "Axi B\<n\>" for any predictable `n` — so this
//! can't be parsed or guessed from the internal name alone. It's resolved by
//! exact lookup against WFCD `warframe-items`' `Relics.json`, a third,
//! separate WFCD dataset from `warframe-drop-data`'s reward tables
//! ([`crate::relics`]) and `warframe-items`' per-category Prime files
//! ([`crate::part_quantities`], see ADR-0011): only this file's `uniqueName`
//! field carries DE's internal relic path alongside the matching
//! player-facing label (e.g. `"Axi B3 Intact"`).
//!
//! Confirms, against this real WFCD data rather than by inference from
//! third-party consumer source (issue #63's caveat): `T1`-`T5` are
//! Lith/Meso/Neo/Axi/Requiem in that order, and the trailing refinement
//! suffix `Bronze`/`Silver`/`Gold`/`Platinum` maps exactly to
//! Intact/Exceptional/Flawless/Radiant.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

const URL: &str = "https://raw.githubusercontent.com/WFCD/warframe-items/master/data/json/Relics.json";
const CACHE_FILE: &str = "relic-names-v1.json";

/// The four known refinement words, in domain order (worst to best) — also
/// the only strings [`parse`] accepts as a trailing refinement word, so a
/// tier-only placeholder row (`"Void Relic"`, `"Lith Relic"`, no
/// refinement/code at all) is never mistaken for one.
const REFINEMENTS: [&str; 4] = ["Intact", "Exceptional", "Flawless", "Radiant"];

/// The game's relic-era order, oldest/lowest tier first, `Requiem` last.
const TIERS: [&str; 5] = ["Lith", "Meso", "Neo", "Axi", "Requiem"];

/// A decoded relic identity: era, code within that era, and refinement state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelicIdentity {
    /// Era, e.g. `"Axi"`.
    pub tier: String,
    /// Code within the era, e.g. `"B3"` (or a Roman numeral for Requiem,
    /// e.g. `"I"`).
    pub code: String,
    /// One of [`REFINEMENTS`], e.g. `"Intact"`.
    pub refinement: String,
}

impl RelicIdentity {
    /// Player-facing label without refinement, e.g. `"Axi B3"` — matches
    /// [`crate::relics::RelicInfo::display`]'s format.
    pub fn display(&self) -> String {
        format!("{} {}", self.tier, self.code)
    }

    /// Sort key ordering by [`TIERS`] era, then code, then [`REFINEMENTS`]
    /// state — an unknown tier/refinement (shouldn't happen post-[`parse`]'s
    /// filtering, but never panics) sorts last within its group. Public so a
    /// caller holding `RelicIdentity`s alongside other data (e.g. `mem-scan`'s
    /// owned-relic entries) can sort directly with `.sort_by(...)` rather than
    /// through a wrapper.
    pub fn sort_key(&self) -> (usize, &str, usize) {
        let tier_rank = TIERS.iter().position(|t| *t == self.tier).unwrap_or(TIERS.len());
        let refinement_rank =
            REFINEMENTS.iter().position(|r| *r == self.refinement).unwrap_or(REFINEMENTS.len());
        (tier_rank, self.code.as_str(), refinement_rank)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    unique_name: String,
    identity: RelicIdentity,
}

/// Raw owned-relic internal name → decoded [`RelicIdentity`] lookup.
pub struct RelicNameIndex {
    by_unique_name: HashMap<String, RelicIdentity>,
}

impl RelicNameIndex {
    fn new(entries: Vec<Entry>) -> Self {
        Self { by_unique_name: entries.into_iter().map(|e| (e.unique_name, e.identity)).collect() }
    }

    /// An empty index — every [`Self::lookup`] call returns `None`. For tests
    /// and callers with no name data available yet.
    pub fn empty() -> Self {
        Self { by_unique_name: HashMap::new() }
    }

    /// Build an index directly from `(unique_name, identity)` pairs, for
    /// tests that need known entries without going through a fetch.
    pub fn from_entries_for_test(entries: Vec<(String, RelicIdentity)>) -> Self {
        Self { by_unique_name: entries.into_iter().collect() }
    }

    /// The decoded identity for a raw owned-relic `item_type` (e.g. from
    /// [`wf_mem::OwnedRelic::item_type`]), if this catalogue has it.
    pub fn lookup(&self, item_type: &str) -> Option<&RelicIdentity> {
        self.by_unique_name.get(item_type)
    }

    /// Fetch + cache (weekly TTL, stale-served on failure), mirroring
    /// [`crate::RelicIndex::load_cached`].
    pub async fn load_cached(client: &reqwest::Client, ttl: Duration) -> anyhow::Result<Self> {
        if let Some(cached) = wf_cache::load_blob::<Vec<Entry>>(CACHE_FILE) {
            if cached.age() < ttl {
                tracing::info!("relic name index from cache ({} entries)", cached.value.len());
                return Ok(Self::new(cached.value));
            }
            match fetch(client).await {
                Ok(entries) => {
                    let _ = wf_cache::save_blob(CACHE_FILE, &entries);
                    return Ok(Self::new(entries));
                }
                Err(e) => {
                    tracing::warn!("relic name index refresh failed ({e}); using stale cache");
                    return Ok(Self::new(cached.value));
                }
            }
        }
        let entries = fetch(client).await?;
        let _ = wf_cache::save_blob(CACHE_FILE, &entries);
        Ok(Self::new(entries))
    }
}

#[derive(Deserialize)]
struct RawRelic {
    #[serde(rename = "uniqueName")]
    unique_name: String,
    name: String,
}

/// Parse WFCD `warframe-items`' `Relics.json`. Each real per-refinement
/// entry's `name` is `"{Tier} {Code} {Refinement}"` (e.g. `"Axi B3 Intact"`,
/// `"Requiem I Radiant"`) — split off the trailing word and accept it only if
/// it's one of [`REFINEMENTS`], so a tier-only placeholder row (`"Void
/// Relic"`, `"Lith Relic"` — no code, no refinement) is skipped rather than
/// parsed into a nonsense identity. Skipped entries never match a real
/// owned-relic `item_type` anyway: [`crate::relics`]'s `is_void_projection`
/// filter requires a `VoidProjection` marker, and only the real per-refinement
/// rows carry a `uniqueName` matching that shape.
fn parse(body: &str) -> anyhow::Result<Vec<Entry>> {
    let raw: Vec<RawRelic> = serde_json::from_str(body)?;
    Ok(raw
        .into_iter()
        .filter_map(|r| {
            let mut words: Vec<&str> = r.name.split_whitespace().collect();
            let refinement = *words.last()?;
            if !REFINEMENTS.contains(&refinement) || words.len() < 3 {
                return None;
            }
            words.pop();
            let code = words.pop()?.to_string();
            let tier = words.join(" ");
            Some(Entry {
                unique_name: r.unique_name,
                identity: RelicIdentity { tier, code, refinement: refinement.to_string() },
            })
        })
        .collect())
}

async fn fetch(client: &reqwest::Client) -> anyhow::Result<Vec<Entry>> {
    tracing::debug!("GET {URL}");
    let body = client.get(URL).send().await?.error_for_status()?.text().await?;
    parse(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(unique_name: &str, name: &str) -> String {
        format!(r#"{{"uniqueName":"{unique_name}","name":"{name}"}}"#)
    }

    #[test]
    fn parses_a_standard_relic_refinement_row() {
        let body = format!(
            "[{}]",
            raw("/Lotus/Types/Game/Projections/T4VoidProjectionIvaraPrimeBBronze", "Axi B3 Intact")
        );
        let entries = parse(&body).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].unique_name, "/Lotus/Types/Game/Projections/T4VoidProjectionIvaraPrimeBBronze");
        assert_eq!(
            entries[0].identity,
            RelicIdentity { tier: "Axi".to_string(), code: "B3".to_string(), refinement: "Intact".to_string() }
        );
    }

    #[test]
    fn parses_a_requiem_roman_numeral_code() {
        let body = format!(
            "[{}]",
            raw("/Lotus/Types/Game/Projections/T5VoidProjectionImmortalAPlatinum", "Requiem I Radiant")
        );
        let entries = parse(&body).unwrap();
        assert_eq!(
            entries[0].identity,
            RelicIdentity { tier: "Requiem".to_string(), code: "I".to_string(), refinement: "Radiant".to_string() }
        );
    }

    #[test]
    fn skips_tier_only_placeholder_rows_with_no_code_or_refinement() {
        let body = format!(
            "[{},{}]",
            raw("/Lotus/Types/Game/Projections/T1VoidProjection", "Lith Relic"),
            raw("/Lotus/Types/Game/Projections/T0VoidProjection", "Void Relic")
        );
        assert!(parse(&body).unwrap().is_empty());
    }

    #[test]
    fn skips_a_row_whose_last_word_is_not_a_known_refinement() {
        // A base/unrefined relic bundle name that happens to have 3+ words but
        // no real refinement suffix — must not be misparsed as one.
        let body = format!(
            "[{}]",
            raw("/Lotus/Types/Game/Projections/T5VoidProjectionImmortalOmniA", "Requiem Eterna Relic")
        );
        assert!(parse(&body).unwrap().is_empty());
    }

    #[test]
    fn lookup_finds_a_known_entry_and_misses_an_unknown_one() {
        let idx = RelicNameIndex::from_entries_for_test(vec![(
            "/Lotus/Types/Game/Projections/T4VoidProjectionIvaraPrimeBBronze".to_string(),
            RelicIdentity { tier: "Axi".to_string(), code: "B3".to_string(), refinement: "Intact".to_string() },
        )]);
        assert_eq!(
            idx.lookup("/Lotus/Types/Game/Projections/T4VoidProjectionIvaraPrimeBBronze").map(|i| i.display()),
            Some("Axi B3".to_string())
        );
        assert!(idx.lookup("/Lotus/Types/Game/Projections/T4VoidProjectionUnknownXBronze").is_none());
    }

    #[test]
    fn sort_key_orders_by_tier_era_then_code_then_refinement() {
        let mut items = [
            RelicIdentity { tier: "Axi".to_string(), code: "B3".to_string(), refinement: "Radiant".to_string() },
            RelicIdentity { tier: "Lith".to_string(), code: "S1".to_string(), refinement: "Intact".to_string() },
            RelicIdentity { tier: "Axi".to_string(), code: "B3".to_string(), refinement: "Intact".to_string() },
            RelicIdentity { tier: "Axi".to_string(), code: "A1".to_string(), refinement: "Intact".to_string() },
        ];
        items.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        let labels: Vec<String> =
            items.iter().map(|i| format!("{} ({})", i.display(), i.refinement)).collect();
        assert_eq!(
            labels,
            vec![
                "Lith S1 (Intact)".to_string(),
                "Axi A1 (Intact)".to_string(),
                "Axi B3 (Intact)".to_string(),
                "Axi B3 (Radiant)".to_string(),
            ]
        );
    }
}
