//! Bulk historical Riven sale statistics from WFCD's `warframestat.us` API —
//! `GET /pc/rivens`. A **different** data source and shape from
//! [`crate::riven_pricing`]'s live warframe.market auction percentiles: this
//! is pre-aggregated avg/median/min/max/stddev/`pop` (sample size) over *all*
//! recorded historical sales, split only by rerolled vs. unrolled, not a
//! live-listing snapshot. One unauthenticated GET returns every weapon's
//! stats at once (confirmed live, 2026-08-17: ~145KB, no rate limit), so this
//! is the Rivens tab's fast, whole-list price signal for sort/filter — the
//! live per-weapon Floor/Ceiling/Verdict (ADR-0020) stays the authoritative
//! number shown once it lazily resolves; this is only ever a placeholder
//! estimate ahead of that.
//!
//! Fetch + cache mirrors [`crate::riven_catalogue::RivenCatalogue::load_cached`]
//! exactly (weekly TTL, stale-served on failure) — see that doc for the
//! rationale, not repeated here.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::riven_catalogue::RivenModCategory;
use crate::riven_pricing::MIN_LISTINGS;

const URL: &str = "https://api.warframestat.us/pc/rivens";
const CACHE_FILE: &str = "riven-bulk-stats-v1.json";

/// The top-level category keys this endpoint uses, e.g. `"Rifle Riven Mod"`
/// — confirmed live to be exactly these seven, 1:1 with [`RivenModCategory`].
fn parse_category(key: &str) -> Option<RivenModCategory> {
    match key {
        "Rifle Riven Mod" => Some(RivenModCategory::Rifle),
        "Shotgun Riven Mod" => Some(RivenModCategory::Shotgun),
        "Pistol Riven Mod" => Some(RivenModCategory::Pistol),
        "Melee Riven Mod" => Some(RivenModCategory::Melee),
        "Archgun Riven Mod" => Some(RivenModCategory::Archgun),
        "Kitgun Riven Mod" => Some(RivenModCategory::Kitgun),
        "Zaw Riven Mod" => Some(RivenModCategory::Zaw),
        _ => None,
    }
}

/// One `(category, weapon name, rerolled)` entry's stats — only `median` and
/// `pop` are kept; the endpoint also carries `avg`/`min`/`max`/`stddev`, but
/// nothing in this app reads them (median is the sort/filter signal, `pop`
/// its confidence gate — see [`RivenBulkStats::median_plat`]).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct BulkRivenStat {
    median: f64,
    pop: u32,
}

/// `(category, weapon name, rerolled) -> stat`, kept as a flat `Vec` for the
/// cached shape (mirrors [`crate::riven_catalogue::RivenBaseValues`]) since
/// `serde_json` can't key a map on a tuple; collected into a `HashMap` by
/// [`RivenBulkStats::new`] for lookup.
type BulkStatsEntries = Vec<(RivenModCategory, String, bool, BulkRivenStat)>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct BulkStatsData {
    entries: BulkStatsEntries,
}

/// Bulk historical Riven price stats, keyed by `(mod category, weapon name,
/// rerolled)`. See the module doc for what this is and isn't.
pub struct RivenBulkStats {
    entries: HashMap<(RivenModCategory, String, bool), BulkRivenStat>,
}

impl RivenBulkStats {
    fn new(data: BulkStatsData) -> Self {
        Self {
            entries: data.entries.into_iter().map(|(cat, name, rerolled, stat)| ((cat, name, rerolled), stat)).collect(),
        }
    }

    /// An empty stats set — every lookup returns `None`. For tests and a
    /// failed-fetch-with-no-cache fallback.
    pub fn empty() -> Self {
        Self { entries: HashMap::new() }
    }

    /// Build a stats set directly from known `(category, weapon name,
    /// rerolled, median plat)` entries, for tests elsewhere in the workspace
    /// that need a known stats set without going through a fetch — mirrors
    /// [`crate::riven_catalogue::RivenCatalogue::from_parts_for_test`]. Each
    /// entry gets a sample size of exactly [`MIN_LISTINGS`], just enough to
    /// clear [`Self::median_plat`]'s own confidence gate.
    pub fn from_entries_for_test(entries: Vec<(RivenModCategory, String, bool, u32)>) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|(cat, name, rerolled, median)| {
                    ((cat, name, rerolled), BulkRivenStat { median: median as f64, pop: MIN_LISTINGS as u32 })
                })
                .collect(),
        }
    }

    /// The median historical sale price for `weapon_name` under
    /// `mod_category`, split by whether the copy being priced has been
    /// rerolled at least once. `None` when unmatched (no entry for this
    /// weapon/state at all) *or* when the matched entry's sample size is
    /// below [`MIN_LISTINGS`] — the same confidence gate
    /// [`crate::riven_pricing::evaluate`] applies to the live percentile
    /// data, reused here since a `pop: 1` median carries the same "don't
    /// trust this" caveat as a thin live-listing sample.
    pub fn median_plat(&self, mod_category: RivenModCategory, weapon_name: &str, rerolled: bool) -> Option<u32> {
        let stat = self.entries.get(&(mod_category, weapon_name.to_string(), rerolled))?;
        if (stat.pop as usize) < MIN_LISTINGS {
            return None;
        }
        Some(stat.median.round() as u32)
    }

    /// Fetch + cache (weekly TTL, stale-served on failure), mirroring
    /// [`crate::riven_catalogue::RivenCatalogue::load_cached`].
    pub async fn load_cached(client: &reqwest::Client, ttl: Duration) -> anyhow::Result<Self> {
        if let Some(cached) = wf_cache::load_blob::<BulkStatsData>(CACHE_FILE) {
            if cached.age() < ttl {
                tracing::info!("riven bulk stats from cache ({} entries)", cached.value.entries.len());
                return Ok(Self::new(cached.value));
            }
            match fetch(client).await {
                Ok(data) => {
                    let _ = wf_cache::save_blob(CACHE_FILE, &data);
                    return Ok(Self::new(data));
                }
                Err(e) => {
                    tracing::warn!("riven bulk stats refresh failed ({e}); using stale cache");
                    return Ok(Self::new(cached.value));
                }
            }
        }
        let data = fetch(client).await?;
        let _ = wf_cache::save_blob(CACHE_FILE, &data);
        Ok(Self::new(data))
    }
}

