//! Owned-relic guide: which relics contain rewards the player hasn't mastered.
//!
//! Relic → reward drop tables come from WFCD `warframe-drop-data` (readable item
//! names + rarity), cached to disk. Cross-referenced with the mastered set
//! ([`MasterySet`]) and warframe.market relic prices, this ranks the relics worth
//! opening for mastery. Ownership itself is supplied by the caller (from OCR of
//! the in-game Relics screen); this module is network/screen-agnostic and pure.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::index::{levenshtein, normalize};
use crate::mastery::{built_name, MasterySet};

const RELICS_URL: &str =
    "https://raw.githubusercontent.com/WFCD/warframe-drop-data/gh-pages/data/relics.json";
const CACHE_FILE: &str = "relics.json";

/// One reward inside a relic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelicReward {
    #[serde(rename = "itemName")]
    pub item_name: String,
    #[serde(default)]
    pub rarity: String,
}

/// A relic and its (Intact) reward set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelicInfo {
    /// Era, e.g. "Axi".
    pub tier: String,
    /// Code within the era, e.g. "H3".
    pub code: String,
    /// Display label, e.g. "Axi H3".
    pub display: String,
    pub rewards: Vec<RelicReward>,
}

impl RelicInfo {
    /// warframe.market slug, e.g. `"axi_h3_relic"`.
    pub fn slug(&self) -> String {
        format!("{}_{}_relic", self.tier.to_lowercase(), self.code.to_lowercase())
    }

    /// Distinct built primes among this relic's rewards that the player has **not**
    /// mastered (skips Forma/untradable), e.g. `["Ember Prime", "Trinity Prime"]`.
    pub fn unmastered(&self, mastery: &MasterySet) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in &self.rewards {
            if crate::untradable_label(&r.item_name).is_some() || mastery.is_mastered(&r.item_name) {
                continue;
            }
            let built = built_name(&r.item_name);
            if !out.contains(&built) {
                out.push(built);
            }
        }
        out
    }
}

/// Relic drop tables indexed by normalised code for fuzzy OCR matching.
pub struct RelicIndex {
    relics: Vec<RelicInfo>,
    normalized: Vec<String>,
}

impl RelicIndex {
    pub fn new(relics: Vec<RelicInfo>) -> Self {
        let normalized = relics.iter().map(|r| normalize(&r.display)).collect();
        Self { relics, normalized }
    }

    pub fn len(&self) -> usize {
        self.relics.len()
    }

    pub fn is_empty(&self) -> bool {
        self.relics.is_empty()
    }

    pub fn all(&self) -> &[RelicInfo] {
        &self.relics
    }

    /// Fetch + cache the drop tables (weekly TTL, stale-served on failure),
    /// mirroring [`crate::ItemIndex::load_cached`].
    pub async fn load_cached(client: &reqwest::Client, ttl: Duration) -> anyhow::Result<Self> {
        if let Some(cached) = wf_cache::load_blob::<Vec<RelicInfo>>(CACHE_FILE) {
            if cached.age() < ttl {
                tracing::info!("relic tables from cache ({} relics)", cached.value.len());
                return Ok(Self::new(cached.value));
            }
            match fetch(client).await {
                Ok(relics) => {
                    let _ = wf_cache::save_blob(CACHE_FILE, &relics);
                    return Ok(Self::new(relics));
                }
                Err(e) => {
                    tracing::warn!("relic table refresh failed ({e}); using stale cache");
                    return Ok(Self::new(cached.value));
                }
            }
        }
        let relics = fetch(client).await?;
        let _ = wf_cache::save_blob(CACHE_FILE, &relics);
        Ok(Self::new(relics))
    }

    /// Best fuzzy match for an OCR'd relic label (e.g. `"AXI H3"`). Relic codes are
    /// short and distinct, so this matches confidently; returns `None` below 0.8.
    pub fn best_match(&self, query: &str) -> Option<&RelicInfo> {
        let q = normalize(query);
        if q.is_empty() {
            return None;
        }
        let qb = q.as_bytes();
        let mut best: Option<(usize, usize)> = None;
        for (i, code) in self.normalized.iter().enumerate() {
            let d = levenshtein(qb, code.as_bytes());
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
                if d == 0 {
                    break;
                }
            }
        }
        let (idx, dist) = best?;
        let longest = self.normalized[idx].len().max(q.len()).max(1);
        if 1.0 - dist as f32 / longest as f32 >= 0.8 {
            Some(&self.relics[idx])
        } else {
            None
        }
    }
}

