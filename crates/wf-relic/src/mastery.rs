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
                mastered.insert(canonical(leaf_of(&path)));
            }
        }
        Self { mastered }
    }

    /// Whether the built item a reward part belongs to has been mastered.
    ///
    /// Both sides reduce to a [`canonical`] token set, so an internal path leaf
    /// (`"PrimeGram"`, `"RubicoPrimeWeapon"`) matches a reward display
    /// (`"Gram Prime Blueprint"`, `"Rubico Prime"`) regardless of word order or a
    /// trailing suffix.
    pub fn is_mastered(&self, reward_item_name: &str) -> bool {
        self.mastered.contains(&canonical(reward_item_name))
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
    // `-v3`: the on-disk set stores normalized keys, and the normalization
    // (see `canonical`) changed — bump the name so old caches are ignored.
    let file = format!("mastery-v3-{account_id}.json");
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

/// The leaf of an internal path: `/Lotus/Powersuits/Ember/EmberPrime` → `EmberPrime`.
fn leaf_of(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// Split a name into words on non-alphanumerics **and** camelCase boundaries, so
/// `"RubicoPrimeWeapon"` → `["Rubico","Prime","Weapon"]` and `"PrimeGram"` →
/// `["Prime","Gram"]`, matching space-separated display names.
fn split_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_alnum_lower = false;
    for c in s.chars() {
        if !c.is_ascii_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_alnum_lower = false;
            continue;
        }
        if c.is_ascii_uppercase() && prev_alnum_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur)); // camelCase boundary
        }
        cur.push(c);
        prev_alnum_lower = !c.is_ascii_uppercase();
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A canonical mastery key: split into words, lowercase, drop component/suffix
/// words (keeping the distinguishing name + `"prime"`), sort, then **concatenate**
/// — so an internal path leaf and a reward display name reduce to the same key
/// regardless of word order (`PrimeGram` vs `Gram Prime`), a suffix
/// (`RubicoPrimeWeapon`), or an extra camelCase split within a display word
/// (`PrimeAkBoltoWeapon` → `ak`+`bolto`, which re-joins to match `Akbolto`).
fn canonical(s: &str) -> String {
    let mut toks: Vec<String> = split_words(s)
        .into_iter()
        .map(|w| w.to_ascii_lowercase())
        .filter(|w| w != "weapon" && !COMPONENTS.contains(&w.as_str()))
        .collect();
    toks.sort();
    toks.concat()
}

/// Component words stripped from a reward part name to get the built item's
/// base name.
const COMPONENTS: &[&str] = &[
    "blueprint", "systems", "chassis", "neuroptics", "barrel", "receiver", "stock", "link",
    "blade", "handle", "hilt", "guard", "grip", "head", "string", "limb", "lower", "upper",
    "ornament", "boot", "gauntlet", "carapace", "cerebrum", "wings", "harness", "pouch", "star",
    "disc", "band", "buckle", "clamp", "collar",
];

/// The built prime's **display** name for a reward part, e.g.
/// `"Ember Prime Systems Blueprint"` → `"Ember Prime"`. Used to dedup and label
/// a relic's rewards by the item they build into.
pub fn built_name(reward: &str) -> String {
    reward
        .split_whitespace()
        .filter(|w| !COMPONENTS.contains(&w.to_ascii_lowercase().as_str()))
        .collect::<Vec<_>>()
        .join(" ")
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
    fn canonical_is_order_suffix_and_split_invariant() {
        // Reversed order, a trailing suffix, and an extra camelCase split within a
        // display word all reduce to the same key as the display name.
        assert_eq!(canonical("PrimeGram"), canonical("Gram Prime Blueprint"));
        assert_eq!(canonical("RubicoPrimeWeapon"), canonical("Rubico Prime Blueprint"));
        assert_eq!(canonical("EmberPrime"), canonical("Ember Prime Systems Blueprint"));
        // `Ak`+`Bolto` re-joins to match the single display word `Akbolto`.
        assert_eq!(canonical("PrimeAkBoltoWeapon"), canonical("Akbolto Prime Blueprint"));
        // Different primes don't collide.
        assert_ne!(canonical("Gram Prime"), canonical("Rubico Prime"));
    }

    #[test]
    fn mastery_matches_reversed_suffixed_and_split_paths() {
        // The cases that slipped through: Prime-first, -Weapon suffix, and a
        // camelCase split within a display word (`PrimeAkBoltoWeapon`).
        let set = MasterySet::from_xp([
            ("/Lotus/Weapons/Tenno/Melee/Swords/PrimeGram/PrimeGram".to_string(), 16_000_000),
            ("/Lotus/Weapons/Tenno/LongGuns/RubicoPrime/RubicoPrimeWeapon".to_string(), 3_000_000),
            ("/Lotus/Weapons/Tenno/Pistols/PrimeAkbolto/PrimeAkBoltoWeapon".to_string(), 648_728),
            ("/Lotus/Powersuits/Ember/EmberPrime".to_string(), 9_000_000),
            ("/Lotus/Weapons/Tenno/LongGuns/BratonPrime".to_string(), 100_000), // below cap
        ]);
        assert!(set.is_mastered("Gram Prime Blueprint"));
        assert!(set.is_mastered("Rubico Prime Blueprint"));
        assert!(set.is_mastered("Akbolto Prime Blueprint"));
        assert!(set.is_mastered("Ember Prime Systems Blueprint"));
        assert!(!set.is_mastered("Braton Prime Receiver")); // below cap → not mastered
        assert!(!set.is_mastered("Volnus Prime Blueprint")); // absent
    }
}
