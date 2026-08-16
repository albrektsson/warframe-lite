//! Floor price / Ceiling price / Verdict for one Riven type, computed from a
//! live warframe.market auction-listing snapshot — per
//! `docs/specs/riven-browse-tab.md` §3, settled across issues #98/#102/#103.
//! No historical/completed-sale data exists for rivens anywhere in the API
//! (`docs/research/warframe-market-riven-pricing-api.md` §6), so this is
//! necessarily built entirely from the current live snapshot's depth and
//! outlier-trimmed percentiles, not a trend.
//!
//! Pure functions over [`ListingInput`] — a slim, crate-local mirror of
//! [`wf_data::riven_market::RivenAuction`]'s three price-relevant fields
//! (all already `pub` there) — rather than the full auction type, so this
//! module stays independently testable without reaching into
//! `wf-data`-internal fields.

use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

/// Recency window for the listing filter (§3.1) — a placeholder, not
/// calibrated against real distribution data (spec §3.4's closing note).
pub const RECENCY_WINDOW_DAYS: i64 = 30;
/// Minimum price-bearing listings (post-filter, post-extraction) before
/// Floor is trusted enough to drive a Verdict, and before Ceiling is shown
/// at full confidence (§3.3) — a placeholder.
pub const MIN_LISTINGS: usize = 5;
/// A Riven type's Floor below this absolute plat value reads as "not worth
/// real plat" (§3.4) — a placeholder, deliberately not scaled to the
/// weapon's own Ceiling/Median (a ratio approach was tried and rejected;
/// see the spec's rationale).
pub const WORTHLESS_THRESHOLD_PLAT: u32 = 15;

/// The three fields [`evaluate`] needs off one listing — a slim mirror of
/// [`wf_data::riven_market::RivenAuction`]'s public price fields. Owner
/// status is deliberately absent: per spec §3.1, seller reachability plays
/// no role in Floor/Ceiling (unlike `market.rs`'s Prime Part pricing) — a
/// recency-only filter, so it can't skew toward whichever timezone happens
/// to be awake when the player checks.
#[derive(Debug, Clone, Copy)]
pub struct ListingInput {
    pub is_direct_sell: bool,
    pub buyout_price: Option<u32>,
    pub top_bid: Option<u32>,
    pub updated: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    LikelyKeep,
    LikelyDissolve,
    /// Too few real listings to trust Floor for a keep/dissolve call —
    /// abstain rather than guess (§3.4).
    InsufficientData,
}

/// The computed Floor/Ceiling/Verdict for one Riven type — a group-level
/// fact (CONTEXT.md's Verdict entry), not per owned copy.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RivenTypeVerdict {
    /// `None` only when there are zero price-bearing listings at all (no
    /// percentile is definable). Otherwise always populated — even when the
    /// price-bearing set is below [`MIN_LISTINGS`], Floor still displays its
    /// caveated computed number; only the *Verdict* abstains (§3.3).
    pub floor: Option<u32>,
    /// Same "always computed when any signal exists" rule as `floor`.
    pub ceiling: Option<u32>,
    /// Below [`MIN_LISTINGS`], Ceiling is still shown but flagged
    /// low-confidence (§3.3) — unlike Floor, it's never hard-gated, since
    /// it's informational upside, not the load-bearing number the Verdict
    /// depends on.
    pub ceiling_low_confidence: bool,
    pub verdict: Verdict,
}

/// Compute the Floor/Ceiling/Verdict for one Riven type from its listing
/// snapshot, as of `now` (passed explicitly rather than read internally, so
/// this stays a pure, deterministically-testable function).
pub fn evaluate(listings: &[ListingInput], now: OffsetDateTime) -> RivenTypeVerdict {
    let mut prices = price_signals(listings, now);
    prices.sort_by(|a, b| a.partial_cmp(b).expect("prices are never NaN"));

    let floor = percentile(&prices, 10.0);
    let ceiling = percentile(&prices, 90.0);
    let low_confidence = prices.len() < MIN_LISTINGS;

    let verdict = match floor {
        _ if low_confidence => Verdict::InsufficientData,
        Some(f) if f < WORTHLESS_THRESHOLD_PLAT as f64 => Verdict::LikelyDissolve,
        Some(_) => Verdict::LikelyKeep,
        None => Verdict::InsufficientData,
    };

    RivenTypeVerdict {
        floor: floor.map(|f| f.round() as u32),
        ceiling: ceiling.map(|c| c.round() as u32),
        ceiling_low_confidence: low_confidence,
        verdict,
    }
}