/// A ranked owned-relic recommendation for the guide.
#[derive(Debug, Clone)]
pub struct RelicPick {
    /// Relic label, e.g. "Axi H3".
    pub display: String,
    /// How many the player owns.
    pub count: u32,
    /// Distinct unmastered built primes this relic can drop.
    pub unmastered: Vec<String>,
    /// Lowest market sell price in platinum, if resolved.
    pub plat: Option<u32>,
}

/// Rank picks by value: highest plat first, then most unmastered rewards.
pub fn rank(picks: &mut [RelicPick]) {
    picks.sort_by(|a, b| {
        b.plat
            .unwrap_or(0)
            .cmp(&a.plat.unwrap_or(0))
            .then(b.unmastered.len().cmp(&a.unmastered.len()))
    });
}

/// Fetch and parse the WFCD relic drop tables, keeping only the Intact state
/// (the reward *set* is state-independent; only drop chances differ).
async fn fetch(client: &reqwest::Client) -> anyhow::Result<Vec<RelicInfo>> {
    #[derive(Deserialize)]
    struct File {
        relics: Vec<Raw>,
    }
    #[derive(Deserialize)]
    struct Raw {
        tier: String,
        // A rare malformed WFCD entry omits relicName; default + filter it out.
        #[serde(rename = "relicName", default)]
        relic_name: String,
        #[serde(default)]
        state: String,
        #[serde(default)]
        rewards: Vec<RelicReward>,
    }

    tracing::debug!("GET {RELICS_URL}");
    let file: File = client
        .get(RELICS_URL)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let relics = file
        .relics
        .into_iter()
        .filter(|r| r.state.eq_ignore_ascii_case("Intact") && !r.relic_name.is_empty())
        .map(|r| RelicInfo {
            display: format!("{} {}", r.tier, r.relic_name),
            tier: r.tier,
            code: r.relic_name,
            rewards: r.rewards,
        })
        .collect();
    Ok(relics)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relic(display: &str, rewards: &[&str]) -> RelicInfo {
        let (tier, code) = display.split_once(' ').unwrap();
        RelicInfo {
            tier: tier.to_string(),
            code: code.to_string(),
            display: display.to_string(),
            rewards: rewards
                .iter()
                .map(|n| RelicReward { item_name: n.to_string(), rarity: String::new() })
                .collect(),
        }
    }

    #[test]
    fn slug_is_market_form() {
        assert_eq!(relic("Axi H3", &[]).slug(), "axi_h3_relic");
        assert_eq!(relic("Requiem I", &[]).slug(), "requiem_i_relic");
    }

    #[test]
    fn unmastered_dedups_by_built_prime_and_skips_forma() {
        let mastery = MasterySet::from_xp([
            ("/Lotus/Powersuits/Ember/EmberPrime".to_string(), 9_000_000), // mastered
        ]);
        let r = relic(
            "Meso E1",
            &[
                "Ember Prime Blueprint",         // mastered → excluded
                "Ember Prime Systems Blueprint", // mastered → excluded
                "Trinity Prime Systems Blueprint",
                "Trinity Prime Blueprint", // same built prime → dedup
                "Forma Blueprint",         // untradable → skip
            ],
        );
        assert_eq!(r.unmastered(&mastery), vec!["Trinity Prime".to_string()]);
    }

    #[test]
    fn best_match_tolerates_ocr_and_rejects_garbage() {
        let idx = RelicIndex::new(vec![relic("Axi H3", &[]), relic("Meso N11", &[])]);
        assert_eq!(idx.best_match("AXI H3").map(|r| r.display.as_str()), Some("Axi H3"));
        assert_eq!(idx.best_match("MES0 N11").map(|r| r.display.as_str()), Some("Meso N11"));
        assert!(idx.best_match("zzzqwx").is_none());
    }

    #[test]
    fn rank_orders_by_plat_then_unmastered() {
        let mut picks = vec![
            RelicPick { display: "A".into(), count: 1, unmastered: vec!["x".into()], plat: Some(10) },
            RelicPick { display: "B".into(), count: 1, unmastered: vec!["x".into(), "y".into()], plat: Some(25) },
            RelicPick { display: "C".into(), count: 1, unmastered: vec!["x".into(), "y".into()], plat: Some(25) },
        ];
        rank(&mut picks);
        assert_eq!(picks[0].display, "B"); // 25p, 2 unmastered (stable vs C)
        assert_eq!(picks[2].display, "A"); // lowest plat last
    }
}
