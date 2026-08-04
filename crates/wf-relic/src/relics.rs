//! Owned-relic guide: which relics contain rewards the player hasn't mastered.
//!
//! Relic → reward drop tables come from WFCD `warframe-drop-data` (readable item
//! names + rarity), cached to disk. Cross-referenced with the mastered set
//! ([`MasterySet`]) and warframe.market relic prices, this ranks the relics worth
//! opening for mastery. Ownership itself is supplied by the caller (from OCR of
//! the in-game Relics screen); this module is network/screen-agnostic and pure.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::index::{levenshtein, normalize};
use crate::mastery::{built_name, prime_part, MasterySet, PrimePart};
use crate::part_quantities::PartQuantities;

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
        for r in unmastered_rewards(self, mastery) {
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

    /// Best fuzzy match for an OCR'd relic label (e.g. `"AXI H3"`). Relic codes
    /// are short, so a garbled read can land exactly as close to two different
    /// real codes (e.g. a dropped letter leaving `"meso1"` equidistant from both
    /// `"Meso I1"` and `"Meso A1"`) — such a tie abstains (`None`) rather than
    /// arbitrarily picking one, the same way a below-threshold score does.
    pub fn best_match(&self, query: &str) -> Option<&RelicInfo> {
        let q = normalize(query);
        if q.is_empty() {
            return None;
        }
        let qb = q.as_bytes();
        let mut best: Option<(usize, usize)> = None;
        let mut tied = false;
        for (i, code) in self.normalized.iter().enumerate() {
            let d = levenshtein(qb, code.as_bytes());
            match best {
                Some((_, bd)) if d < bd => {
                    best = Some((i, d));
                    tied = false;
                    if d == 0 {
                        break;
                    }
                }
                Some((_, bd)) if d == bd => tied = true,
                Some(_) => {}
                None => best = Some((i, d)),
            }
        }
        if tied {
            return None;
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

/// The file `owned-relics.json` is cached under, via `wf_cache::load_blob`/
/// `save_blob` — the OCR scanner's only persisted record of Owned relic
/// counts (ADR-0001, ADR-0003). Shared so every consumer (the CLI's scanner
/// and reader, `wf-browse`) names the same file.
pub const OWNED_RELICS_FILE: &str = "owned-relics.json";

/// The era prefix of a relic display label, e.g. `"Axi H3"` → `"Axi"`.
pub fn tier_of(relic_display: &str) -> &str {
    relic_display.split_whitespace().next().unwrap_or("")
}

/// Every owned relic — mastered or not — as a priced, ranked [`RelicPick`],
/// for the Sell tab: deciding which relics are worth selling rather than
/// cracking. Unlike [`mastery_plan`] (and the full-catalogue guide behind
/// `relics_cmd`), relics with zero Unmastered rewards are kept here — a
/// fully-mastered relic is the clearest "sell, don't crack" case.
///
/// Prices come from a caller-supplied map (relic market slug →
/// already-resolved plat, `None` where unresolved) rather than being fetched
/// inline, keeping this pure; a slug missing from the map is treated the same
/// as an explicit `None` — the relic still appears, unpriced.
pub fn sell_picks(
    owned: &HashMap<String, u32>,
    prices: &HashMap<String, Option<u32>>,
    index: &RelicIndex,
    mastery: &MasterySet,
) -> Vec<RelicPick> {
    let mut picks: Vec<RelicPick> = index
        .all()
        .iter()
        .filter_map(|relic| {
            let count = *owned.get(&relic.display)?;
            if count == 0 {
                return None;
            }
            let plat = prices.get(&relic.slug()).copied().flatten();
            Some(RelicPick {
                display: relic.display.clone(),
                count,
                unmastered: relic.unmastered(mastery),
                plat,
            })
        })
        .collect();
    rank(&mut picks);
    picks
}

/// Whether a relic reward name is an actual prime part — as opposed to a
/// Requiem relic's Requiem Mod, Ayatan Sculpture, or Riven Sliver, which mastery
/// (a weapon/Warframe-only concept) never applies to.
fn is_prime_reward(item_name: &str) -> bool {
    item_name.to_ascii_lowercase().contains("prime")
}

/// A relic's rewards that are real, still-unmastered prime parts — the shared
/// filter behind [`RelicInfo::unmastered`] and [`mastery_plan`], which both
/// need it (the former collapsed to built-prime names, the latter kept at
/// per-reward granularity to also resolve each one's [`PrimePart`]).
fn unmastered_rewards<'a>(
    relic: &'a RelicInfo,
    mastery: &'a MasterySet,
) -> impl Iterator<Item = &'a RelicReward> {
    relic.rewards.iter().filter(move |r| {
        crate::untradable_label(&r.item_name).is_none()
            && is_prime_reward(&r.item_name)
            && !mastery.is_mastered(&r.item_name)
    })
}

/// A relic's rewards that are real prime parts the player has already
/// mastered — the Farm tab's candidate pool, the mirror image of
/// [`unmastered_rewards`].
fn mastered_rewards<'a>(
    relic: &'a RelicInfo,
    mastery: &'a MasterySet,
) -> impl Iterator<Item = &'a RelicReward> {
    relic.rewards.iter().filter(move |r| {
        crate::untradable_label(&r.item_name).is_none()
            && is_prime_reward(&r.item_name)
            && mastery.is_mastered(&r.item_name)
    })
}

