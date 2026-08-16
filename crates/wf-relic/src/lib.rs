//! Relic reward evaluation: turn OCR'd reward names into a ranked, priced list
//! so the overlay can highlight the most valuable pick.

pub mod bom;
pub mod index;
pub mod mastery;
pub mod owned;
pub mod ducats;
pub mod owned_parts;
pub mod owned_rivens;
pub mod part_market;
pub mod part_quantities;
#[cfg(feature = "grid-scan")]
pub mod regions;
pub mod relic_names;
pub mod relics;
pub mod riven_catalogue;
pub mod riven_decode;
pub mod riven_pricing;
pub mod wishlist;

pub use bom::{buy_or_farm_plan, unmastered_primes, BomGap, BomPlan};
pub use index::ItemIndex;
pub use mastery::{inventory_prime_part, owned_part_from_item_type, MasterySet, PrimePart};
pub use owned::{
    apply_confirmed_count, apply_exact_snapshot, clear_entry, count_source, intact_age,
    intact_age_range, intact_ages, mark_seen, owned_counts, owned_evidence, parse_refinement,
    OwnedCount, OwnedEntry, OwnedRelics, RelicEvidence, Refinement, Source, STALE_AFTER,
};
pub use ducats::{ducat_picks, DucatPick};
pub use owned_parts::{OwnedPrimeParts, OWNED_PRIME_PARTS_FILE};
pub use owned_rivens::{apply_exact_snapshot as apply_riven_snapshot, OwnedRivens, OWNED_RIVENS_FILE};
pub use part_market::{reward_label, resolve as part_market_info, PartMarketInfo};
pub use part_quantities::{EquipmentCategory, PartQuantities, CATEGORY_ORDER};
#[cfg(feature = "grid-scan")]
pub use regions::{
    InventoryGridRegions, InventorySlot, Rect, RelicGridRegions, RelicSlot, RewardRegions,
};
pub use relic_names::{RelicIdentity, RelicNameIndex};
pub use relics::{
    active_tier_reward_names, expected_value, farm_picks, farm_reward_names, mastery_browser,
    mastery_plan, rank as rank_relics, sell_picks, tier_of, vaulted_rewards, EvRefinement,
    FarmPick, MasteryEntry, PartsOwnedSummary, PrimePartGroup, PrimePlan, PrimeRelicSource,
    RelicContext, RelicIndex, RelicInfo, RelicPick, RelicReward, OWNED_RELICS_FILE,
};
pub use riven_catalogue::{RivenCatalogue, RivenModCategory, WeaponRivenInfo};
pub use riven_decode::{decode as decode_riven, DecodedRiven, DecodedStat, RawRiven as RivenRawRiven, RawStat as RivenRawStat};
pub use riven_pricing::{evaluate as evaluate_riven_price, ListingInput, RivenTypeVerdict, Verdict as RivenVerdict};
pub use wishlist::{Wishlist, WISHLIST_FILE};

use std::collections::HashMap;
use std::time::Duration;

use wf_data::market::{MarketClient, PriceSummary};
use wf_data::riven_market::RivenMarketClient;

/// A disk-backed cache of per-item price summaries.
pub type PriceCache = wf_cache::KeyedCache<PriceSummary>;

/// Load the shared on-disk price cache every price lookup (relic or reward)
/// reads from and writes back to.
pub fn price_cache() -> PriceCache {
    PriceCache::load("prices.json")
}

/// A disk-backed cache of per-Riven-type Floor/Ceiling/Verdict results,
/// keyed by `weapon_url_name` (the warframe.market riven-weapon slug — see
/// [`wf_data::riven_market`]). Never shares storage with [`OwnedRivens`]'s
/// `rivens.json`: identity/decoded-stat data persists across sessions, but
/// price/Verdict data is explicitly live-only (spec §6) — this cache exists
/// purely for the stale-serves-instantly pattern, not as a source of truth.
pub type RivenPriceCache = wf_cache::KeyedCache<RivenTypeVerdict>;