/// §3.1 recency filter + §3.2 price extraction, combined: every listing
/// `updated` within [`RECENCY_WINDOW_DAYS`] of `now`, reduced to its one
/// price signal (buyout for a direct sell; `top_bid` for a bidding auction,
/// *only* if a bid exists — a bidding listing with no bid yet carries no
/// price signal and is dropped, not counted as zero).
fn price_signals(listings: &[ListingInput], now: OffsetDateTime) -> Vec<f64> {
    listings
        .iter()
        .filter(|l| is_recent(l, now))
        .filter_map(price_of)
        .map(|p| p as f64)
        .collect()
}

fn is_recent(listing: &ListingInput, now: OffsetDateTime) -> bool {
    match listing.updated {
        // No timestamp at all is treated as stale (conservative — this
        // filter's whole purpose is trusting only listings with a known,
        // recent `updated`).
        None => false,
        Some(updated) => now - updated <= Duration::days(RECENCY_WINDOW_DAYS),
    }
}

fn price_of(listing: &ListingInput) -> Option<u32> {
    if listing.is_direct_sell {
        listing.buyout_price
    } else {
        listing.top_bid
    }
}

/// `p`th percentile (0-100) of an already-sorted, non-empty-or-empty slice,
/// via standard linear interpolation between ranks (NumPy-default style) —
/// so a small sample doesn't collapse to exactly `min()`/`max()`. `None` for
/// an empty slice (nothing to compute a percentile of).
fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    match sorted.len() {
        0 => None,
        1 => Some(sorted[0]),
        n => {
            let rank = (p / 100.0) * (n - 1) as f64;
            let lower = rank.floor() as usize;
            let upper = rank.ceil() as usize;
            if lower == upper {
                Some(sorted[lower])
            } else {
                let frac = rank - lower as f64;
                Some(sorted[lower] + (sorted[upper] - sorted[lower]) * frac)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    fn direct_sell(price: u32, days_ago: i64) -> ListingInput {
        ListingInput {
            is_direct_sell: true,
            buyout_price: Some(price),
            top_bid: None,
            updated: Some(now() - Duration::days(days_ago)),
        }
    }

    fn bidding(top_bid: Option<u32>, days_ago: i64) -> ListingInput {
        ListingInput {
            is_direct_sell: false,
            buyout_price: None,
            top_bid,
            updated: Some(now() - Duration::days(days_ago)),
        }
    }

    #[test]
    fn no_listings_at_all_is_insufficient_data_with_no_floor_or_ceiling() {
        let result = evaluate(&[], now());
        assert_eq!(result.floor, None);
        assert_eq!(result.ceiling, None);
        assert_eq!(result.verdict, Verdict::InsufficientData);
        assert!(result.ceiling_low_confidence);
    }

    #[test]
    fn below_min_listings_abstains_even_though_floor_and_ceiling_still_compute() {
        // 4 listings, all well above the worthless threshold — still
        // abstains purely on count, per §3.3/§3.4.
        let listings: Vec<_> = (0..4).map(|_| direct_sell(100, 1)).collect();
        let result = evaluate(&listings, now());
        assert_eq!(result.verdict, Verdict::InsufficientData);
        assert_eq!(result.floor, Some(100));
        assert!(result.ceiling_low_confidence);
    }

    #[test]
    fn exactly_min_listings_no_longer_abstains() {
        let listings: Vec<_> = (0..MIN_LISTINGS).map(|_| direct_sell(100, 1)).collect();
        let result = evaluate(&listings, now());
        assert_ne!(result.verdict, Verdict::InsufficientData);
        assert!(!result.ceiling_low_confidence);
    }

    #[test]
    fn floor_below_worthless_threshold_is_likely_dissolve() {
        let listings: Vec<_> = (0..MIN_LISTINGS)
            .map(|_| direct_sell(WORTHLESS_THRESHOLD_PLAT - 1, 1))
            .collect();
        let result = evaluate(&listings, now());
        assert_eq!(result.verdict, Verdict::LikelyDissolve);
    }

    #[test]
    fn floor_exactly_at_worthless_threshold_is_likely_keep_not_dissolve() {
        // The comparison is strict `<`, so the threshold value itself keeps.
        let listings: Vec<_> = (0..MIN_LISTINGS)
            .map(|_| direct_sell(WORTHLESS_THRESHOLD_PLAT, 1))
            .collect();
        let result = evaluate(&listings, now());
        assert_eq!(result.verdict, Verdict::LikelyKeep);
    }

    #[test]
    fn a_bidding_listing_with_no_bid_yet_contributes_no_price_signal() {
        let listings = vec![
            bidding(None, 1),
            bidding(None, 1),
            direct_sell(50, 1),
            direct_sell(50, 1),
            direct_sell(50, 1),
        ];
        let result = evaluate(&listings, now());
        // Only the 3 direct-sell listings count toward MIN_LISTINGS, so this
        // still abstains despite 5 raw listings being present.
        assert_eq!(result.verdict, Verdict::InsufficientData);
    }

    #[test]
    fn a_bidding_listing_with_a_bid_uses_the_top_bid_as_its_price() {
        let listings = vec![bidding(Some(200), 1)];
        let result = evaluate(&listings, now());
        assert_eq!(result.floor, Some(200));
        assert_eq!(result.ceiling, Some(200));
    }

    #[test]
    fn listings_older_than_the_recency_window_are_dropped() {
        let listings: Vec<_> = (0..MIN_LISTINGS)
            .map(|_| direct_sell(100, RECENCY_WINDOW_DAYS + 1))
            .collect();
        let result = evaluate(&listings, now());
        assert_eq!(result.floor, None);
        assert_eq!(result.verdict, Verdict::InsufficientData);
    }

    #[test]
    fn a_listing_exactly_at_the_recency_boundary_still_counts() {
        let listings: Vec<_> = (0..MIN_LISTINGS)
            .map(|_| direct_sell(100, RECENCY_WINDOW_DAYS))
            .collect();
        let result = evaluate(&listings, now());
        assert_eq!(result.floor, Some(100));
    }

    #[test]
    fn a_listing_with_no_updated_timestamp_is_dropped() {
        let listings = vec![ListingInput {
            is_direct_sell: true,
            buyout_price: Some(100),
            top_bid: None,
            updated: None,
        }];
        let result = evaluate(&listings, now());
        assert_eq!(result.floor, None);
    }

    #[test]
    fn owner_reachability_plays_no_role_only_listing_input_has_no_status_field() {
        // Compile-time proof, effectively: ListingInput has no owner/status
        // field at all for `evaluate` to filter on (see the module doc).
        let listings = vec![direct_sell(100, 1)];
        let _ = evaluate(&listings, now());
    }

    #[test]
    fn a_single_joke_listing_outlier_does_not_dominate_ceiling_with_enough_real_depth() {
        // Mirrors the live dual_toxocyst finding (research doc §5): a dense
        // low cluster plus one wildly-priced outlier. The 90th percentile
        // should sit far below the outlier, not near it.
        let mut listings: Vec<ListingInput> = (0..20).map(|_| direct_sell(50, 1)).collect();
        listings.push(direct_sell(888_888, 1));
        let result = evaluate(&listings, now());
        let ceiling = result.ceiling.expect("ceiling computed");
        assert!(
            ceiling < 200,
            "ceiling {ceiling} should not be dragged toward the 888888 outlier"
        );
    }

    #[test]
    fn percentile_of_a_single_value_is_that_value() {
        assert_eq!(percentile(&[42.0], 10.0), Some(42.0));
        assert_eq!(percentile(&[42.0], 90.0), Some(42.0));
    }

    #[test]
    fn percentile_interpolates_linearly_between_ranks() {
        // NumPy-default linear interpolation, worked example: [10, 20, 30, 40],
        // 50th percentile: rank = 0.5 * 3 = 1.5 -> interpolate between index
        // 1 (20) and index 2 (30) at frac 0.5 -> 25.
        let sorted = vec![10.0, 20.0, 30.0, 40.0];
        assert_eq!(percentile(&sorted, 50.0), Some(25.0));
    }

    #[test]
    fn percentile_of_empty_is_none() {
        assert_eq!(percentile(&[], 50.0), None);
    }
}