/// One owned relic's most profitable already-mastered drop: crack the relic
/// and sell this specific part instead of selling the relic itself — and,
/// since it names the *one* best reward, a natural relic to organize a
/// 4-player "radiant share" around to maximize the odds of rolling it.
#[derive(Debug, Clone)]
pub struct FarmPick {
    /// Relic label, e.g. "Axi H3".
    pub display: String,
    /// How many the player owns.
    pub count: u32,
    /// The already-mastered reward name this relic can drop with the highest
    /// resolved market price, e.g. "Atlas Prime Neuroptics Blueprint".
    pub best_reward: String,
    /// Market sell price in platinum of `best_reward`, if resolved.
    pub plat: Option<u32>,
    /// Drop rarity of `best_reward` within this relic (Common/Uncommon/Rare).
    pub rarity: String,
}

/// The distinct already-mastered prime reward names among the player's owned
/// relics — the set of item names the Farm tab needs a market price for.
/// Kept separate from [`farm_picks`] so pricing (a network concern) happens
/// once at the caller before ranking, mirroring [`sell_picks`]'s split from
/// its caller's price-fetch loop.
pub fn farm_reward_names(
    owned: &HashMap<String, u32>,
    index: &RelicIndex,
    mastery: &MasterySet,
) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for relic in index.all() {
        if owned.get(&relic.display).copied().unwrap_or(0) == 0 {
            continue;
        }
        for r in mastered_rewards(relic, mastery) {
            if !names.contains(&r.item_name) {
                names.push(r.item_name.clone());
            }
        }
    }
    names
}

