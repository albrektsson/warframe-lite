//! Relic reward evaluation: turn OCR'd reward names into a ranked, priced list
//! so the overlay can highlight the most valuable pick.

pub mod index;
pub mod mastery;
pub mod regions;
pub mod relics;

pub use index::ItemIndex;
pub use mastery::MasterySet;
pub use regions::{Rect, RelicGridRegions, RelicSlot, RewardRegions};
pub use relics::{
    mastery_browser, mastery_plan, rank as rank_relics, sell_picks, tier_of, MasteryEntry,
    PrimePlan, PrimeRelicSource, RelicIndex, RelicInfo, RelicPick, OWNED_RELICS_FILE,
};

use std::time::Duration;

use wf_data::market::{MarketClient, PriceSummary};

/// A disk-backed cache of per-item price summaries.
pub type PriceCache = wf_cache::KeyedCache<PriceSummary>;

/// Load the shared on-disk price cache every price lookup (relic or reward)
/// reads from and writes back to.
pub fn price_cache() -> PriceCache {
    PriceCache::load("prices.json")
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
) -> Vec<RewardEval> {
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        // Untradable rewards (Forma/Kuva/…) are labelled, not fuzzy-matched.
        if let Some(label) = untradable_label(name) {
            out.push(RewardEval::untradable(name.clone(), label));
            continue;
        }
        match index.best_match(name) {
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
        .map(|((ocr, res), price)| build_eval(ocr.clone(), res, price))
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
            // A fissure reward is never a relic; reject relic matches so the Void
            // Relics inventory grid can't be mistaken for a reward screen.
            index.best_match(t).filter(|m| !is_relic(m.item)).map(|m| m.score)
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

/// Whether a catalogue item is a Void Relic (tagged `relic`) — such items are
/// never fissure rewards, so they must not resolve in the reward path.
fn is_relic(item: &wf_data::items::Item) -> bool {
    item.tags.iter().any(|t| t == "relic")
}

fn resolve(name: &str, index: &ItemIndex) -> Resolution {
    if let Some(label) = untradable_label(name) {
        return Resolution::Untradable(label);
    }
    match index.best_match(name) {
        Some(m) if !is_relic(m.item) => Resolution::Matched {
            name: m.item.name.clone(),
            slug: m.item.slug.clone(),
            score: m.score,
            ducats: m.item.ducats,
        },
        _ => Resolution::Unresolved,
    }
}

fn build_eval(ocr: String, res: Resolution, price: Option<PriceSummary>) -> RewardEval {
    match res {
        Resolution::Untradable(label) => RewardEval::untradable(ocr, label),
        Resolution::Unresolved => RewardEval::unresolved(ocr),
        Resolution::Matched {
            name,
            slug,
            score,
            ducats,
        } => RewardEval {
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

/// Index of the highest-platinum reward, if any have a price.
pub fn best_by_plat(evals: &[RewardEval]) -> Option<usize> {
    evals
        .iter()
        .enumerate()
        .filter_map(|(i, e)| e.plat.map(|p| (i, p)))
        .max_by_key(|&(_, p)| p)
        .map(|(i, _)| i)
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

    fn eval(plat: Option<u32>, ducats: Option<u32>) -> RewardEval {
        RewardEval {
            ocr: String::new(),
            matched_name: None,
            slug: None,
            score: 1.0,
            ducats,
            plat,
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
}
