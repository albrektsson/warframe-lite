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
    /// mastered (skips Forma/untradable and non-prime rewards — Requiem relics
    /// drop Requiem Mods, Ayatan Sculptures, and Riven Slivers, none of which
    /// mastery applies to, so they'd otherwise always look "unmastered"), e.g.
    /// `["Ember Prime", "Trinity Prime"]`.
    pub fn unmastered(&self, mastery: &MasterySet) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in &self.rewards {
            if crate::untradable_label(&r.item_name).is_some()
                || !is_prime_reward(&r.item_name)
                || mastery.is_mastered(&r.item_name)
            {
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

/// Whether a relic reward name is an actual prime part — as opposed to a
/// Requiem relic's Requiem Mod, Ayatan Sculpture, or Riven Sliver, which mastery
/// (a weapon/Warframe-only concept) never applies to.
fn is_prime_reward(item_name: &str) -> bool {
    item_name.to_ascii_lowercase().contains("prime")
}

/// One entry in the full Mastery browser: a built prime and whether it's been
/// mastered yet.
#[derive(Debug, Clone)]
pub struct MasteryEntry {
    /// Built prime name, e.g. "Ember Prime".
    pub prime: String,
    /// Whether `prime` has been mastered.
    pub mastered: bool,
}

/// Every distinct built prime across the relic catalogue's reward tables,
/// flagged mastered/Unmastered — the universe for the Mastery browser.
/// Every Prime currently drops from some Relic, so no catalogue beyond the one
/// already loaded for relic browsing is needed. Sorted alphabetically.
///
/// Checks mastery against the already-built prime name (e.g. "Ember Prime")
/// rather than the raw reward name `unmastered` uses (e.g. "Ember Prime
/// Systems Blueprint") — safe because `MasterySet::is_mastered`'s own
/// `reward_core` normalisation strips "Prime" and component words either way,
/// so both call shapes reduce to the same core token.
pub fn mastery_browser(index: &RelicIndex, mastery: &MasterySet) -> Vec<MasteryEntry> {
    let mut primes: Vec<String> = Vec::new();
    for relic in index.all() {
        for r in &relic.rewards {
            if crate::untradable_label(&r.item_name).is_some() || !is_prime_reward(&r.item_name) {
                continue;
            }
            let built = built_name(&r.item_name);
            if !primes.contains(&built) {
                primes.push(built);
            }
        }
    }
    primes.sort();
    primes
        .into_iter()
        .map(|prime| {
            let mastered = mastery.is_mastered(&prime);
            MasteryEntry { prime, mastered }
        })
        .collect()
}

/// One owned relic that can still drop a given unmastered prime.
#[derive(Debug, Clone)]
pub struct PrimeRelicSource {
    /// Relic label, e.g. "Axi H3".
    pub relic_display: String,
    /// How many the player owns.
    pub owned_count: u32,
    /// Drop rarity of this prime within this relic (Common/Uncommon/Rare).
    pub rarity: String,
}

/// An unmastered prime and the owned relics that can still drop it — the basis
/// for deciding which fissures to prioritise.
#[derive(Debug, Clone)]
pub struct PrimePlan {
    /// Built prime name, e.g. "Rubico Prime".
    pub prime: String,
    /// Owned relics that can drop it, most-owned first.
    pub relics: Vec<PrimeRelicSource>,
    /// Sum of owned counts across all sourcing relics — a rough farming budget.
    pub total_owned: u32,
}

/// Build a fissure-planning view from an owned-relic count map (relic display →
/// count): for every unmastered prime the player's relics can still drop, which
/// relics (and how many of each) can drop it. Ranked by `total_owned` descending,
/// so the primes with the most farming budget already in hand come first.
pub fn mastery_plan(
    owned: &std::collections::HashMap<String, u32>,
    index: &RelicIndex,
    mastery: &MasterySet,
) -> Vec<PrimePlan> {
    let mut by_prime: std::collections::HashMap<String, Vec<PrimeRelicSource>> =
        std::collections::HashMap::new();
    for relic in index.all() {
        let Some(&count) = owned.get(&relic.display) else {
            continue;
        };
        if count == 0 {
            continue;
        }
        for prime in relic.unmastered(mastery) {
            let rarity = relic
                .rewards
                .iter()
                .find(|r| built_name(&r.item_name) == prime)
                .map(|r| r.rarity.clone())
                .unwrap_or_default();
            by_prime.entry(prime).or_default().push(PrimeRelicSource {
                relic_display: relic.display.clone(),
                owned_count: count,
                rarity,
            });
        }
    }
    let mut plans: Vec<PrimePlan> = by_prime
        .into_iter()
        .map(|(prime, mut relics)| {
            relics.sort_by(|a, b| {
                b.owned_count.cmp(&a.owned_count).then_with(|| a.relic_display.cmp(&b.relic_display))
            });
            let total_owned = relics.iter().map(|r| r.owned_count).sum();
            PrimePlan { prime, relics, total_owned }
        })
        .collect();
    plans.sort_by(|a, b| b.total_owned.cmp(&a.total_owned).then_with(|| a.prime.cmp(&b.prime)));
    plans
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
    fn unmastered_excludes_non_prime_rewards() {
        // Requiem relics drop Requiem Mods / Ayatan Sculptures / Riven Slivers —
        // mastery never applies to these, so they must never appear as
        // "unmastered" (they'd otherwise always look needed).
        let mastery = MasterySet::default();
        let r = relic("Requiem I", &["Xata", "Ayatan Amber Star", "Riven Sliver", "Lohk Prime"]);
        assert_eq!(r.unmastered(&mastery), vec!["Lohk Prime".to_string()]);
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

    #[test]
    fn mastery_browser_dedups_across_relics_and_flags_mastered() {
        let idx = RelicIndex::new(vec![
            relic(
                "Meso E1",
                &["Ember Prime Blueprint", "Ember Prime Systems Blueprint", "Trinity Prime Blueprint"],
            ),
            relic("Lith G4", &["Ember Prime Chassis Blueprint"]), // same built prime, elsewhere
        ]);
        let mastery = MasterySet::from_xp([
            ("/Lotus/Powersuits/Ember/EmberPrime".to_string(), 9_000_000), // mastered
        ]);
        let entries = mastery_browser(&idx, &mastery);

        // Ember Prime appears once despite three reward rows across two relics.
        assert_eq!(entries.len(), 2);
        let ember = entries.iter().find(|e| e.prime == "Ember Prime").unwrap();
        assert!(ember.mastered);
        let trinity = entries.iter().find(|e| e.prime == "Trinity Prime").unwrap();
        assert!(!trinity.mastered);
    }

    #[test]
    fn mastery_browser_excludes_non_prime_rewards() {
        // Requiem relics drop Requiem Mods / Ayatan Sculptures / Riven Slivers —
        // none of these are primes, so they must never appear in the browser.
        let idx = RelicIndex::new(vec![relic(
            "Requiem I",
            &["Xata", "Ayatan Amber Star", "Riven Sliver", "Lohk Prime"],
        )]);
        let entries = mastery_browser(&idx, &MasterySet::default());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].prime, "Lohk Prime");
        assert!(!entries[0].mastered);
    }

    #[test]
    fn mastery_browser_sorts_alphabetically() {
        let idx = RelicIndex::new(vec![relic("Axi A1", &["Volt Prime Blueprint", "Ash Prime Blueprint"])]);
        let entries = mastery_browser(&idx, &MasterySet::default());
        assert_eq!(
            entries.iter().map(|e| e.prime.as_str()).collect::<Vec<_>>(),
            vec!["Ash Prime", "Volt Prime"]
        );
    }

    #[test]
    fn mastery_plan_groups_by_prime_across_relics() {
        let idx = RelicIndex::new(vec![
            relic("Axi A1", &["Akstiletto Prime Barrel", "Trinity Prime Systems Blueprint"]),
            relic("Meso N11", &["Akstiletto Prime Receiver", "Volt Prime Blueprint"]),
            relic("Lith G4", &["Ember Prime Blueprint"]), // fully mastered relic
        ]);
        let mastery =
            MasterySet::from_xp([("/Lotus/Powersuits/Ember/EmberPrime".to_string(), 9_000_000)]);
        let owned = std::collections::HashMap::from([
            ("Axi A1".to_string(), 5),
            ("Meso N11".to_string(), 2),
            ("Lith G4".to_string(), 9),  // owned but nothing unmastered → contributes nothing
            ("Neo V9".to_string(), 3),   // owned but not in the index → ignored
        ]);
        let plans = mastery_plan(&owned, &idx, &mastery);

        // Akstiletto Prime is sourced from two owned relics: 5 + 2 = 7, and ranks
        // first (highest total_owned).
        let aksti = plans.iter().find(|p| p.prime == "Akstiletto Prime").unwrap();
        assert_eq!(aksti.total_owned, 7);
        assert_eq!(aksti.relics.len(), 2);
        assert_eq!(aksti.relics[0].relic_display, "Axi A1"); // most-owned first
        assert_eq!(plans[0].prime, "Akstiletto Prime");

        // Single-relic primes are present with their own relic's count.
        assert!(plans.iter().any(|p| p.prime == "Trinity Prime" && p.total_owned == 5));
        assert!(plans.iter().any(|p| p.prime == "Volt Prime" && p.total_owned == 2));

        // A fully-mastered relic (Ember Prime) contributes no plan entries at all.
        assert!(!plans.iter().any(|p| p.prime == "Ember Prime"));
    }
}