/// For every owned relic with at least one already-mastered prime reward, the
/// single highest-value pick — the basis of the Farm tab. Ranked by that
/// reward's price, highest first.
///
/// Prices come from a caller-supplied map (reward item name →
/// already-resolved plat, `None` where unresolved) rather than being fetched
/// inline, keeping this pure; a name missing from the map is treated the same
/// as an explicit `None`.
pub fn farm_picks(
    owned: &HashMap<String, u32>,
    prices: &HashMap<String, Option<u32>>,
    index: &RelicIndex,
    mastery: &MasterySet,
) -> Vec<FarmPick> {
    let mut picks: Vec<FarmPick> = index
        .all()
        .iter()
        .filter_map(|relic| {
            let count = *owned.get(&relic.display)?;
            if count == 0 {
                return None;
            }
            let best = mastered_rewards(relic, mastery)
                .max_by_key(|r| prices.get(&r.item_name).copied().flatten().unwrap_or(0))?;
            Some(FarmPick {
                display: relic.display.clone(),
                count,
                best_reward: best.item_name.clone(),
                plat: prices.get(&best.item_name).copied().flatten(),
                rarity: best.rarity.clone(),
            })
        })
        .collect();
    picks.sort_by_key(|p| std::cmp::Reverse(p.plat.unwrap_or(0)));
    picks
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

/// One owned relic that can still drop a given [`PrimePart`].
#[derive(Debug, Clone)]
pub struct PrimeRelicSource {
    /// Relic label, e.g. "Axi H3".
    pub relic_display: String,
    /// How many the player owns.
    pub owned_count: u32,
    /// Drop rarity of this part within this relic (Common/Uncommon/Rare).
    pub rarity: String,
}

/// One Prime Part of an unmastered prime, and the owned relics that can still
/// drop it.
#[derive(Debug, Clone)]
pub struct PrimePartGroup {
    pub part: PrimePart,
    /// How many `part.part` a full build needs, if known — never guessed when
    /// unknown (see ADR-0011).
    pub build_quantity: Option<u32>,
    /// Owned relics that can drop this part, most-owned first.
    pub relics: Vec<PrimeRelicSource>,
}

/// An unmastered prime, broken down by Prime Part, and the owned relics that
/// can still drop each — the basis for deciding which fissures to prioritise.
#[derive(Debug, Clone)]
pub struct PrimePlan {
    /// Built prime name, e.g. "Rubico Prime".
    pub prime: String,
    /// This prime's parts, each with their own sourcing relics.
    pub parts: Vec<PrimePartGroup>,
    /// Sum of owned counts across all distinct sourcing relics (a relic
    /// counted once even if it drops more than one of this prime's parts) —
    /// a rough farming budget.
    pub total_owned: u32,
}

/// Build a fissure-planning view from an owned-relic count map (relic display →
/// count): for every unmastered prime the player's relics can still drop, broken
/// down by Prime Part, which relics (and how many of each) can drop it. Ranked
/// by `total_owned` descending, so the primes with the most farming budget
/// already in hand come first; parts within a prime are ranked the same way.
pub fn mastery_plan(
    owned: &std::collections::HashMap<String, u32>,
    index: &RelicIndex,
    mastery: &MasterySet,
    quantities: &PartQuantities,
) -> Vec<PrimePlan> {
    let mut by_prime: std::collections::HashMap<String, std::collections::HashMap<PrimePart, Vec<PrimeRelicSource>>> =
        std::collections::HashMap::new();
    // Dedup a relic's contribution to a prime's `total_owned` even when it
    // drops more than one of that prime's parts.
    let mut prime_relic_counts: std::collections::HashMap<String, std::collections::HashMap<String, u32>> =
        std::collections::HashMap::new();

    for relic in index.all() {
        let Some(&count) = owned.get(&relic.display) else {
            continue;
        };
        if count == 0 {
            continue;
        }
        let mut seen: Vec<PrimePart> = Vec::new();
        for r in unmastered_rewards(relic, mastery) {
            let pp = prime_part(&r.item_name);
            if seen.contains(&pp) {
                continue;
            }
            seen.push(pp.clone());
            by_prime.entry(pp.prime.clone()).or_default().entry(pp.clone()).or_default().push(
                PrimeRelicSource {
                    relic_display: relic.display.clone(),
                    owned_count: count,
                    rarity: r.rarity.clone(),
                },
            );
            prime_relic_counts.entry(pp.prime).or_default().insert(relic.display.clone(), count);
        }
    }

    let mut plans: Vec<PrimePlan> = by_prime
        .into_iter()
        .map(|(prime, part_map)| {
            let mut parts: Vec<PrimePartGroup> = part_map
                .into_iter()
                .map(|(pp, mut relics)| {
                    relics.sort_by(|a, b| {
                        b.owned_count
                            .cmp(&a.owned_count)
                            .then_with(|| a.relic_display.cmp(&b.relic_display))
                    });
                    let build_quantity = quantities.get(&pp);
                    PrimePartGroup { part: pp, build_quantity, relics }
                })
                .collect();
            parts.sort_by(|a, b| a.part.part.cmp(&b.part.part));
            let total_owned = prime_relic_counts.get(&prime).map(|m| m.values().sum()).unwrap_or(0);
            PrimePlan { prime, parts, total_owned }
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
    use std::collections::HashMap;

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

    fn relic_with_rarity(display: &str, rewards: &[(&str, &str)]) -> RelicInfo {
        let (tier, code) = display.split_once(' ').unwrap();
        RelicInfo {
            tier: tier.to_string(),
            code: code.to_string(),
            display: display.to_string(),
            rewards: rewards
                .iter()
                .map(|(n, r)| RelicReward { item_name: n.to_string(), rarity: r.to_string() })
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
    fn best_match_abstains_on_a_tie_instead_of_guessing() {
        // A dropped letter ("Meso I1" -> "meso1") lands exactly one edit from both
        // real codes below — must abstain rather than arbitrarily pick one.
        let idx = RelicIndex::new(vec![relic("Meso I1", &[]), relic("Meso A1", &[])]);
        assert!(idx.best_match("Meso 1").is_none());
        // An unambiguous read still resolves normally.
        assert_eq!(idx.best_match("Meso I1").map(|r| r.display.as_str()), Some("Meso I1"));
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
    fn sell_picks_includes_fully_mastered_relics() {
        // Meso E1's only reward is mastered, so it has zero unmastered rewards
        // — mastery_plan/relics_cmd would hide it, but the Sell tab must show
        // it: a fully-mastered relic is the clearest "sell, don't crack" case.
        let idx = RelicIndex::new(vec![relic("Meso E1", &["Ember Prime Blueprint"])]);
        let mastery = MasterySet::from_xp([
            ("/Lotus/Powersuits/Ember/EmberPrime".to_string(), 9_000_000),
        ]);
        let owned = HashMap::from([("Meso E1".to_string(), 3)]);
        let prices = HashMap::from([("meso_e1_relic".to_string(), Some(12))]);

        let picks = sell_picks(&owned, &prices, &idx, &mastery);

        assert_eq!(picks.len(), 1);
        assert_eq!(picks[0].display, "Meso E1");
        assert_eq!(picks[0].count, 3);
        assert!(picks[0].unmastered.is_empty());
        assert_eq!(picks[0].plat, Some(12));
    }

    #[test]
    fn sell_picks_unresolved_price_is_none_not_dropped() {
        let idx = RelicIndex::new(vec![relic("Axi H3", &["Volt Prime Blueprint"])]);
        let owned = HashMap::from([("Axi H3".to_string(), 1)]);
        let prices = HashMap::new(); // never resolved (timeout/error)

        let picks = sell_picks(&owned, &prices, &idx, &MasterySet::default());

        assert_eq!(picks.len(), 1);
        assert_eq!(picks[0].plat, None);
    }

    #[test]
    fn sell_picks_sorted_by_price_descending_and_excludes_unowned() {
        let idx = RelicIndex::new(vec![
            relic("Axi A1", &["Volt Prime Blueprint"]),
            relic("Meso B2", &["Rhino Prime Blueprint"]),
            relic("Lith C3", &["Loki Prime Blueprint"]), // not owned — must be excluded
        ]);
        let owned = HashMap::from([
            ("Axi A1".to_string(), 2),
            ("Meso B2".to_string(), 1),
            ("Lith C3".to_string(), 0), // owned key present but zero count — excluded too
        ]);
        let prices = HashMap::from([
            ("axi_a1_relic".to_string(), Some(15)),
            ("meso_b2_relic".to_string(), Some(40)),
        ]);

        let picks = sell_picks(&owned, &prices, &idx, &MasterySet::default());

        assert_eq!(picks.iter().map(|p| p.display.as_str()).collect::<Vec<_>>(), vec!["Meso B2", "Axi A1"]);
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
    fn mastery_plan_groups_by_prime_and_part_across_relics() {
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
        let plans = mastery_plan(&owned, &idx, &mastery, &PartQuantities::empty());

        // Akstiletto Prime's Barrel and Receiver are two different parts, each
        // sourced from a different owned relic: 5 + 2 = 7 total_owned, and it
        // ranks first (highest total_owned).
        let aksti = plans.iter().find(|p| p.prime == "Akstiletto Prime").unwrap();
        assert_eq!(aksti.total_owned, 7);
        assert_eq!(aksti.parts.len(), 2);
        let barrel = aksti.parts.iter().find(|g| g.part.part == "Barrel").unwrap();
        assert_eq!(barrel.relics.len(), 1);
        assert_eq!(barrel.relics[0].relic_display, "Axi A1");
        assert_eq!(barrel.relics[0].owned_count, 5);
        let receiver = aksti.parts.iter().find(|g| g.part.part == "Receiver").unwrap();
        assert_eq!(receiver.relics[0].relic_display, "Meso N11");
        assert_eq!(plans[0].prime, "Akstiletto Prime");

        // Single-relic primes are present with their own relic's count.
        assert!(plans.iter().any(|p| p.prime == "Trinity Prime" && p.total_owned == 5));
        assert!(plans.iter().any(|p| p.prime == "Volt Prime" && p.total_owned == 2));

        // A fully-mastered relic (Ember Prime) contributes no plan entries at all.
        assert!(!plans.iter().any(|p| p.prime == "Ember Prime"));
    }

    #[test]
    fn mastery_plan_dedups_a_relics_owned_count_across_its_own_multiple_parts() {
        // One relic dropping two different parts of the same prime must only
        // count its owned copies once toward that prime's total_owned, not
        // once per part.
        let idx = RelicIndex::new(vec![relic(
            "Axi B2",
            &["Loki Prime Systems Blueprint", "Loki Prime Chassis Blueprint"],
        )]);
        let owned = std::collections::HashMap::from([("Axi B2".to_string(), 4)]);

        let plans = mastery_plan(&owned, &idx, &MasterySet::default(), &PartQuantities::empty());

        let loki = plans.iter().find(|p| p.prime == "Loki Prime").unwrap();
        assert_eq!(loki.total_owned, 4);
        assert_eq!(loki.parts.len(), 2);
        assert!(loki.parts.iter().all(|g| g.relics.len() == 1 && g.relics[0].owned_count == 4));
    }

    #[test]
    fn mastery_plan_carries_build_quantity_when_known_and_none_when_unknown() {
        let idx = RelicIndex::new(vec![relic("Axi C3", &["Afuris Prime Barrel", "Afuris Prime Link"])]);
        let owned = std::collections::HashMap::from([("Axi C3".to_string(), 1)]);
        let quantities = PartQuantities::from_entries_for_test(vec![(
            "Afuris Prime".to_string(),
            "Barrel".to_string(),
            2,
        )]);

        let plans = mastery_plan(&owned, &idx, &MasterySet::default(), &quantities);

        let afuris = plans.iter().find(|p| p.prime == "Afuris Prime").unwrap();
        let barrel = afuris.parts.iter().find(|g| g.part.part == "Barrel").unwrap();
        assert_eq!(barrel.build_quantity, Some(2));
        let link = afuris.parts.iter().find(|g| g.part.part == "Link").unwrap();
        assert_eq!(link.build_quantity, None); // not in the lookup — never guessed at 1
    }

    #[test]
    fn farm_reward_names_lists_distinct_mastered_prime_rewards_of_owned_relics() {
        let mastery = MasterySet::from_xp([
            ("/Lotus/Powersuits/Ember/EmberPrime".to_string(), 9_000_000), // mastered
        ]);
        let idx = RelicIndex::new(vec![
            relic(
                "Meso E1",
                &["Ember Prime Blueprint", "Ember Prime Systems Blueprint", "Trinity Prime Blueprint"],
            ),
            relic("Axi A1", &["Volt Prime Blueprint"]), // not owned
        ]);
        let owned = HashMap::from([("Meso E1".to_string(), 2)]);

        let names = farm_reward_names(&owned, &idx, &mastery);

        assert_eq!(names.len(), 2);
        assert!(names.contains(&"Ember Prime Blueprint".to_string()));
        assert!(names.contains(&"Ember Prime Systems Blueprint".to_string()));
        assert!(!names.iter().any(|n| n.contains("Trinity"))); // not mastered
        assert!(!names.iter().any(|n| n.contains("Volt"))); // not owned
    }

    #[test]
    fn farm_picks_selects_the_highest_priced_mastered_reward_per_relic() {
        let mastery = MasterySet::from_xp([
            ("/Lotus/Powersuits/Ember/EmberPrime".to_string(), 9_000_000),
            ("/Lotus/Powersuits/Trinity/TrinityPrime".to_string(), 9_000_000),
        ]);
        let idx = RelicIndex::new(vec![relic_with_rarity(
            "Meso E1",
            &[
                ("Ember Prime Blueprint", "Common"),
                ("Ember Prime Systems Blueprint", "Uncommon"),
                ("Trinity Prime Blueprint", "Rare"),
            ],
        )]);
        let owned = HashMap::from([("Meso E1".to_string(), 3)]);
        let prices = HashMap::from([
            ("Ember Prime Blueprint".to_string(), Some(5)),
            ("Ember Prime Systems Blueprint".to_string(), Some(40)),
            ("Trinity Prime Blueprint".to_string(), Some(20)),
        ]);

        let picks = farm_picks(&owned, &prices, &idx, &mastery);

        assert_eq!(picks.len(), 1);
        assert_eq!(picks[0].display, "Meso E1");
        assert_eq!(picks[0].count, 3);
        assert_eq!(picks[0].best_reward, "Ember Prime Systems Blueprint");
        assert_eq!(picks[0].plat, Some(40));
        assert_eq!(picks[0].rarity, "Uncommon");
    }

    #[test]
    fn farm_picks_excludes_relics_with_no_mastered_reward_and_unowned_relics() {
        let idx = RelicIndex::new(vec![
            relic("Axi A1", &["Volt Prime Blueprint"]), // owned, nothing mastered
            relic("Lith C3", &["Loki Prime Blueprint"]), // not owned
        ]);
        let owned = HashMap::from([("Axi A1".to_string(), 1), ("Lith C3".to_string(), 0)]);

        let picks = farm_picks(&owned, &HashMap::new(), &idx, &MasterySet::default());

        assert!(picks.is_empty());
    }

    #[test]
    fn farm_picks_ranked_by_price_descending() {
        let mastery = MasterySet::from_xp([
            ("/Lotus/Powersuits/Ember/EmberPrime".to_string(), 9_000_000),
            ("/Lotus/Powersuits/Volt/VoltPrime".to_string(), 9_000_000),
        ]);
        let idx = RelicIndex::new(vec![
            relic("Meso E1", &["Ember Prime Blueprint"]),
            relic("Axi A1", &["Volt Prime Blueprint"]),
        ]);
        let owned = HashMap::from([("Meso E1".to_string(), 1), ("Axi A1".to_string(), 1)]);
        let prices = HashMap::from([
            ("Ember Prime Blueprint".to_string(), Some(10)),
            ("Volt Prime Blueprint".to_string(), Some(50)),
        ]);

        let picks = farm_picks(&owned, &prices, &idx, &mastery);

        assert_eq!(
            picks.iter().map(|p| p.display.as_str()).collect::<Vec<_>>(),
            vec!["Axi A1", "Meso E1"]
        );
    }

    #[test]
    fn farm_picks_unresolved_price_is_none_not_dropped() {
        let mastery =
            MasterySet::from_xp([("/Lotus/Powersuits/Ember/EmberPrime".to_string(), 9_000_000)]);
        let idx = RelicIndex::new(vec![relic("Meso E1", &["Ember Prime Blueprint"])]);
        let owned = HashMap::from([("Meso E1".to_string(), 1)]);

        let picks = farm_picks(&owned, &HashMap::new(), &idx, &mastery);

        assert_eq!(picks.len(), 1);
        assert_eq!(picks[0].plat, None);
    }
}