/// Load the shared on-disk riven-price cache.
pub fn riven_price_cache() -> RivenPriceCache {
    RivenPriceCache::load("riven-prices.json")
}

/// Tuning for cached price lookups.
#[derive(Debug, Clone, Copy)]
pub struct PriceOpts {
    /// Cached prices younger than this are used without any network call.
    pub fresh_ttl: Duration,
    /// Per-request timeout when a live fetch is needed. On timeout/error a stale
    /// cached value is served if available — vital for the short relic-selection
    /// window.
    pub fetch_timeout: Duration,
}

impl Default for PriceOpts {
    fn default() -> Self {
        Self {
            fresh_ttl: Duration::from_secs(30 * 60),
            fetch_timeout: Duration::from_millis(2500),
        }
    }
}

/// One evaluated reward choice from the fissure screen.
#[derive(Debug, Clone)]
pub struct RewardEval {
    /// Raw OCR text for this choice.
    pub ocr: String,
    /// Matched catalogue name, if resolved.
    pub matched_name: Option<String>,
    /// Matched market slug, if resolved.
    pub slug: Option<String>,
    /// Match confidence (1.0 = exact).
    pub score: f32,
    /// Ducat value, if a prime part.
    pub ducats: Option<u32>,
    /// Lowest active sell price in platinum, if resolved.
    pub plat: Option<u32>,
    /// Whether every relic that can drop this reward is itself vaulted (see
    /// [`vaulted_rewards`]).
    pub vaulted: bool,
}

impl RewardEval {
    fn unresolved(ocr: String) -> Self {
        Self {
            ocr,
            matched_name: None,
            slug: None,
            score: 0.0,
            ducats: None,
            plat: None,
            vaulted: false,
        }
    }

    /// A recognized-but-untradable reward (Forma, Kuva, Endo, …): labelled, but
    /// with no market price.
    fn untradable(ocr: String, label: &str) -> Self {
        Self {
            ocr,
            matched_name: Some(label.to_string()),
            slug: None,
            score: 1.0,
            ducats: None,
            plat: None,
            vaulted: false,
        }
    }
}

/// Recognize common relic rewards that are not tradable on warframe.market, so
/// they are labelled instead of fuzzy-matched to a similar tradable item.
fn untradable_label(name: &str) -> Option<&'static str> {
    let n = index::normalize(name);
    // Substring checks tolerate OCR prefixes/suffixes and quantity markers.
    const TABLE: &[(&str, &str)] = &[
        ("forma", "Forma (untradable)"),
        ("kuva", "Kuva (untradable)"),
        ("endo", "Endo (untradable)"),
        ("exilus", "Exilus Adapter (untradable)"),
    ];
    TABLE
        .iter()
        .find(|(needle, _)| n.contains(needle))
        .map(|(_, label)| *label)
}

/// Evaluate the reward choices in screen order: resolve each name to an item and
/// look up its live platinum price and ducat value. Order is preserved so the
/// caller can map results back to on-screen positions.
pub async fn evaluate(
    names: &[String],
    index: &ItemIndex,
    market: &MarketClient,
    vaulted: &HashMap<String, bool>,
) -> Vec<RewardEval> {
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        // Untradable rewards (Forma/Kuva/…) are labelled, not fuzzy-matched.
        if let Some(label) = untradable_label(name) {
            out.push(RewardEval::untradable(name.clone(), label));
            continue;
        }
        match index.best_match(name).filter(|m| is_prime(m.item)) {
            Some(m) => {
                let slug = m.item.slug.clone();
                let plat = match market.price_summary(&slug).await {
                    Ok(s) => s.lowest_sell,
                    Err(e) => {
                        tracing::warn!("price lookup failed for {slug}: {e}");
                        None
                    }
                };
                out.push(RewardEval {
                    ocr: name.clone(),
                    vaulted: vaulted.get(&m.item.name).copied().unwrap_or(false),
                    matched_name: Some(m.item.name.clone()),
                    slug: Some(slug),
                    score: m.score,
                    ducats: m.item.ducats,
                    plat,
                });
            }
            None => out.push(RewardEval::unresolved(name.clone())),
        }
    }
    out
}

