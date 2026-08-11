//! Owned Prime Part ducat-efficiency ranking for the Ducats tab: pairs each
//! owned part's ducat value (already resolved catalogue-wide via
//! [`crate::part_market`]) with its warframe.market plat price and computes
//! **Ducat efficiency** (see CONTEXT.md) — ducats ÷ plat — the basis for
//! ranking which owned parts are most worth trading in for ducats over
//! listing on the market.

use std::collections::HashMap;

use crate::mastery::{MasterySet, PrimePart};
use crate::owned_parts::OwnedPrimeParts;
use crate::part_market::{reward_label, PartMarketInfo};
use crate::part_quantities::PartQuantities;

/// One owned Prime Part's ducat-efficiency row.
#[derive(Debug, Clone)]
pub struct DucatPick {
    pub part: PrimePart,
    pub owned: u32,
    /// The full build's requirement for this part, if known (see ADR-0011) —
    /// never guessed at 1 when a lookup misses.
    pub build_quantity: Option<u32>,
    pub ducats: Option<u32>,
    /// Lowest active sell price, if resolved.
    pub plat: Option<u32>,
    /// `ducats / plat`, `None` when either side is unresolved.
    pub efficiency: Option<f64>,
    /// Whether this part's Built prime has already been mastered.
    pub mastered: bool,
}

/// Rank every owned Prime Part by [Ducat efficiency](https://en.wikipedia.org/wiki/Ducat)
/// (descending; unresolved efficiency sorts last, mirroring
/// [`crate::relics::farm_picks`]'s unresolved-price convention), pairing
/// already-resolved ducat info (`part_market`) with `plat_prices` (keyed by
/// [`reward_label`], resolved by the caller's own bounded-concurrency market
/// fetch — mirroring `sell_picks`/`farm_picks`'s launch-time pricing).
pub fn ducat_picks(
    owned_parts: &OwnedPrimeParts,
    part_market: &HashMap<PrimePart, PartMarketInfo>,
    plat_prices: &HashMap<String, Option<u32>>,
    quantities: &PartQuantities,
    mastery: &MasterySet,
) -> Vec<DucatPick> {
    let mut picks: Vec<DucatPick> = owned_parts
        .iter()
        .flat_map(|(prime, parts)| {
            parts.iter().map(move |(part, count)| {
                let pp = PrimePart { prime: prime.clone(), part: part.clone() };
                let label = reward_label(&pp);
                let ducats = part_market.get(&pp).and_then(|i| i.ducats);
                let plat = plat_prices.get(&label).copied().flatten();
                let efficiency = match (ducats, plat) {
                    (Some(d), Some(p)) if p > 0 => Some(d as f64 / p as f64),
                    _ => None,
                };
                DucatPick {
                    mastered: mastery.is_mastered(&label),
                    owned: count.count.value,
                    build_quantity: quantities.get(&pp),
                    ducats,
                    plat,
                    efficiency,
                    part: pp,
                }
            })
        })
        .collect();
    picks.sort_by(|a, b| {
        b.efficiency
            .unwrap_or(0.0)
            .partial_cmp(&a.efficiency.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    picks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(entries: Vec<(&str, &str, u32)>) -> OwnedPrimeParts {
        let mut owned = OwnedPrimeParts::new();
        for (prime, part, count) in entries {
            crate::owned_parts::apply_count(
                &mut owned,
                &PrimePart { prime: prime.to_string(), part: part.to_string() },
                count,
                crate::owned::Source::Ocr,
            );
        }
        owned
    }

    fn market(entries: Vec<(&str, &str, u32)>) -> HashMap<PrimePart, PartMarketInfo> {
        entries
            .into_iter()
            .map(|(prime, part, ducats)| {
                (
                    PrimePart { prime: prime.to_string(), part: part.to_string() },
                    PartMarketInfo { vaulted: false, ducats: Some(ducats) },
                )
            })
            .collect()
    }

    fn mastered_set() -> MasterySet {
        // Mirrors mastery.rs's own test fixture: Ember Prime mastered, Loki
        // Prime never mentioned (so is_mastered is false for it).
        MasterySet::from_xp([("/Lotus/Powersuits/Ember/EmberPrime".to_string(), 9_000_000)])
    }

    #[test]
    fn computes_efficiency_and_sorts_descending() {
        let owned_parts = owned(vec![("Ember Prime", "Systems", 2), ("Ember Prime", "Chassis", 1)]);
        let part_market = market(vec![("Ember Prime", "Systems", 45), ("Ember Prime", "Chassis", 15)]);
        let mut plat_prices = HashMap::new();
        plat_prices.insert(reward_label(&PrimePart {
            prime: "Ember Prime".to_string(),
            part: "Systems".to_string(),
        }), Some(10)); // 45/10 = 4.5
        plat_prices.insert(reward_label(&PrimePart {
            prime: "Ember Prime".to_string(),
            part: "Chassis".to_string(),
        }), Some(30)); // 15/30 = 0.5

        let picks = ducat_picks(
            &owned_parts,
            &part_market,
            &plat_prices,
            &PartQuantities::empty(),
            &mastered_set(),
        );

        assert_eq!(picks.len(), 2);
        assert_eq!(picks[0].part.part, "Systems");
        assert_eq!(picks[0].efficiency, Some(4.5));
        assert_eq!(picks[0].owned, 2);
        assert_eq!(picks[1].part.part, "Chassis");
        assert_eq!(picks[1].efficiency, Some(0.5));
    }

    #[test]
    fn missing_price_or_ducats_gives_no_efficiency_and_sorts_last() {
        let owned_parts =
            owned(vec![("Ember Prime", "Systems", 1), ("Ember Prime", "Blueprint", 1)]);
        let part_market = market(vec![("Ember Prime", "Systems", 45)]); // Blueprint unresolved
        let mut plat_prices = HashMap::new();
        plat_prices.insert(
            reward_label(&PrimePart { prime: "Ember Prime".to_string(), part: "Systems".to_string() }),
            Some(10),
        );
        // No plat price entry at all for Blueprint.

        let picks = ducat_picks(
            &owned_parts,
            &part_market,
            &plat_prices,
            &PartQuantities::empty(),
            &mastered_set(),
        );

        assert_eq!(picks[0].part.part, "Systems");
        assert!(picks[0].efficiency.is_some());
        assert_eq!(picks[1].part.part, "Blueprint");
        assert_eq!(picks[1].efficiency, None);
        assert_eq!(picks[1].ducats, None);
    }

    #[test]
    fn flags_mastered_status_per_part() {
        let owned_parts = owned(vec![("Ember Prime", "Systems", 1), ("Loki Prime", "Systems", 1)]);
        let part_market = market(vec![("Ember Prime", "Systems", 45), ("Loki Prime", "Systems", 15)]);
        let picks = ducat_picks(
            &owned_parts,
            &part_market,
            &HashMap::new(),
            &PartQuantities::empty(),
            &mastered_set(),
        );

        let ember = picks.iter().find(|p| p.part.prime == "Ember Prime").unwrap();
        let loki = picks.iter().find(|p| p.part.prime == "Loki Prime").unwrap();
        assert!(ember.mastered);
        assert!(!loki.mastered);
    }
}
