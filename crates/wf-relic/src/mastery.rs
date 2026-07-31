//! Mastery tracking via Digital Extremes' **public** profile API.
//!
//! `getProfileViewingData.php?playerId=<accountId>` returns public profile data
//! (the same the game shows when you inspect a player) — no authentication. Its
//! `LoadOutInventory.XPInfo` lists every item the player has earned affinity on,
//! by internal path and lifetime affinity. An item is **mastered** once its
//! lifetime affinity reaches the rank-30 cap (which never resets, even on Forma),
//! so `affinity >= cap` is a stable mastery test.
//!
//! Caps derive from the standard formula (cumulative affinity to rank 30):
//! weapons `1000·r²/2 = 450,000`; Warframes/companions/archwing `2×` that
//! `= 900,000`. Verified against a real high-MR profile.

use std::collections::HashSet;

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// PC profile endpoint (public, no auth).
const PC_ENDPOINT: &str = "https://api.warframe.com/cdn/getProfileViewingData.php";

const WEAPON_CAP: u64 = 450_000;
const FRAME_CAP: u64 = 900_000;

/// The set of items a player has mastered, keyed by normalized base name
/// (e.g. `"emberprime"`), so a reward *part* can be tested against the built
/// item it belongs to.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MasterySet {
    mastered: HashSet<String>,
}

impl MasterySet {
    pub fn len(&self) -> usize {
        self.mastered.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mastered.is_empty()
    }

    /// Build a set from `(item_path, lifetime_affinity)` pairs.
    pub fn from_xp(entries: impl IntoIterator<Item = (String, u64)>) -> Self {
        let mut mastered = HashSet::new();
        for (path, xp) in entries {
            if xp >= cap_for(&path) {
                mastered.insert(leaf_norm(&path));
            }
        }
        Self { mastered }
    }

    /// Whether the built item a reward part belongs to has been mastered, e.g.
    /// `"Ember Prime Systems Blueprint"` → base `"emberprime"`.
    pub fn is_mastered(&self, reward_item_name: &str) -> bool {
        self.mastered.contains(&base_norm(reward_item_name))
    }
}

/// Fetch and build a [`MasterySet`] for `account_id` (24-hex) on PC.
pub async fn fetch(client: &reqwest::Client, account_id: &str) -> anyhow::Result<MasterySet> {
    let url = format!("{PC_ENDPOINT}?playerId={account_id}");
    tracing::debug!("GET {url}");
    let body = client
        .get(&url)
        // DE's CDN expects a browser-like UA.
        .header("User-Agent", "Mozilla/5.0")
        .send()
        .await?
        .error_for_status()
        .context("profile request failed (is the account id correct?)")?
        .json::<ProfileResponse>()
        .await
        .context("parsing profile JSON")?;

    let result = body
        .results
        .into_iter()
        .next()
        .context("profile response had no Results")?;
    let set = MasterySet::from_xp(
        result
            .loadout
            .xp_info
            .into_iter()
            .map(|e| (e.item_type, e.xp)),
    );
    tracing::info!("mastery: {} mastered items for {account_id}", set.len());
    Ok(set)
}

/// Fetch just the public `DisplayName` for `account_id`, used to verify that a
/// candidate account id (e.g. scraped from `EE.log`) actually belongs to the
/// local player. Returns `None` if the profile has no name / does not exist.
pub async fn fetch_display_name(
    client: &reqwest::Client,
    account_id: &str,
) -> anyhow::Result<Option<String>> {
    let url = format!("{PC_ENDPOINT}?playerId={account_id}");
    let body = client
        .get(&url)
        .header("User-Agent", "Mozilla/5.0")
        .send()
        .await?
        .error_for_status()
        .context("profile request failed")?
        .json::<ProfileResponse>()
        .await
        .context("parsing profile JSON")?;
    let name = body
        .results
        .into_iter()
        .next()
        .map(|r| r.display_name)
        .filter(|n| !n.is_empty());
    Ok(name)
}

