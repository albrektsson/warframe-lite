//! Riven auction lookups from warframe.market.
//!
//! A **separate** API surface from [`crate::market::MarketClient`] — rivens
//! trade via `/v1/auctions/search`, not the `/v2/orders/item/{slug}` fixed
//! price-book that prices Prime Parts, and a Riven type's slug comes from a
//! *different* catalogue (`/v2/riven/weapons`, not `/v2/items`). Confirmed
//! live, 2026-08-15, per `docs/research/warframe-market-riven-pricing-api.md`:
//! no official docs exist for this surface at all (only an unofficial,
//! reverse-engineered OpenAPI spec), so every shape here was captured from
//! real, live responses, re-verified while writing this module rather than
//! trusted from the research doc's snapshot alone.
//!
//! Riven auctions never migrated to v2 and still live under the **v1** base
//! URL — the only v1 surface confirmed still working, since v1's own
//! `/orders` endpoint now 403s (see `market.rs`'s doc). The weapon catalogue
//! (`weapon_catalogue`), by contrast, *is* v2.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use time::OffsetDateTime;

use crate::rate_limit::TokenBucket;

const V1_BASE: &str = "https://api.warframe.market/v1";
const V2_BASE: &str = "https://api.warframe.market/v2";
/// `/v1/auctions/search`'s own budget (ADR-0020) — warframe.market documents
/// a general 3 req/s budget across the API, but a much tighter ~10-20 req/min
/// budget specifically for this endpoint (confirmed live, 2026-08-15, per
/// `docs/research/warframe-market-riven-pricing-api.md`). 15/min sits in the
/// middle of that range.
const AUCTIONS_SEARCH_RATE_CAPACITY: u32 = 15;
const AUCTIONS_SEARCH_RATE_WINDOW: Duration = Duration::from_secs(60);