/// Like [`evaluate`], but prices come through a disk-backed [`PriceCache`]:
/// fresh cached prices are used instantly, stale/missing ones are fetched
/// concurrently with a short timeout, and on timeout/error a stale value is
/// served — so the panel is ready within the few-second selection window even
/// when warframe.market is slow.
pub async fn evaluate_cached(
    names: &[String],
    index: &ItemIndex,
    market: &MarketClient,
    cache: &PriceCache,
    opts: PriceOpts,
    vaulted: &HashMap<String, bool>,
) -> Vec<RewardEval> {
    let resolutions: Vec<Resolution> = names.iter().map(|n| resolve(n, index)).collect();

    // Price all matched items concurrently.
    let price_futs = resolutions.iter().map(|r| async move {
        match r {
            Resolution::Matched { slug, .. } => cached_price(cache, market, slug, opts).await,
            _ => None,
        }
    });
    let prices = futures::future::join_all(price_futs).await;

    let evals: Vec<RewardEval> = names
        .iter()
        .zip(resolutions)
        .zip(prices)
        .map(|((ocr, res), price)| build_eval(ocr.clone(), res, price, vaulted))
        .collect();

    cache.save(); // persist any newly fetched prices
    evals
}

/// From the OCR of the candidate slots, pick the ones that are real rewards.
///
/// Each slot text is fuzzy-matched; slots that resolve are kept in left-to-right
/// order. Because candidate centres are spaced at half-pitch, a real card can be
/// partially caught by an adjacent slot too — so when two *adjacent* slots both
/// resolve, only the higher-scoring one is kept. The returned OCR strings can be
/// passed to [`evaluate`] / [`evaluate_cached`].
pub fn select_rewards(slot_texts: &[String], index: &ItemIndex) -> Vec<String> {
    let scored: Vec<Option<f32>> = slot_texts
        .iter()
        .map(|t| {
            // Forma/Kuva/Endo/Exilus are real rewards but aren't in the tradable
            // catalogue — recognize them so a mostly-Forma screen still resolves
            // ≥2 slots and triggers.
            if untradable_label(t).is_some() {
                return Some(0.9);
            }
            // A fissure reward is always a Prime part; reject anything else
            // (mods, arcanes, relics, …) so the Void Relics inventory grid, an
            // end-of-mission loot summary, or other on-screen text can't be
            // mistaken for a reward screen.
            index.best_match(t).filter(|m| is_prime(m.item)).map(|m| m.score)
        })
        .collect();

    let mut kept: Vec<usize> = Vec::new();
    for i in 0..slot_texts.len() {
        if scored[i].is_none() {
            continue;
        }
        if let Some(&last) = kept.last() {
            if i == last + 1 {
                // Adjacent conflict: keep the better score, drop the other.
                if scored[i] > scored[last] {
                    kept.pop();
                    kept.push(i);
                }
                continue;
            }
        }
        kept.push(i);
    }
    kept.into_iter().map(|i| slot_texts[i].clone()).collect()
}

