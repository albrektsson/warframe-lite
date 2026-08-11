//! Owned-relic guide: which relics contain rewards the player hasn't mastered.
//!
//! Relic → reward drop tables come from WFCD `warframe-drop-data` (readable item
//! names + rarity), cached to disk. Cross-referenced with the mastered set
//! ([`MasterySet`]) and warframe.market relic prices, this ranks the relics worth
//! opening for mastery. Ownership itself is supplied by the caller (from OCR of
//! the in-game Relics screen); this module is network/screen-agnostic and pure.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::index::{levenshtein, normalize, ItemIndex};
use crate::mastery::{built_name, prime_part, MasterySet, PrimePart};
use crate::owned::RelicEvidence;
use crate::part_quantities::PartQuantities;

const RELICS_URL: &str =
    "https://raw.githubusercontent.com/WFCD/warframe-drop-data/gh-pages/data/relics.json";
// `-v2`: rewards gained `intact_chance`/`radiant_chance` (see `fetch`'s
// Intact+Radiant merge) — bump so an old-format cache isn't deserialized into
// the new shape.
const CACHE_FILE: &str = "relics-v2.json";

/// One reward inside a relic. `intact_chance`/`radiant_chance` are drop-chance
/// percentages (e.g. `20.0` for 20%) at those two refinement states — fixed
/// game-wide constants per (rarity tier, refinement state), not per relic
/// (see issue #19's research), used to compute [`expected_value`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelicReward {
    #[serde(rename = "itemName")]
    pub item_name: String,
    #[serde(default)]
    pub rarity: String,
    #[serde(default)]
    pub intact_chance: f32,
    #[serde(default)]
    pub radiant_chance: f32,
}

/// A relic and its Intact-state reward set (with each reward's Radiant chance
/// merged in too — see `fetch`).
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
    /// This relic's worst-off distinct unmastered Prime Part, per the
    /// Inventory/Sell screen scan — `None` when the relic has no unmastered
    /// rewards at all (see [`PartsOwnedSummary`]). Display-only: never
    /// changes `unmastered`/`plat`-driven ranking or classification.
    pub parts_owned: Option<PartsOwnedSummary>,
}

/// One relic's Sell tab owned-Prime-Part summary: the worst-off distinct
/// unmastered Prime Part it can still drop (smallest owned/need ratio — an
/// unscanned part, `owned: None`, always sorts worst of all — see issue #37's
/// downstream-wiring decision), plus how many other distinct unmastered parts
/// it can also drop, mirroring the overlay's existing "top reward, +N"
/// truncation pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct PartsOwnedSummary {
    pub part: PrimePart,
    /// How many the player already owns — `None` means never scanned, never
    /// `0` (see ADR-0011's "never guess an unknown quantity" precedent).
    pub owned: Option<u32>,
    /// How many a full build needs, if known (see ADR-0011).
    pub need: Option<u32>,
    /// How many *other* distinct unmastered Prime Parts this relic can also
    /// still drop.
    pub more: usize,
}

/// The worst-off distinct unmastered Prime Part `relic` can still drop (see
/// [`PartsOwnedSummary`]), or `None` if it has no unmastered rewards.
fn worst_off_part(
    relic: &RelicInfo,
    mastery: &MasterySet,
    quantities: &PartQuantities,
    owned_parts: &crate::OwnedPrimeParts,
) -> Option<PartsOwnedSummary> {
    let mut parts: Vec<PrimePart> = Vec::new();
    for r in unmastered_rewards(relic, mastery) {
        let pp = prime_part(&r.item_name);
        if !parts.contains(&pp) {
            parts.push(pp);
        }
    }
    if parts.is_empty() {
        return None;
    }
    // Sort key: an unscanned part (owned: None) sorts first (worst of all);
    // among scanned parts, ascending owned/need ratio (need defaults to 1
    // when unknown) puts the least-progressed part first.
    let key = |p: &PrimePart| -> (bool, f32) {
        match crate::owned_parts::get(owned_parts, p) {
            None => (false, 0.0),
            Some(o) => {
                let need = quantities.get(p).unwrap_or(1).max(1);
                (true, o as f32 / need as f32)
            }
        }
    };
    parts.sort_by(|a, b| key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal));
    let worst = parts.remove(0);
    let owned = crate::owned_parts::get(owned_parts, &worst);
    let need = quantities.get(&worst);
    Some(PartsOwnedSummary { part: worst, owned, need, more: parts.len() })
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

/// Reward catalogue name → vaulted status, joining each relic's drop table
/// against the item catalogue's per-relic `vaulted` flag — a free read over
/// data both `RelicIndex` and `ItemIndex` already have cached, no new network
/// call. A reward is vaulted only when **every** relic that can drop it
/// resolves (by [`RelicInfo::slug`]) to a catalogue entry with
/// `vaulted == true` — the strict, conservative reading, since a live scan
/// resolves reward names but not which specific relic on a mixed-squad
/// screen produced them. A reward with no known source relic is absent from
/// the map (unknown ≠ vaulted).
pub fn vaulted_rewards(relics: &RelicIndex, items: &ItemIndex) -> HashMap<String, bool> {
    let mut sources: HashMap<String, Vec<&RelicInfo>> = HashMap::new();
    for relic in relics.all() {
        for r in &relic.rewards {
            // Resolve through the catalogue (rather than keying on the raw
            // relics.json name) so the map's keys line up exactly with
            // `RewardEval::matched_name`, which is always a catalogue name.
            let Some(m) = items.best_match(&r.item_name) else {
                continue;
            };
            sources.entry(m.item.name.clone()).or_default().push(relic);
        }
    }
    sources
        .into_iter()
        .map(|(name, srcs)| {
            let vaulted = srcs
                .iter()
                .all(|r| items.by_slug(&r.slug()).is_some_and(|i| i.vaulted));
            (name, vaulted)
        })
        .collect()
}