/// One entry from `/v2/riven/weapons` — the catalogue a Riven type's
/// `weapon_url_name` slug comes from, distinct from [`crate::market`]'s
/// `/v2/items` catalogue (a plain item slug like `"toxocyst"` may not exist
/// here at all — confirmed live, only `"dual_toxocyst"` does).
#[derive(Debug, Clone, Deserialize)]
pub struct RivenWeapon {
    pub slug: String,
    /// DE's own internal unique name for this weapon (e.g.
    /// `/Lotus/Weapons/Infested/Pistols/InfVomitGun/InfVomitGunWep`) — the
    /// same string a riven fingerprint's `compat` field carries (see
    /// `wf_mem::riven::Riven::weapon_unique_name`), so this is the join key
    /// between a decoded owned riven and the market slug needed to price it.
    #[serde(rename = "gameRef")]
    pub game_ref: String,
    #[serde(default, rename = "i18n")]
    i18n: I18n,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct I18n {
    en: Option<Localized>,
}

#[derive(Debug, Clone, Deserialize)]
struct Localized {
    name: String,
}

impl RivenWeapon {
    pub fn name(&self) -> &str {
        self.i18n
            .en
            .as_ref()
            .map(|l| l.name.as_str())
            .unwrap_or(&self.slug)
    }
}

#[derive(Debug, Deserialize)]
struct WeaponsResponse {
    data: Vec<RivenWeapon>,
}

/// Fetch the full riven-eligible weapon catalogue — `GET /v2/riven/weapons`.
pub async fn weapon_catalogue(
    client: &reqwest::Client,
    platform: &str,
) -> anyhow::Result<Vec<RivenWeapon>> {
    let url = format!("{V2_BASE}/riven/weapons");
    tracing::debug!("GET {url}");
    let resp = client
        .get(&url)
        .header("Platform", platform)
        .send()
        .await?
        .error_for_status()?
        .json::<WeaponsResponse>()
        .await?;
    Ok(resp.data)
}

/// One rolled attribute on a listed riven (a decoded, already-displayable
/// stat percentage from warframe.market's own side — distinct from
/// `wf_relic::riven_decode`'s own decode of a raw fingerprint `Value`,
/// which this crate has no access to for another player's listing).
#[derive(Debug, Clone, Deserialize)]
pub struct AuctionAttribute {
    pub value: f64,
    pub positive: bool,
    pub url_name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct AuctionItem {
    #[serde(default)]
    attributes: Vec<AuctionAttribute>,
    polarity: Option<String>,
    mastery_level: Option<u32>,
    re_rolls: Option<u32>,
    mod_rank: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
struct AuctionOwner {
    /// "ingame", "online", or "offline" — the same vocabulary
    /// [`crate::market`]'s orders carry, just nested one level differently
    /// (`owner.status` here vs. `user.status` there).
    status: String,
}

/// One riven listing. `is_direct_sell` is the field that actually
/// distinguishes a fixed "buy now" listing from a real bidding auction —
/// not merely which price fields are set (a bidding auction may still carry
/// an optional `buyout_price` as an instant-buy escape hatch).
#[derive(Debug, Clone, Deserialize)]
pub struct RivenAuction {
    pub is_direct_sell: bool,
    pub buyout_price: Option<u32>,
    /// The current highest bid on a bidding auction, if any bids exist yet
    /// (`None` on a fresh bidding auction with no bids — carries zero price
    /// signal). Always `None` on a direct-sell listing.
    pub top_bid: Option<u32>,
    #[serde(default, deserialize_with = "deserialize_optional_timestamp")]
    pub updated: Option<OffsetDateTime>,
    owner: AuctionOwner,
    item: AuctionItem,
}

impl RivenAuction {
    /// "ingame", "online", or "offline". Exposed for completeness — per
    /// `docs/specs/riven-browse-tab.md` §3.1, seller reachability plays no
    /// role in Floor/Ceiling filtering (unlike `market.rs`'s Prime Part
    /// pricing), but the field is still real data a caller may want to
    /// show.
    pub fn owner_status(&self) -> &str {
        &self.owner.status
    }

    pub fn attributes(&self) -> &[AuctionAttribute] {
        &self.item.attributes
    }

    pub fn polarity(&self) -> Option<&str> {
        self.item.polarity.as_deref()
    }

    pub fn mastery_level(&self) -> Option<u32> {
        self.item.mastery_level
    }

    pub fn re_rolls(&self) -> Option<u32> {
        self.item.re_rolls
    }

    pub fn mod_rank(&self) -> Option<u32> {
        self.item.mod_rank
    }
}

fn deserialize_optional_timestamp<'de, D>(
    deserializer: D,
) -> Result<Option<OffsetDateTime>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<String> = Option::deserialize(deserializer)?;
    Ok(raw.and_then(|s| {
        OffsetDateTime::parse(&s, &time::format_description::well_known::Rfc3339).ok()
    }))
}

#[derive(Debug, Deserialize)]
struct AuctionsPayload {
    auctions: Vec<RivenAuction>,
}

#[derive(Debug, Deserialize)]
struct AuctionsResponse {
    payload: AuctionsPayload,
}

/// A warframe.market riven-auction client bound to a platform. Cheap to
/// clone (both `reqwest::Client` and the rate limiter are internally
/// `Arc`-backed) — callers that need [`auctions_for`](Self::auctions_for)'s
/// rate limit to actually hold across a whole session (rather than resetting
/// every time a fresh client is built) should construct one instance and
/// clone it, not call [`new`](Self::new) again per call site.
#[derive(Clone)]
pub struct RivenMarketClient {
    client: reqwest::Client,
    platform: String,
    /// Scoped to `auctions_for` specifically (ADR-0020) — shared, `Arc`-held
    /// state so every clone of this client throttles against the same
    /// budget, since a fresh bucket per clone would defeat the point.
    rate_limiter: Arc<TokenBucket>,
}

impl RivenMarketClient {
    pub fn new(client: reqwest::Client, platform: impl Into<String>) -> Self {
        Self {
            client,
            platform: platform.into(),
            rate_limiter: Arc::new(TokenBucket::new(
                AUCTIONS_SEARCH_RATE_CAPACITY,
                AUCTIONS_SEARCH_RATE_WINDOW,
            )),
        }
    }

    /// Every open (unclosed, visible) listing for `weapon_url_name` —
    /// `GET /v1/auctions/search?type=riven&weapon_url_name={slug}`. Hard-capped
    /// at 500 listings server-side with no working pagination (confirmed
    /// live — see the research doc §5); this returns whatever the API gives
    /// back, capping/trimming is the caller's concern (see
    /// `wf_relic::riven_pricing`).
    ///
    /// Blocks under `rate_limiter` (ADR-0020) before firing the request —
    /// underneath any concurrency cap a caller applies across several
    /// weapons at once (e.g. `wf-browse`'s `PRICE_FETCH_CONCURRENCY`-wide
    /// `buffer_unordered`), so a burst across many distinct weapons still
    /// can't exceed this endpoint's own tighter budget.
    pub async fn auctions_for(&self, weapon_url_name: &str) -> anyhow::Result<Vec<RivenAuction>> {
        self.rate_limiter.acquire().await;
        let url = format!("{V1_BASE}/auctions/search");
        tracing::debug!("GET {url}?type=riven&weapon_url_name={weapon_url_name}");
        let resp = self
            .client
            .get(&url)
            .query(&[("type", "riven"), ("weapon_url_name", weapon_url_name)])
            .header("Platform", self.platform.as_str())
            .send()
            .await?
            .error_for_status()?
            .json::<AuctionsResponse>()
            .await?;
        Ok(resp.payload.auctions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_the_weapon_catalogue_shape() {
        let json = r#"{"apiVersion":"6","data":[
            {"id":"5c5ca81696e8d2003834fdc1","slug":"dual_toxocyst",
             "gameRef":"/Lotus/Weapons/Infested/Pistols/InfVomitGun/InfVomitGunWep",
             "group":"secondary","rivenType":"pistol","disposition":1.35,
             "reqMasteryRank":11,
             "i18n":{"en":{"name":"Dual Toxocyst"}}}
        ],"error":null}"#;
        let resp: WeaponsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.data.len(), 1);
        assert_eq!(resp.data[0].slug, "dual_toxocyst");
        assert_eq!(resp.data[0].name(), "Dual Toxocyst");
    }