/// How a reward name resolved against the catalogue.
enum Resolution {
    Untradable(&'static str),
    Matched {
        name: String,
        slug: String,
        score: f32,
        ducats: Option<u32>,
    },
    Unresolved,
}

/// Whether a catalogue item is tagged `prime` — a fissure reward is always
/// either a Prime part/blueprint or one of the untradables handled separately
/// by [`untradable_label`], so this is the only shape a resolved reward can
/// take. Rejects everything else (mods, arcanes, relics, …) that might
/// otherwise clear the fuzzy-match threshold on noisy OCR text — including
/// "Primed"-named mods, since the check is on the catalogue tag, not the name.
fn is_prime(item: &wf_data::items::Item) -> bool {
    item.tags.iter().any(|t| t == "prime")
}

fn resolve(name: &str, index: &ItemIndex) -> Resolution {
    if let Some(label) = untradable_label(name) {
        return Resolution::Untradable(label);
    }
    match index.best_match(name) {
        Some(m) if is_prime(m.item) => Resolution::Matched {
            name: m.item.name.clone(),
            slug: m.item.slug.clone(),
            score: m.score,
            ducats: m.item.ducats,
        },
        _ => Resolution::Unresolved,
    }
}

fn build_eval(
    ocr: String,
    res: Resolution,
    price: Option<PriceSummary>,
    vaulted: &HashMap<String, bool>,
) -> RewardEval {
    match res {
        Resolution::Untradable(label) => RewardEval::untradable(ocr, label),
        Resolution::Unresolved => RewardEval::unresolved(ocr),
        Resolution::Matched {
            name,
            slug,
            score,
            ducats,
        } => RewardEval {
            vaulted: vaulted.get(&name).copied().unwrap_or(false),
            ocr,
            matched_name: Some(name),
            slug: Some(slug),
            score,
            ducats,
            plat: price.and_then(|p| p.lowest_sell),
        },
    }
}

/// Resolve a price via the cache: fresh → instant; stale/missing → bounded
/// fetch, falling back to any stale value on timeout/error.
async fn cached_price(
    cache: &PriceCache,
    market: &MarketClient,
    slug: &str,
    opts: PriceOpts,
) -> Option<PriceSummary> {
    let stale = cache.get(slug);
    if let Some(s) = &stale {
        if s.age() < opts.fresh_ttl {
            return Some(s.value.clone());
        }
    }
    match tokio::time::timeout(opts.fetch_timeout, market.price_summary(slug)).await {
        Ok(Ok(fresh)) => {
            cache.put(slug, fresh.clone());
            Some(fresh)
        }
        _ => stale.map(|s| s.value), // timeout or error → serve stale if we have it
    }
}

/// Caps how many lookups [`prewarm_reward_prices`] fires at once. Unlike
/// [`evaluate_cached`]'s handful of on-screen rewards (fine to fetch
/// unbounded), a tier's worth of owned relics can be dozens of distinct
/// reward items — an unbounded burst against warframe.market would be
/// inconsiderate, so this bounds it the same way the `wf-browse` GUI already
/// bounds its own bulk price warm-up.
const PREWARM_CONCURRENCY: usize = 8;

/// Warm the price cache for `names` (reward item names, e.g. "Mirage Prime
/// Blueprint" — not slugs) without needing a reward screen's OCR'd slot list:
/// resolves each to its catalogue slug and fetches/caches its price the same
/// way [`evaluate_cached`] does, at bounded concurrency (see
/// [`PREWARM_CONCURRENCY`]). Used to pre-warm prices as soon as a fissure
/// starts (see [`relics::active_tier_reward_names`]) rather than only once
/// the reward screen actually appears, so a fresh price is more likely to
/// already be on disk inside the short selection window.
pub async fn prewarm_reward_prices(
    names: &[String],
    index: &ItemIndex,
    market: &MarketClient,
    cache: &PriceCache,
    opts: PriceOpts,
) {
    use futures::stream::StreamExt;

    let slugs: Vec<&str> = names
        .iter()
        .filter_map(|n| index.best_match(n).filter(|m| is_prime(m.item)).map(|m| m.item.slug.as_str()))
        .collect();
    futures::stream::iter(slugs)
        .for_each_concurrent(PREWARM_CONCURRENCY, |slug| async move {
            cached_price(cache, market, slug, opts).await;
        })
        .await;
    cache.save();
}

/// Lowest sell price (platinum) for `slug` via the disk price cache: fresh →
/// instant, stale/missing → bounded fetch falling back to stale. Used to price
/// relics for the owned-relic guide.
pub async fn cached_plat(
    cache: &PriceCache,
    market: &MarketClient,
    slug: &str,
    opts: PriceOpts,
) -> Option<u32> {
    cached_price(cache, market, slug, opts).await.and_then(|s| s.lowest_sell)
}

/// [`RivenTypeVerdict`] for `weapon_url_name` via the disk riven-price
/// cache: fresh → instant, stale/missing → bounded fetch (falling back to
/// stale on timeout/error), mirroring [`cached_price`]'s exact pattern for
/// Prime Part prices.
pub async fn cached_riven_verdict(
    cache: &RivenPriceCache,
    market: &RivenMarketClient,
    weapon_url_name: &str,
    opts: PriceOpts,
) -> Option<RivenTypeVerdict> {
    let stale = cache.get(weapon_url_name);
    if let Some(s) = &stale {
        if s.age() < opts.fresh_ttl {
            return Some(s.value);
        }
    }
    match tokio::time::timeout(opts.fetch_timeout, market.auctions_for(weapon_url_name)).await {
        Ok(Ok(auctions)) => {
            let listings: Vec<ListingInput> = auctions
                .iter()
                .map(|a| ListingInput {
                    is_direct_sell: a.is_direct_sell,
                    buyout_price: a.buyout_price,
                    top_bid: a.top_bid,
                    updated: a.updated,
                })
                .collect();
            let verdict = evaluate_riven_price(&listings, time::OffsetDateTime::now_utc());
            cache.put(weapon_url_name, verdict);
            Some(verdict)
        }
        _ => stale.map(|s| s.value),
    }
}

/// Index of the highest-platinum reward, if any have a price.
pub fn best_by_plat(evals: &[RewardEval]) -> Option<usize> {
    evals
        .iter()
        .enumerate()
        .filter_map(|(i, e)| e.plat.map(|p| (i, p)))
        .max_by_key(|&(_, p)| p)
        .map(|(i, _)| i)
}

/// An unmastered reward may give up this fraction of the top-plat reward's
/// value and still outrank it — "slightly cheaper" per issue #8: the plat
/// price alone doesn't capture the mastery progress an unmastered pick still
/// carries.
const MASTERY_PREFERENCE_RATIO: f32 = 0.8;

/// Index of the reward to highlight as the overall best pick: the
/// highest-plat reward, unless the highest-plat *unmastered* reward is priced
/// within [`MASTERY_PREFERENCE_RATIO`] of it, in which case the unmastered
/// one wins.
pub fn best_pick(evals: &[RewardEval], mastery: &MasterySet) -> Option<usize> {
    let top = best_by_plat(evals)?;
    let top_plat = evals[top].plat? as f32;

    let unmastered_best = evals
        .iter()
        .enumerate()
        .filter(|(_, e)| e.plat.is_some())
        .filter(|(_, e)| e.matched_name.as_deref().is_some_and(|n| !mastery.is_mastered(n)))
        .max_by_key(|(_, e)| e.plat);

    match unmastered_best {
        Some((i, e)) if e.plat.unwrap() as f32 >= top_plat * MASTERY_PREFERENCE_RATIO => Some(i),
        _ => Some(top),
    }
}

/// Index of the highest-ducat reward, if any are prime parts.
pub fn best_by_ducats(evals: &[RewardEval]) -> Option<usize> {
    evals
        .iter()
        .enumerate()
        .filter_map(|(i, e)| e.ducats.map(|d| (i, d)))
        .max_by_key(|&(_, d)| d)
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wf_data::items::Item;

    fn item(name: &str, tags: &[&str]) -> Item {
        Item {
            slug: index::normalize(name),
            name: name.to_string(),
            ducats: None,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            vaulted: false,
        }
    }

    #[test]
    fn select_rewards_counts_forma_and_rejects_relics() {
        let idx = ItemIndex::new(vec![
            item("Mirage Prime Blueprint", &["prime"]),
            item("Meso N11 Relic", &["relic"]),
        ]);
        // A relic name must never resolve as a fissure reward.
        assert!(select_rewards(&["Meso N11 Relic".to_string()], &idx).is_empty());
        // Forma (untradable) counts toward the ≥2 guard alongside a prime part,
        // so a mostly-Forma screen still triggers. (An empty slot between them, so
        // the adjacent-slot dedup doesn't merge the two cards.)
        let sel = select_rewards(
            &[
                "Forma Blueprint".to_string(),
                String::new(),
                "Mirage Prime Blueprint".to_string(),
            ],
            &idx,
        );
        assert_eq!(sel.len(), 2);
    }

    #[test]
    fn select_rewards_rejects_mods() {
        // A mod (e.g. picked up during the mission, not a fissure reward) must
        // never resolve as a reward choice, even though it isn't relic-tagged
        // and clears the fuzzy-match threshold cleanly.
        let idx = ItemIndex::new(vec![
            item("Mirage Prime Blueprint", &["prime"]),
            item("Blunderbuss", &["mod", "shotgun", "primary"]),
            item("Flame Repellent", &["mod", "warframe"]),
        ]);
        assert!(select_rewards(&["Blunderbuss".to_string()], &idx).is_empty());
        assert!(select_rewards(&["Flame Repellent".to_string()], &idx).is_empty());
        let sel = select_rewards(
            &["Blunderbuss".to_string(), "Mirage Prime Blueprint".to_string()],
            &idx,
        );
        assert_eq!(sel, vec!["Mirage Prime Blueprint".to_string()]);
    }

    fn eval(plat: Option<u32>, ducats: Option<u32>) -> RewardEval {
        RewardEval {
            ocr: String::new(),
            matched_name: None,
            slug: None,
            score: 1.0,
            ducats,
            plat,
            vaulted: false,
        }
    }

    #[test]
    fn ranks_by_plat_and_ducats() {
        let evals = vec![
            eval(Some(12), Some(15)),
            eval(Some(45), Some(45)),
            eval(None, Some(100)),
        ];
        assert_eq!(best_by_plat(&evals), Some(1));
        assert_eq!(best_by_ducats(&evals), Some(2));
    }

    #[test]
    fn handles_all_unresolved() {
        let evals = vec![eval(None, None), eval(None, None)];
        assert_eq!(best_by_plat(&evals), None);
        assert_eq!(best_by_ducats(&evals), None);
    }

    fn matched_eval(name: &str, plat: Option<u32>) -> RewardEval {
        RewardEval {
            ocr: name.to_string(),
            matched_name: Some(name.to_string()),
            slug: None,
            score: 1.0,
            ducats: None,
            plat,
            vaulted: false,
        }
    }

    #[test]
    fn best_pick_prefers_a_close_unmastered_reward_over_a_pricier_mastered_one() {
        let mastery = MasterySet::from_xp([("/Lotus/Powersuits/Ember/EmberPrime".to_string(), 900_000)]);
        let evals = vec![
            matched_eval("Ember Prime Blueprint", Some(100)), // mastered, top plat
            matched_eval("Nova Prime Blueprint", Some(85)),   // unmastered, within 20%
        ];
        assert_eq!(best_pick(&evals, &mastery), Some(1));
    }

    #[test]
    fn best_pick_falls_back_to_plat_when_the_unmastered_gap_is_too_wide() {
        let mastery = MasterySet::from_xp([("/Lotus/Powersuits/Ember/EmberPrime".to_string(), 900_000)]);
        let evals = vec![
            matched_eval("Ember Prime Blueprint", Some(100)),
            matched_eval("Nova Prime Blueprint", Some(50)), // too far below the top pick
        ];
        assert_eq!(best_pick(&evals, &mastery), Some(0));
    }

    #[test]
    fn best_pick_matches_plat_when_the_top_pick_is_already_unmastered() {
        let mastery = MasterySet::default();
        let evals = vec![matched_eval("Ember Prime Blueprint", Some(100)), matched_eval("Nova Prime Blueprint", Some(50))];
        assert_eq!(best_pick(&evals, &mastery), Some(0));
    }
}