/// The relic/mastery/part catalogue and owned-parts scan, bundled by
/// reference: these four always travel together, unchanged, through every
/// planning call in a request (see [`sell_picks`], [`mastery_plan`]).
pub struct RelicContext<'a> {
    pub index: &'a RelicIndex,
    pub mastery: &'a MasterySet,
    pub quantities: &'a PartQuantities,
    pub owned_parts: &'a crate::OwnedPrimeParts,
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
///
/// `ctx.quantities` and `ctx.owned_parts` populate each pick's
/// [`RelicPick::parts_owned`] display-only summary — they play no role in
/// `rank`'s plat/unmastered-driven ordering.
pub fn sell_picks(
    owned: &HashMap<String, u32>,
    prices: &HashMap<String, Option<u32>>,
    ctx: &RelicContext,
) -> Vec<RelicPick> {
    let mut picks: Vec<RelicPick> = ctx
        .index
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
                unmastered: relic.unmastered(ctx.mastery),
                plat,
                parts_owned: worst_off_part(relic, ctx.mastery, ctx.quantities, ctx.owned_parts),
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

/// Reward item names — every tradable prime part, mastered or not — from
/// relics the player owns whose tier currently has an active Fissure. The
/// candidate set for background price pre-warming: these are the items that
/// could actually land on a reward screen if the player cracks one of these
/// relics right now, so it's worth having their prices already fresh rather
/// than fetching them cold once the screen appears.
pub fn active_tier_reward_names(
    owned: &HashMap<String, RelicEvidence>,
    index: &RelicIndex,
    active_tiers: &HashSet<String>,
) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for relic in index.all() {
        if !active_tiers.contains(&relic.tier) || !owned.contains_key(&relic.display) {
            continue;
        }
        for r in &relic.rewards {
            if crate::untradable_label(&r.item_name).is_none()
                && is_prime_reward(&r.item_name)
                && !names.contains(&r.item_name)
            {
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
    /// Ownership evidence: a confirmed total count, or merely seen with no
    /// confirmed count yet (see [`RelicEvidence`]).
    pub evidence: RelicEvidence,
    /// Drop rarity of this part within this relic (Common/Uncommon/Rare).
    pub rarity: String,
    /// Lowest market sell price in platinum, if resolved.
    pub plat: Option<u32>,
}

/// One Prime Part of an unmastered prime, and the owned relics that can still
/// drop it.
#[derive(Debug, Clone)]
pub struct PrimePartGroup {
    pub part: PrimePart,
    /// How many `part.part` a full build needs, if known — never guessed when
    /// unknown (see ADR-0011).
    pub build_quantity: Option<u32>,
    /// How many the player already owns, per the Inventory/Sell screen scan —
    /// `None` when this part's card has never been scanned, never `0` (a
    /// part shows a concrete number only once a scan has actually observed
    /// its card at least once — see issue #37's downstream-wiring decision,
    /// following ADR-0011's "never guess an unknown quantity" precedent).
    pub owned: Option<u32>,
    /// Owned relics that can drop this part, cheapest first (unresolved prices
    /// last) so a plentiful cheap relic is never overlooked in favor of a
    /// pricier — often vaulted — one that happens to be owned too; ties broken
    /// by most-owned, then relic display.
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
    /// Sum of confirmed owned counts across all distinct sourcing relics (a
    /// relic counted once even if it drops more than one of this prime's
    /// parts, and a seen-only relic with no confirmed count contributing 0) —
    /// a rough farming budget.
    pub total_owned: u32,
    /// Lowest market sell price in platinum for the whole built-prime Set, if
    /// resolved.
    pub set_plat: Option<u32>,
}

/// A relic's evidence rank for sort tie-breaking: `Confirmed` outranks
/// `SeenOnly`, and among `Confirmed` a higher count outranks a lower one.
pub(crate) fn evidence_rank(evidence: RelicEvidence) -> (u8, u32) {
    match evidence {
        RelicEvidence::Confirmed(n) => (1, n),
        RelicEvidence::SeenOnly => (0, 0),
    }
}

pub(crate) type RelicGroups =
    std::collections::HashMap<String, std::collections::HashMap<PrimePart, Vec<PrimeRelicSource>>>;
pub(crate) type PrimeRelicEvidence =
    std::collections::HashMap<String, std::collections::HashMap<String, RelicEvidence>>;

/// Walk every relic the player has any evidence for (confirmed or seen-only),
/// grouping its still-unmastered rewards by [`PrimePart`] — the shared core of
/// [`mastery_plan`] and [`crate::bom::buy_or_farm_plan`], which both need "which
/// relics can still drop this prime's parts" but differ in which primes/parts
/// they surface. Also returns, per prime, each contributing relic's evidence
/// (deduped per relic even when it drops more than one of that prime's parts)
/// so callers can derive a farming-budget total without re-walking relics.
pub(crate) fn relic_sourced_parts(
    owned: &HashMap<String, RelicEvidence>,
    prices: &HashMap<String, Option<u32>>,
    index: &RelicIndex,
    mastery: &MasterySet,
) -> (RelicGroups, PrimeRelicEvidence) {
    let mut by_prime: RelicGroups = std::collections::HashMap::new();
    let mut prime_relic_evidence: PrimeRelicEvidence = std::collections::HashMap::new();

    for relic in index.all() {
        let Some(&evidence) = owned.get(&relic.display) else {
            continue;
        };
        let plat = prices.get(&relic.slug()).copied().flatten();
        let mut seen: Vec<PrimePart> = Vec::new();
        for r in unmastered_rewards(relic, mastery) {
            let pp = prime_part(&r.item_name);
            if seen.contains(&pp) {
                continue;
            }
            seen.push(pp.clone());
            by_prime.entry(pp.prime.clone()).or_default().entry(pp.clone()).or_default().push(
                PrimeRelicSource { relic_display: relic.display.clone(), evidence, rarity: r.rarity.clone(), plat },
            );
            prime_relic_evidence.entry(pp.prime).or_default().insert(relic.display.clone(), evidence);
        }
    }
    (by_prime, prime_relic_evidence)
}

/// One relic that can drop a given [`PrimePart`], regardless of whether the
/// player owns it — a candidate to buy or farm, not a report of what's
/// already held (contrast [`PrimeRelicSource`]).
#[derive(Debug, Clone)]
pub struct RelicOption {
    pub relic_display: String,
    /// Lowest market sell price in platinum, if resolved.
    pub plat: Option<u32>,
}

/// Every relic in the catalogue that can still drop each still-unmastered
/// [`PrimePart`] — ownership-independent, so it can name a relic to go buy or
/// farm even when the player owns none of it yet (unlike
/// [`relic_sourced_parts`], which only walks relics the player has evidence
/// for). Cheapest first within each part.
pub(crate) fn all_relic_sources(
    prices: &HashMap<String, Option<u32>>,
    index: &RelicIndex,
    mastery: &MasterySet,
) -> HashMap<PrimePart, Vec<RelicOption>> {
    let mut by_part: HashMap<PrimePart, Vec<RelicOption>> = HashMap::new();
    for relic in index.all() {
        let plat = prices.get(&relic.slug()).copied().flatten();
        let mut seen: Vec<PrimePart> = Vec::new();
        for r in unmastered_rewards(relic, mastery) {
            let pp = prime_part(&r.item_name);
            if seen.contains(&pp) {
                continue;
            }
            seen.push(pp.clone());
            by_part.entry(pp).or_default().push(RelicOption { relic_display: relic.display.clone(), plat });
        }
    }
    for options in by_part.values_mut() {
        options.sort_by(|a, b| {
            a.plat.unwrap_or(u32::MAX).cmp(&b.plat.unwrap_or(u32::MAX)).then_with(|| a.relic_display.cmp(&b.relic_display))
        });
    }
    by_part
}

/// Build a fissure-planning view from an owned-relic evidence map (relic display →
/// [`RelicEvidence`]): for every unmastered prime the player's relics can still drop,
/// broken down by Prime Part, which relics (and their evidence) can drop it. Ranked
/// by `total_owned` descending, so the primes with the most farming budget
/// already in hand come first; parts within a prime are ranked the same way.
///
/// Prices come from caller-supplied maps (relic market slug → already-resolved
/// plat, built-prime name → already-resolved Set plat; `None` where
/// unresolved) rather than being fetched inline, keeping this pure and
/// mirroring [`sell_picks`]; a key missing from either map is treated the same
/// as an explicit `None`.
///
/// `ctx.owned_parts` is the Inventory/Sell screen's scanned owned-Prime-Part
/// counts (see [`crate::OwnedPrimeParts`]), used only to populate each
/// [`PrimePartGroup::owned`] — it plays no role in which primes/parts appear
/// or how they're ranked (that's still driven entirely by `owned`, the
/// owned-*relic* evidence).
pub fn mastery_plan(
    owned: &HashMap<String, RelicEvidence>,
    prices: &HashMap<String, Option<u32>>,
    set_prices: &HashMap<String, Option<u32>>,
    ctx: &RelicContext,
) -> Vec<PrimePlan> {
    let (by_prime, prime_relic_evidence) = relic_sourced_parts(owned, prices, ctx.index, ctx.mastery);

    let mut plans: Vec<PrimePlan> = by_prime
        .into_iter()
        .map(|(prime, part_map)| {
            let mut parts: Vec<PrimePartGroup> = part_map
                .into_iter()
                .map(|(pp, mut relics)| {
                    // Cheapest first (unresolved last) — see `PrimePartGroup::relics`.
                    relics.sort_by(|a, b| {
                        a.plat
                            .unwrap_or(u32::MAX)
                            .cmp(&b.plat.unwrap_or(u32::MAX))
                            .then_with(|| evidence_rank(b.evidence).cmp(&evidence_rank(a.evidence)))
                            .then_with(|| a.relic_display.cmp(&b.relic_display))
                    });
                    let build_quantity = ctx.quantities.get(&pp);
                    let owned = crate::owned_parts::get(ctx.owned_parts, &pp);
                    PrimePartGroup { part: pp, build_quantity, owned, relics }
                })
                .collect();
            parts.sort_by(|a, b| a.part.part.cmp(&b.part.part));
            let total_owned = prime_relic_evidence
                .get(&prime)
                .map(|m| {
                    m.values()
                        .map(|e| match e {
                            RelicEvidence::Confirmed(n) => *n,
                            RelicEvidence::SeenOnly => 0,
                        })
                        .sum()
                })
                .unwrap_or(0);
            let set_plat = set_prices.get(&prime).copied().flatten();
            PrimePlan { prime, parts, total_owned, set_plat }
        })
        .collect();
    plans.sort_by(|a, b| b.total_owned.cmp(&a.total_owned).then_with(|| a.prime.cmp(&b.prime)));
    plans
}

#[derive(Deserialize)]
struct RawFile {
    relics: Vec<RawRelic>,
}

#[derive(Deserialize, Clone)]
struct RawRelic {
    tier: String,
    // A rare malformed WFCD entry omits relicName; default + filter it out.
    #[serde(rename = "relicName", default)]
    relic_name: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    rewards: Vec<RawReward>,
}

#[derive(Deserialize, Clone)]
struct RawReward {
    #[serde(rename = "itemName")]
    item_name: String,
    #[serde(default)]
    rarity: String,
    #[serde(default)]
    chance: f32,
}

/// Merge WFCD's one-row-per-(relic,state) source into one [`RelicInfo`] per
/// relic, keeping only the Intact and Radiant rows (the reward item set is
/// state-independent — only `chance` differs, per issue #19's research) and
/// dropping any relic missing either state row entirely (logged, not
/// surfaced as a partial entry) rather than emit a reward with no
/// `radiant_chance`. A reward present in the Intact row but absent from the
/// Radiant row (shouldn't happen for a standard relic, but WFCD sync lag is
/// possible) is likewise dropped rather than guessed.
fn merge_intact_radiant(raw: Vec<RawRelic>) -> Vec<RelicInfo> {
    // Group Intact/Radiant rows by (tier, code), preserving first-seen order
    // for a stable result (a `HashMap`'s own iteration order isn't).
    let mut order: Vec<(String, String)> = Vec::new();
    let mut by_relic: HashMap<(String, String), (Option<RawRelic>, Option<RawRelic>)> = HashMap::new();
    for r in raw {
        if r.relic_name.is_empty() {
            continue;
        }
        let key = (r.tier.clone(), r.relic_name.clone());
        if !by_relic.contains_key(&key) {
            order.push(key.clone());
        }
        let entry = by_relic.entry(key).or_insert((None, None));
        if r.state.eq_ignore_ascii_case("Intact") {
            entry.0 = Some(r);
        } else if r.state.eq_ignore_ascii_case("Radiant") {
            entry.1 = Some(r);
        }
    }

    let mut relics = Vec::new();
    for key in order {
        let (tier, code) = key.clone();
        let (intact, radiant) = by_relic.remove(&key).unwrap_or((None, None));
        let (Some(intact), Some(radiant)) = (intact, radiant) else {
            tracing::warn!("relic {tier} {code}: missing Intact or Radiant row, dropped");
            continue;
        };
        let radiant_chances: HashMap<String, f32> =
            radiant.rewards.into_iter().map(|r| (r.item_name, r.chance)).collect();
        let rewards: Vec<RelicReward> = intact
            .rewards
            .into_iter()
            .filter_map(|r| {
                let radiant_chance = *radiant_chances.get(&r.item_name)?;
                Some(RelicReward {
                    item_name: r.item_name,
                    rarity: r.rarity,
                    intact_chance: r.chance,
                    radiant_chance,
                })
            })
            .collect();
        relics.push(RelicInfo { display: format!("{tier} {code}"), tier, code, rewards });
    }
    relics
}

/// Fetch and parse the WFCD relic drop tables (see [`merge_intact_radiant`]).
async fn fetch(client: &reqwest::Client) -> anyhow::Result<Vec<RelicInfo>> {
    tracing::debug!("GET {RELICS_URL}");
    let file: RawFile = client
        .get(RELICS_URL)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(merge_intact_radiant(file.relics))
}

/// Which refinement state's drop chance [`expected_value`] weights by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvRefinement {
    Intact,
    Radiant,
}

/// Drop-chance-weighted expected plat value of `rewards` at `refinement`:
/// `Σ price_i × chance_i / 100`. `prices` maps reward item name to its
/// resolved market price (`None` = checked, no listing — contributes `0`, a
/// real outcome, not "unknown"). Returns `None` if any reward in `rewards`
/// has no entry in `prices` at all (still loading), so the caller can show a
/// loading state instead of an EV computed from partial data.
pub fn expected_value(
    rewards: &[RelicReward],
    prices: &HashMap<String, Option<u32>>,
    refinement: EvRefinement,
) -> Option<f32> {
    let mut total = 0.0;
    for r in rewards {
        let price = (*prices.get(&r.item_name)?).unwrap_or(0);
        let chance = match refinement {
            EvRefinement::Intact => r.intact_chance,
            EvRefinement::Radiant => r.radiant_chance,
        };
        total += chance / 100.0 * price as f32;
    }
    Some(total)
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
                .map(|n| RelicReward {
                    item_name: n.to_string(),
                    rarity: String::new(),
                    intact_chance: 0.0,
                    radiant_chance: 0.0,
                })
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
                .map(|(n, r)| RelicReward {
                    item_name: n.to_string(),
                    rarity: r.to_string(),
                    intact_chance: 0.0,
                    radiant_chance: 0.0,
                })
                .collect(),
        }
    }

    fn raw_relic(tier: &str, code: &str, state: &str, rewards: &[(&str, &str, f32)]) -> RawRelic {
        RawRelic {
            tier: tier.to_string(),
            relic_name: code.to_string(),
            state: state.to_string(),
            rewards: rewards
                .iter()
                .map(|(name, rarity, chance)| RawReward {
                    item_name: name.to_string(),
                    rarity: rarity.to_string(),
                    chance: *chance,
                })
                .collect(),
        }
    }

    #[test]
    fn merge_intact_radiant_combines_both_states_per_reward() {
        let raw = vec![
            raw_relic("Axi", "H3", "Intact", &[("Nikana Prime Blueprint", "Uncommon", 11.0)]),
            raw_relic("Axi", "H3", "Radiant", &[("Nikana Prime Blueprint", "Uncommon", 20.0)]),
        ];
        let relics = merge_intact_radiant(raw);
        assert_eq!(relics.len(), 1);
        let reward = &relics[0].rewards[0];
        assert_eq!(reward.intact_chance, 11.0);
        assert_eq!(reward.radiant_chance, 20.0);
    }

    #[test]
    fn merge_intact_radiant_drops_a_relic_missing_either_state() {
        // Intact only, no Radiant row at all for this relic.
        let raw = vec![raw_relic("Axi", "H3", "Intact", &[("Nikana Prime Blueprint", "Uncommon", 11.0)])];
        assert!(merge_intact_radiant(raw).is_empty());
    }

    #[test]
    fn merge_intact_radiant_drops_a_reward_absent_from_the_radiant_row() {
        let raw = vec![
            raw_relic(
                "Axi",
                "H3",
                "Intact",
                &[("Nikana Prime Blueprint", "Uncommon", 11.0), ("Forma Blueprint", "Common", 25.33)],
            ),
            // Radiant row is missing the Forma reward — a WFCD sync-lag quirk.
            raw_relic("Axi", "H3", "Radiant", &[("Nikana Prime Blueprint", "Uncommon", 20.0)]),
        ];
        let relics = merge_intact_radiant(raw);
        assert_eq!(relics[0].rewards.len(), 1);
        assert_eq!(relics[0].rewards[0].item_name, "Nikana Prime Blueprint");
    }

    #[test]
    fn merge_intact_radiant_ignores_malformed_entries_and_other_states() {
        let raw = vec![
            // Missing relicName entirely.
            raw_relic("Axi", "", "Intact", &[]),
            raw_relic("Axi", "H3", "Exceptional", &[("Nikana Prime Blueprint", "Uncommon", 13.0)]),
            raw_relic("Axi", "H3", "Intact", &[("Nikana Prime Blueprint", "Uncommon", 11.0)]),
            raw_relic("Axi", "H3", "Radiant", &[("Nikana Prime Blueprint", "Uncommon", 20.0)]),
        ];
        let relics = merge_intact_radiant(raw);
        assert_eq!(relics.len(), 1);
        assert_eq!(relics[0].display, "Axi H3");
    }

    #[test]
    fn expected_value_weights_prices_by_drop_chance() {
        let rewards = vec![
            RelicReward {
                item_name: "A".to_string(),
                rarity: "Uncommon".to_string(),
                intact_chance: 25.33,
                radiant_chance: 16.67,
            },
            RelicReward {
                item_name: "B".to_string(),
                rarity: "Rare".to_string(),
                intact_chance: 2.0,
                radiant_chance: 10.0,
            },
        ];
        let prices = HashMap::from([("A".to_string(), Some(10)), ("B".to_string(), Some(100))]);
        let intact = expected_value(&rewards, &prices, EvRefinement::Intact).unwrap();
        // 0.2533*10 + 0.02*100 = 2.533 + 2.0
        assert!((intact - 4.533).abs() < 0.001);
        let radiant = expected_value(&rewards, &prices, EvRefinement::Radiant).unwrap();
        // 0.1667*10 + 0.10*100 = 1.667 + 10.0
        assert!((radiant - 11.667).abs() < 0.001);
    }

    #[test]
    fn expected_value_treats_an_unlisted_reward_as_zero_not_unknown() {
        let rewards = vec![RelicReward {
            item_name: "A".to_string(),
            rarity: "Uncommon".to_string(),
            intact_chance: 25.33,
            radiant_chance: 16.67,
        }];
        let prices = HashMap::from([("A".to_string(), None)]);
        assert_eq!(expected_value(&rewards, &prices, EvRefinement::Intact), Some(0.0));
    }

    #[test]
    fn expected_value_is_none_when_a_reward_has_no_price_entry_at_all() {
        let rewards = vec![RelicReward {
            item_name: "A".to_string(),
            rarity: "Uncommon".to_string(),
            intact_chance: 25.33,
            radiant_chance: 16.67,
        }];
        let prices = HashMap::new();
        assert_eq!(expected_value(&rewards, &prices, EvRefinement::Intact), None);
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
            RelicPick { display: "A".into(), count: 1, unmastered: vec!["x".into()], plat: Some(10), parts_owned: None },
            RelicPick { display: "B".into(), count: 1, unmastered: vec!["x".into(), "y".into()], plat: Some(25), parts_owned: None },
            RelicPick { display: "C".into(), count: 1, unmastered: vec!["x".into(), "y".into()], plat: Some(25), parts_owned: None },
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

        let picks = sell_picks(
            &owned,
            &prices,
            &RelicContext {
                index: &idx,
                mastery: &mastery,
                quantities: &PartQuantities::empty(),
                owned_parts: &crate::OwnedPrimeParts::new(),
            },
        );

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

        let picks = sell_picks(
            &owned,
            &prices,
            &RelicContext {
                index: &idx,
                mastery: &MasterySet::default(),
                quantities: &PartQuantities::empty(),
                owned_parts: &crate::OwnedPrimeParts::new(),
            },
        );

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

        let picks = sell_picks(
            &owned,
            &prices,
            &RelicContext {
                index: &idx,
                mastery: &MasterySet::default(),
                quantities: &PartQuantities::empty(),
                owned_parts: &crate::OwnedPrimeParts::new(),
            },
        );

        assert_eq!(picks.iter().map(|p| p.display.as_str()).collect::<Vec<_>>(), vec!["Meso B2", "Axi A1"]);
    }

    #[test]
    fn sell_picks_parts_owned_is_none_for_a_fully_mastered_relic() {
        let idx = RelicIndex::new(vec![relic("Meso E1", &["Ember Prime Blueprint"])]);
        let mastery =
            MasterySet::from_xp([("/Lotus/Powersuits/Ember/EmberPrime".to_string(), 9_000_000)]);
        let owned = HashMap::from([("Meso E1".to_string(), 3)]);

        let picks = sell_picks(
            &owned,
            &HashMap::new(),
            &RelicContext {
                index: &idx,
                mastery: &mastery,
                quantities: &PartQuantities::empty(),
                owned_parts: &crate::OwnedPrimeParts::new(),
            },
        );

        assert_eq!(picks[0].parts_owned, None);
    }

    #[test]
    fn sell_picks_parts_owned_picks_the_worst_off_part_and_counts_the_rest() {
        // Two distinct unmastered parts: Barrel is scanned (2/2 = fully
        // covered), Link has never been scanned (unknown → always worst).
        let idx = RelicIndex::new(vec![relic(
            "Axi C3",
            &["Afuris Prime Barrel", "Afuris Prime Link"],
        )]);
        let owned = HashMap::from([("Axi C3".to_string(), 1)]);
        let quantities = PartQuantities::from_entries_for_test(vec![(
            "Afuris Prime".to_string(),
            "Barrel".to_string(),
            2,
        )]);
        let mut owned_parts = crate::OwnedPrimeParts::new();
        crate::owned_parts::apply_count(
            &mut owned_parts,
            &PrimePart { prime: "Afuris Prime".to_string(), part: "Barrel".to_string() },
            2,
            crate::owned::Source::Ocr,
        );

        let picks = sell_picks(
            &owned,
            &HashMap::new(),
            &RelicContext {
                index: &idx,
                mastery: &MasterySet::default(),
                quantities: &quantities,
                owned_parts: &owned_parts,
            },
        );

        let summary = picks[0].parts_owned.as_ref().unwrap();
        assert_eq!(summary.part.part, "Link"); // unscanned always sorts worst
        assert_eq!(summary.owned, None);
        assert_eq!(summary.more, 1); // Barrel is the one other unmastered part
    }

    #[test]
    fn sell_picks_parts_owned_ranks_by_owned_need_ratio_when_both_are_scanned() {
        let idx = RelicIndex::new(vec![relic(
            "Axi C3",
            &["Afuris Prime Barrel", "Afuris Prime Link"],
        )]);
        let owned = HashMap::from([("Axi C3".to_string(), 1)]);
        let quantities = PartQuantities::from_entries_for_test(vec![
            ("Afuris Prime".to_string(), "Barrel".to_string(), 2),
            ("Afuris Prime".to_string(), "Link".to_string(), 2),
        ]);
        let mut owned_parts = crate::OwnedPrimeParts::new();
        // Barrel: 1/2 = 0.5 ratio. Link: 2/2 = 1.0 ratio — Barrel is worse off.
        crate::owned_parts::apply_count(
            &mut owned_parts,
            &PrimePart { prime: "Afuris Prime".to_string(), part: "Barrel".to_string() },
            1,
            crate::owned::Source::Ocr,
        );
        crate::owned_parts::apply_count(
            &mut owned_parts,
            &PrimePart { prime: "Afuris Prime".to_string(), part: "Link".to_string() },
            2,
            crate::owned::Source::Ocr,
        );

        let picks = sell_picks(
            &owned,
            &HashMap::new(),
            &RelicContext {
                index: &idx,
                mastery: &MasterySet::default(),
                quantities: &quantities,
                owned_parts: &owned_parts,
            },
        );

        let summary = picks[0].parts_owned.as_ref().unwrap();
        assert_eq!(summary.part.part, "Barrel");
        assert_eq!(summary.owned, Some(1));
        assert_eq!(summary.need, Some(2));
        assert_eq!(summary.more, 1);
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
        let owned = HashMap::from([
            ("Axi A1".to_string(), RelicEvidence::Confirmed(5)),
            ("Meso N11".to_string(), RelicEvidence::Confirmed(2)),
            ("Lith G4".to_string(), RelicEvidence::Confirmed(9)), // owned but nothing unmastered → contributes nothing
            ("Neo V9".to_string(), RelicEvidence::Confirmed(3)),  // owned but not in the index → ignored
        ]);
        let plans = mastery_plan(
            &owned,
            &HashMap::new(),
            &HashMap::new(),
            &RelicContext {
                index: &idx,
                mastery: &mastery,
                quantities: &PartQuantities::empty(),
                owned_parts: &crate::OwnedPrimeParts::new(),
            },
        );

        // Akstiletto Prime's Barrel and Receiver are two different parts, each
        // sourced from a different owned relic: 5 + 2 = 7 total_owned, and it
        // ranks first (highest total_owned).
        let aksti = plans.iter().find(|p| p.prime == "Akstiletto Prime").unwrap();
        assert_eq!(aksti.total_owned, 7);
        assert_eq!(aksti.parts.len(), 2);
        let barrel = aksti.parts.iter().find(|g| g.part.part == "Barrel").unwrap();
        assert_eq!(barrel.relics.len(), 1);
        assert_eq!(barrel.relics[0].relic_display, "Axi A1");
        assert_eq!(barrel.relics[0].evidence, RelicEvidence::Confirmed(5));
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
    fn mastery_plan_orders_relics_cheapest_first() {
        // Same part droppable from a cheap, plentiful relic and an expensive
        // (e.g. vaulted) one owned in smaller number — cheapest must sort
        // first regardless of owned_count, so the guide never nudges toward
        // burning the expensive relic when the cheap one would do.
        let idx = RelicIndex::new(vec![
            relic("Axi H3", &["Rubico Prime Barrel"]),
            relic("Lith V1", &["Rubico Prime Barrel"]),
            relic("Meso B4", &["Rubico Prime Barrel"]), // unpriced
        ]);
        let owned = HashMap::from([
            ("Axi H3".to_string(), RelicEvidence::Confirmed(1)),
            ("Lith V1".to_string(), RelicEvidence::Confirmed(20)),
            ("Meso B4".to_string(), RelicEvidence::Confirmed(5)),
        ]);
        let prices = std::collections::HashMap::from([
            ("axi_h3_relic".to_string(), Some(50)),
            ("lith_v1_relic".to_string(), Some(2)),
            // Meso B4 deliberately absent — unresolved price.
        ]);
        let plans = mastery_plan(
            &owned,
            &prices,
            &HashMap::new(),
            &RelicContext {
                index: &idx,
                mastery: &MasterySet::default(),
                quantities: &PartQuantities::empty(),
                owned_parts: &crate::OwnedPrimeParts::new(),
            },
        );

        let rubico = plans.iter().find(|p| p.prime == "Rubico Prime").unwrap();
        let barrel = rubico.parts.iter().find(|g| g.part.part == "Barrel").unwrap();
        assert_eq!(
            barrel.relics.iter().map(|r| r.relic_display.as_str()).collect::<Vec<_>>(),
            vec!["Lith V1", "Axi H3", "Meso B4"]
        );
        assert_eq!(barrel.relics[0].plat, Some(2));
        assert_eq!(barrel.relics[2].plat, None);
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
        let owned = HashMap::from([("Axi B2".to_string(), RelicEvidence::Confirmed(4))]);

        let plans = mastery_plan(
            &owned,
            &HashMap::new(),
            &HashMap::new(),
            &RelicContext {
                index: &idx,
                mastery: &MasterySet::default(),
                quantities: &PartQuantities::empty(),
                owned_parts: &crate::OwnedPrimeParts::new(),
            },
        );

        let loki = plans.iter().find(|p| p.prime == "Loki Prime").unwrap();
        assert_eq!(loki.total_owned, 4);
        assert_eq!(loki.parts.len(), 2);
        assert!(loki.parts.iter().all(|g| g.relics.len() == 1
            && g.relics[0].evidence == RelicEvidence::Confirmed(4)));
    }

    #[test]
    fn mastery_plan_carries_build_quantity_when_known_and_none_when_unknown() {
        let idx = RelicIndex::new(vec![relic("Axi C3", &["Afuris Prime Barrel", "Afuris Prime Link"])]);
        let owned = HashMap::from([("Axi C3".to_string(), RelicEvidence::Confirmed(1))]);
        let quantities = PartQuantities::from_entries_for_test(vec![(
            "Afuris Prime".to_string(),
            "Barrel".to_string(),
            2,
        )]);

        let plans = mastery_plan(
            &owned,
            &HashMap::new(),
            &HashMap::new(),
            &RelicContext {
                index: &idx,
                mastery: &MasterySet::default(),
                quantities: &quantities,
                owned_parts: &crate::OwnedPrimeParts::new(),
            },
        );

        let afuris = plans.iter().find(|p| p.prime == "Afuris Prime").unwrap();
        let barrel = afuris.parts.iter().find(|g| g.part.part == "Barrel").unwrap();
        assert_eq!(barrel.build_quantity, Some(2));
        let link = afuris.parts.iter().find(|g| g.part.part == "Link").unwrap();
        assert_eq!(link.build_quantity, None); // not in the lookup — never guessed at 1
    }

    #[test]
    fn mastery_plan_carries_owned_part_count_when_scanned_and_none_when_unscanned() {
        let idx = RelicIndex::new(vec![relic("Axi C3", &["Afuris Prime Barrel", "Afuris Prime Link"])]);
        let owned = HashMap::from([("Axi C3".to_string(), RelicEvidence::Confirmed(1))]);
        let mut owned_parts = crate::OwnedPrimeParts::new();
        crate::owned_parts::apply_count(
            &mut owned_parts,
            &PrimePart { prime: "Afuris Prime".to_string(), part: "Barrel".to_string() },
            3,
            crate::owned::Source::Ocr,
        );

        let plans = mastery_plan(
            &owned,
            &HashMap::new(),
            &HashMap::new(),
            &RelicContext {
                index: &idx,
                mastery: &MasterySet::default(),
                quantities: &PartQuantities::empty(),
                owned_parts: &owned_parts,
            },
        );

        let afuris = plans.iter().find(|p| p.prime == "Afuris Prime").unwrap();
        let barrel = afuris.parts.iter().find(|g| g.part.part == "Barrel").unwrap();
        assert_eq!(barrel.owned, Some(3));
        // Never scanned — unknown, not zero.
        let link = afuris.parts.iter().find(|g| g.part.part == "Link").unwrap();
        assert_eq!(link.owned, None);
    }

    #[test]
    fn mastery_plan_includes_a_seen_only_relic_with_no_confirmed_copy() {
        // A relic that's only been Seen (never confirmed, ADR-0009) must still
        // surface its part — with SeenOnly evidence, not silently omitted, and
        // it must not count toward the farming-budget total.
        let idx = RelicIndex::new(vec![relic("Axi A22", &["Afentis Prime Blueprint"])]);
        let owned = HashMap::from([("Axi A22".to_string(), RelicEvidence::SeenOnly)]);

        let plans = mastery_plan(
            &owned,
            &HashMap::new(),
            &HashMap::new(),
            &RelicContext {
                index: &idx,
                mastery: &MasterySet::default(),
                quantities: &PartQuantities::empty(),
                owned_parts: &crate::OwnedPrimeParts::new(),
            },
        );

        let afentis = plans.iter().find(|p| p.prime == "Afentis Prime").unwrap();
        assert_eq!(afentis.total_owned, 0);
        let blueprint = afentis.parts.iter().find(|g| g.part.part == "Blueprint").unwrap();
        assert_eq!(blueprint.relics[0].evidence, RelicEvidence::SeenOnly);
    }

    #[test]
    fn mastery_plan_surfaces_a_part_whose_only_confirmed_copy_is_radiant() {
        // Regression test for the reported bug: Kompressa Prime's Barrel was
        // missing because the only confirmed copy of its sourcing relic (Lith
        // K12) was Radiant-refined, and the old Intact-only projection
        // dropped it. Goes through the real owned::owned_evidence projection
        // (not a hand-built RelicEvidence) so a regression in that plumbing
        // would also be caught here.
        let idx = RelicIndex::new(vec![relic("Lith K12", &["Kompressa Prime Barrel"])]);
        let mut owned: crate::owned::OwnedRelics = HashMap::new();
        owned.insert(
            "Lith K12".to_string(),
            HashMap::from([(
                crate::owned::Refinement::Radiant,
                crate::owned::OwnedEntry {
                    seen: true,
                    count: Some(wf_cache::Stamped { value: 1, fetched_at: 0 }),
                    source: crate::owned::Source::Ocr,
                },
            )]),
        );
        let evidence = crate::owned::owned_evidence(&owned);

        let plans = mastery_plan(
            &evidence,
            &HashMap::new(),
            &HashMap::new(),
            &RelicContext {
                index: &idx,
                mastery: &MasterySet::default(),
                quantities: &PartQuantities::empty(),
                owned_parts: &crate::OwnedPrimeParts::new(),
            },
        );

        let kompressa = plans.iter().find(|p| p.prime == "Kompressa Prime").unwrap();
        let barrel = kompressa.parts.iter().find(|g| g.part.part == "Barrel").unwrap();
        assert_eq!(barrel.relics[0].evidence, RelicEvidence::Confirmed(1));
        assert_eq!(kompressa.total_owned, 1);
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
    fn active_tier_reward_names_requires_both_owned_and_an_active_tier() {
        let idx = RelicIndex::new(vec![
            relic("Axi A1", &["Volt Prime Blueprint", "Volt Prime Systems Blueprint", "Forma Blueprint"]),
            relic("Axi B2", &["Ember Prime Blueprint"]), // active tier, but not owned
            relic("Meso C3", &["Trinity Prime Blueprint"]), // owned, but tier not active
        ]);
        let owned: HashMap<String, RelicEvidence> = HashMap::from([
            ("Axi A1".to_string(), RelicEvidence::Confirmed(3)),
            ("Meso C3".to_string(), RelicEvidence::SeenOnly),
        ]);
        let active_tiers: HashSet<String> = HashSet::from(["Axi".to_string()]);

        let names = active_tier_reward_names(&owned, &idx, &active_tiers);

        // Owned + active tier: both tradable prime rewards kept, Forma dropped.
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"Volt Prime Blueprint".to_string()));
        assert!(names.contains(&"Volt Prime Systems Blueprint".to_string()));
        assert!(!names.iter().any(|n| n.contains("Ember"))); // active tier, not owned
        assert!(!names.iter().any(|n| n.contains("Trinity"))); // owned, tier not active
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

    fn catalogue_item(slug: &str, name: &str, vaulted: bool) -> wf_data::items::Item {
        wf_data::items::Item {
            slug: slug.to_string(),
            name: name.to_string(),
            ducats: None,
            tags: vec!["prime".to_string()],
            vaulted,
        }
    }

    #[test]
    fn vaulted_rewards_requires_every_source_relic_vaulted() {
        let relics = RelicIndex::new(vec![
            // Sole source of Ember Prime Blueprint, and it's vaulted.
            relic("Meso E1", &["Ember Prime Blueprint"]),
            // Two sources of Trinity Prime Blueprint: one vaulted, one not.
            relic("Axi A1", &["Trinity Prime Blueprint"]),
            relic("Neo N1", &["Trinity Prime Blueprint"]),
        ]);
        let items = ItemIndex::new(vec![
            catalogue_item("meso_e1_relic", "Meso E1", true),
            catalogue_item("axi_a1_relic", "Axi A1", true),
            catalogue_item("neo_n1_relic", "Neo N1", false),
            catalogue_item("ember_prime_blueprint", "Ember Prime Blueprint", false),
            catalogue_item("trinity_prime_blueprint", "Trinity Prime Blueprint", false),
        ]);

        let vaulted = vaulted_rewards(&relics, &items);

        // Every source relic vaulted -> vaulted.
        assert_eq!(vaulted.get("Ember Prime Blueprint"), Some(&true));
        // At least one source relic not vaulted -> not vaulted.
        assert_eq!(vaulted.get("Trinity Prime Blueprint"), Some(&false));
        // No known source relic at all -> absent, not a panic.
        assert_eq!(vaulted.get("Volt Prime Blueprint"), None);
    }
}