    #[test]
    fn weapon_name_falls_back_to_slug_when_i18n_is_missing() {
        let weapon = RivenWeapon {
            slug: "kulstar".to_string(),
            game_ref: "/Lotus/Weapons/Grineer/Pistols/GrnTorpedoPistol/GrnTorpedoPistol"
                .to_string(),
            i18n: I18n::default(),
        };
        assert_eq!(weapon.name(), "kulstar");
    }

    #[test]
    fn deserializes_a_direct_sell_listing() {
        // Real payload shape, captured live (research doc §4).
        let json = r#"{"payload":{"auctions":[{
            "starting_price": 888888, "buyout_price": 888888, "minimal_reputation": 0,
            "visible": true, "platform": "pc", "crossplay": true, "closed": false,
            "top_bid": null, "created": "2025-05-24T13:31:18.000+00:00",
            "updated": "2025-08-27T16:26:15.000+00:00", "is_direct_sell": true,
            "id": "6831ca26ddb959000825f9f2",
            "owner": {"reputation": 5999, "ingame_name": "Forever_Prime", "status": "offline"},
            "item": {
                "weapon_url_name": "dual_toxocyst",
                "attributes": [
                    {"value": 161.7, "positive": true, "url_name": "multishot"},
                    {"value": -124.7, "positive": false, "url_name": "puncture_damage"}
                ],
                "polarity": "vazarin", "mastery_level": 14, "re_rolls": 24,
                "mod_rank": 8, "type": "riven", "name": "sati-critatis"
            },
            "private": false
        }]}}"#;
        let resp: AuctionsResponse = serde_json::from_str(json).unwrap();
        let auctions = resp.payload.auctions;
        assert_eq!(auctions.len(), 1);
        let a = &auctions[0];
        assert!(a.is_direct_sell);
        assert_eq!(a.buyout_price, Some(888_888));
        assert_eq!(a.top_bid, None);
        assert_eq!(a.owner_status(), "offline");
        assert_eq!(a.polarity(), Some("vazarin"));
        assert_eq!(a.mastery_level(), Some(14));
        assert_eq!(a.re_rolls(), Some(24));
        assert_eq!(a.mod_rank(), Some(8));
        assert_eq!(a.attributes().len(), 2);
        assert!(a.updated.is_some());
    }

    #[test]
    fn deserializes_an_open_bidding_listing_with_no_buyout() {
        let json = r#"{"payload":{"auctions":[{
            "starting_price": 20000, "buyout_price": null, "minimal_reputation": 0,
            "visible": true, "platform": "pc", "crossplay": true, "closed": false,
            "top_bid": 200000, "created": "2025-07-03T14:59:40.000+00:00",
            "updated": "2026-04-29T02:48:29.000+00:00", "is_direct_sell": false,
            "id": "68669adc1a935b0006a3494e",
            "owner": {"reputation": 33, "ingame_name": "Smoothie26", "status": "ingame"},
            "item": {
                "weapon_url_name": "dual_toxocyst",
                "attributes": [],
                "polarity": "madurai", "mastery_level": 13, "re_rolls": 70,
                "mod_rank": 8, "type": "riven", "name": "crita-acrican"
            },
            "private": false
        }]}}"#;
        let resp: AuctionsResponse = serde_json::from_str(json).unwrap();
        let a = &resp.payload.auctions[0];
        assert!(!a.is_direct_sell);
        assert_eq!(a.buyout_price, None);
        assert_eq!(a.top_bid, Some(200_000));
    }

    #[test]
    fn a_bidding_listing_with_no_bids_yet_has_no_top_bid() {
        let json = r#"{"payload":{"auctions":[{
            "starting_price": 5000, "buyout_price": null, "minimal_reputation": 0,
            "visible": true, "platform": "pc", "crossplay": true, "closed": false,
            "top_bid": null, "created": "2025-07-03T14:59:40.000+00:00",
            "updated": "2026-04-29T02:48:29.000+00:00", "is_direct_sell": false,
            "id": "abc",
            "owner": {"reputation": 1, "ingame_name": "Fresh", "status": "online"},
            "item": {
                "weapon_url_name": "dual_toxocyst", "attributes": [],
                "polarity": "madurai", "mastery_level": 8, "re_rolls": 0,
                "mod_rank": 0, "type": "riven", "name": "fresh-riven"
            },
            "private": false
        }]}}"#;
        let resp: AuctionsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.payload.auctions[0].top_bid, None);
    }
}