#[derive(Debug, Deserialize)]
struct RawStat {
    median: f64,
    pop: u32,
}

#[derive(Debug, Deserialize, Default)]
struct RawWeaponEntry {
    #[serde(default)]
    unrolled: Option<RawStat>,
    #[serde(default)]
    rerolled: Option<RawStat>,
}

fn parse_body(body: &str) -> anyhow::Result<BulkStatsEntries> {
    let raw: HashMap<String, HashMap<String, RawWeaponEntry>> = serde_json::from_str(body)?;
    let mut entries = Vec::new();
    for (category_key, weapons) in raw {
        let Some(mod_category) = parse_category(&category_key) else {
            continue;
        };
        for (weapon_name, entry) in weapons {
            if let Some(s) = entry.unrolled {
                entries.push((mod_category, weapon_name.clone(), false, BulkRivenStat { median: s.median, pop: s.pop }));
            }
            if let Some(s) = entry.rerolled {
                entries.push((mod_category, weapon_name.clone(), true, BulkRivenStat { median: s.median, pop: s.pop }));
            }
        }
    }
    Ok(entries)
}

async fn fetch(client: &reqwest::Client) -> anyhow::Result<BulkStatsData> {
    tracing::debug!("GET {URL}");
    let body = client.get(URL).send().await?.error_for_status()?.text().await?;
    let entries = parse_body(&body)?;
    if entries.is_empty() {
        anyhow::bail!("no riven bulk stats parsed");
    }
    Ok(BulkStatsData { entries })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_sample_body_into_both_rerolled_and_unrolled_entries() {
        let body = r#"{
            "Rifle Riven Mod": {
                "Soma Prime": {
                    "unrolled": {"itemType": "Rifle Riven Mod", "compatibility": "Soma Prime",
                                 "rerolled": false, "avg": 30, "stddev": 5, "min": 20, "max": 40,
                                 "pop": 6, "median": 28},
                    "rerolled": {"itemType": "Rifle Riven Mod", "compatibility": "Soma Prime",
                                 "rerolled": true, "avg": 90, "stddev": 20, "min": 50, "max": 200,
                                 "pop": 10, "median": 80}
                }
            },
            "Unknown Category": {
                "Something": {"unrolled": {"median": 9, "pop": 1}}
            }
        }"#;
        let entries = parse_body(body).unwrap();
        assert_eq!(entries.len(), 2);
        let stats = RivenBulkStats::new(BulkStatsData { entries });
        assert_eq!(stats.median_plat(RivenModCategory::Rifle, "Soma Prime", false), Some(28));
        assert_eq!(stats.median_plat(RivenModCategory::Rifle, "Soma Prime", true), Some(80));
    }

    #[test]
    fn below_min_listings_reads_as_no_confident_price() {
        let stats = RivenBulkStats::new(BulkStatsData {
            entries: vec![(RivenModCategory::Melee, "Test Melee".to_string(), false, BulkRivenStat { median: 500.0, pop: 1 })],
        });
        assert_eq!(stats.median_plat(RivenModCategory::Melee, "Test Melee", false), None);
    }

    #[test]
    fn an_unmatched_weapon_or_state_returns_none() {
        let stats = RivenBulkStats::new(BulkStatsData {
            entries: vec![(RivenModCategory::Melee, "Test Melee".to_string(), false, BulkRivenStat { median: 50.0, pop: 10 })],
        });
        assert_eq!(stats.median_plat(RivenModCategory::Melee, "Unknown Weapon", false), None);
        assert_eq!(stats.median_plat(RivenModCategory::Melee, "Test Melee", true), None);
    }

    #[test]
    fn empty_stats_returns_none_for_every_lookup() {
        let stats = RivenBulkStats::empty();
        assert_eq!(stats.median_plat(RivenModCategory::Rifle, "Anything", false), None);
    }
}