/// Load the mastered set from a disk cache when fresh (younger than `ttl`),
/// otherwise refetch. Falls back to a stale cache on network failure, and to an
/// empty set if there is nothing cached and the fetch fails.
pub async fn load_cached(
    client: &reqwest::Client,
    account_id: &str,
    ttl: std::time::Duration,
) -> MasterySet {
    let file = format!("mastery-{account_id}.json");
    if let Some(cached) = wf_cache::load_blob::<MasterySet>(&file) {
        if cached.age() < ttl {
            tracing::info!("mastery from cache ({} items)", cached.value.len());
            return cached.value;
        }
        match fetch(client, account_id).await {
            Ok(set) => {
                let _ = wf_cache::save_blob(&file, &set);
                return set;
            }
            Err(e) => {
                tracing::warn!("mastery refresh failed ({e:#}); using stale cache");
                return cached.value;
            }
        }
    }
    match fetch(client, account_id).await {
        Ok(set) => {
            let _ = wf_cache::save_blob(&file, &set);
            set
        }
        Err(e) => {
            tracing::warn!("mastery fetch failed: {e:#}");
            MasterySet::default()
        }
    }
}

#[derive(Deserialize)]
struct ProfileResponse {
    #[serde(rename = "Results", default)]
    results: Vec<ProfileResult>,
}

#[derive(Deserialize)]
struct ProfileResult {
    #[serde(rename = "LoadOutInventory", default)]
    loadout: LoadOut,
    #[serde(rename = "DisplayName", default)]
    display_name: String,
}

#[derive(Deserialize, Default)]
struct LoadOut {
    #[serde(rename = "XPInfo", default)]
    xp_info: Vec<XpEntry>,
}

#[derive(Deserialize)]
struct XpEntry {
    #[serde(rename = "ItemType")]
    item_type: String,
    #[serde(rename = "XP", default)]
    xp: u64,
}

/// Rank-30 affinity cap for an item, chosen by its category path.
fn cap_for(path: &str) -> u64 {
    let p = path.to_ascii_lowercase();
    if p.contains("/powersuits/")
        || p.contains("/sentinels/")
        || p.contains("kubrow")
        || p.contains("catbrow")
        || p.contains("necromech")
    {
        FRAME_CAP
    } else {
        WEAPON_CAP
    }
}

/// Normalize the leaf of an internal path: `/Lotus/Powersuits/Ember/EmberPrime`
/// → `"emberprime"`.
fn leaf_norm(path: &str) -> String {
    let leaf = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    leaf.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Component words stripped from a reward part name to get the built item's
/// base name.
const COMPONENTS: &[&str] = &[
    "blueprint", "systems", "chassis", "neuroptics", "barrel", "receiver", "stock", "link",
    "blade", "handle", "hilt", "guard", "grip", "head", "string", "limb", "lower", "upper",
    "ornament", "boot", "gauntlet", "carapace", "cerebrum", "wings", "harness", "pouch", "star",
    "disc", "band", "buckle", "clamp", "collar",
];

/// `"Ember Prime Systems Blueprint"` → `"emberprime"`.
fn base_norm(name: &str) -> String {
    name.split_whitespace()
        .filter(|w| !COMPONENTS.contains(&w.to_ascii_lowercase().as_str()))
        .flat_map(|w| w.chars())
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_by_category() {
        assert_eq!(cap_for("/Lotus/Powersuits/Ember/EmberPrime"), FRAME_CAP);
        assert_eq!(cap_for("/Lotus/Weapons/Tenno/LongGuns/BratonPrime"), WEAPON_CAP);
    }

    #[test]
    fn base_name_strips_components() {
        assert_eq!(base_norm("Ember Prime Systems Blueprint"), "emberprime");
        assert_eq!(base_norm("Paris Prime Lower Limb"), "parisprime");
        assert_eq!(base_norm("Braton Prime Receiver"), "bratonprime");
    }

    #[test]
    fn mastery_lookup_via_base_name() {
        let set = MasterySet::from_xp([
            ("/Lotus/Powersuits/Ember/EmberPrime".to_string(), 9_000_000), // mastered
            ("/Lotus/Weapons/Tenno/LongGuns/BratonPrime".to_string(), 100_000), // not
        ]);
        assert!(set.is_mastered("Ember Prime Systems Blueprint"));
        assert!(!set.is_mastered("Braton Prime Receiver"));
        assert!(!set.is_mastered("Volnus Prime Blueprint"));
    }
}
