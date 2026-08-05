//! Full per-Prime parts list (bill of materials), independent of relic
//! ownership — the "Buy or Farm" tab's data source. Unlike [`crate::mastery_plan`]
//! (a fissure-farming planner scoped to parts you already have relic evidence
//! for), this enumerates every part of every unmastered Prime via
//! [`PartQuantities`] and flags which ones still need buying or farming.

use std::collections::HashMap;

use crate::mastery::{MasterySet, PrimePart};
use crate::owned::RelicEvidence;
use crate::part_quantities::PartQuantities;
use crate::relics::{all_relic_sources, relic_sourced_parts, RelicIndex, RelicOption};

/// One still-missing Prime Part: you don't yet own enough of it (or any, if
/// never scanned) and no relic evidence covers it.
#[derive(Debug, Clone)]
pub struct BomGap {
    pub part: PrimePart,
    /// How many a full build needs — always known here, since the part came
    /// from [`PartQuantities`] in the first place (see ADR-0011).
    pub build_quantity: Option<u32>,
    /// How many the player already owns, per the Inventory/Sell screen scan —
    /// `None` when never scanned (unknown, not zero).
    pub owned: Option<u32>,
    /// Relics that can drop this part — cheapest first, regardless of
    /// ownership (a gap has no owned evidence by construction, so these are
    /// candidates to go buy or farm, not relics already in hand; contrast
    /// [`crate::PrimeRelicSource`]). Empty when no relic source is known at
    /// all.
    pub relics: Vec<RelicOption>,
}

/// One unmastered Prime's full parts list, split into what's still missing
/// and what's already covered (by owned parts or relic evidence).
#[derive(Debug, Clone)]
pub struct BomPlan {
    /// Built prime name, e.g. "Ember Prime".
    pub prime: String,
    /// Parts that still need buying or farming, cheapest-relic-first, part
    /// name-sorted.
    pub gaps: Vec<BomGap>,
    /// Count of this prime's parts NOT in `gaps`.
    pub covered: usize,
    /// Lowest market sell price in platinum for the whole built-prime Set, if
    /// resolved.
    pub set_plat: Option<u32>,
    /// Sum of each gap's cheapest relic price — `None` only when no gap has
    /// any resolved price at all (as opposed to some gaps simply having no
    /// price yet, which are skipped in the sum).
    pub cost_to_fill: Option<u32>,
}

/// Every unmastered Prime known to `quantities`, sorted alphabetically — the
/// scope of the Buy-or-Farm tab (a mastered Prime has nothing left to buy or
/// farm).
pub fn unmastered_primes(quantities: &PartQuantities, mastery: &MasterySet) -> Vec<String> {
    quantities.primes().into_iter().filter(|p| !mastery.is_mastered(p)).map(str::to_string).collect()
}

/// Build the full-BOM "Buy or Farm" view: for every unmastered Prime, every
/// known component, split into gaps (need buying/farming) and covered parts.
///
/// A part is a gap unless the player already owns at least as many as a
/// build needs (per the Inventory/Sell screen scan) OR has relic evidence
/// (confirmed or merely seen) for it — an unscanned owned count doesn't by
/// itself clear a part, since nothing proves ownership yet.
///
/// `prices`/`set_prices`/`quantities`/`owned_parts` are caller-supplied,
/// already-resolved lookups (see [`crate::mastery_plan`]'s docs for the same
/// pattern) — a missing key is treated the same as an explicit `None`.
pub fn buy_or_farm_plan(
    owned: &HashMap<String, RelicEvidence>,
    prices: &HashMap<String, Option<u32>>,
    set_prices: &HashMap<String, Option<u32>>,
    index: &RelicIndex,
    mastery: &MasterySet,
    quantities: &PartQuantities,
    owned_parts: &crate::OwnedPrimeParts,
) -> Vec<BomPlan> {
    // Two different relic lookups: `owned_by_part` (relics the player has
    // evidence for) decides coverage; `all_sources_by_part` (every relic in
    // the catalogue) supplies what to go buy/farm for an actual gap — a gap
    // has no owned evidence by construction, so the former is always empty
    // for it.
    let (owned_by_part, _) = relic_sourced_parts(owned, prices, index, mastery);
    let all_sources_by_part = all_relic_sources(prices, index, mastery);

    unmastered_primes(quantities, mastery)
        .into_iter()
        .map(|prime| {
            let owned_parts_for_prime = owned_by_part.get(&prime);
            let mut gaps: Vec<BomGap> = Vec::new();
            let mut covered = 0usize;

            for (part_label, quantity) in quantities.parts_for(&prime) {
                let pp = PrimePart { prime: prime.clone(), part: part_label };
                let build_quantity = Some(quantity);
                let owned_count = crate::owned_parts::get(owned_parts, &pp);

                let owned_meets_need =
                    matches!((owned_count, build_quantity), (Some(o), Some(n)) if o >= n);
                let has_relic_evidence =
                    owned_parts_for_prime.is_some_and(|m| m.get(&pp).is_some_and(|v| !v.is_empty()));
                if owned_meets_need || has_relic_evidence {
                    covered += 1;
                } else {
                    let relics = all_sources_by_part.get(&pp).cloned().unwrap_or_default();
                    gaps.push(BomGap { part: pp, build_quantity, owned: owned_count, relics });
                }
            }
            gaps.sort_by(|a, b| a.part.part.cmp(&b.part.part));

            let set_plat = set_prices.get(&prime).copied().flatten();
            let priced: Vec<u32> =
                gaps.iter().filter_map(|g| g.relics.first().and_then(|r| r.plat)).collect();
            let cost_to_fill = (!priced.is_empty()).then(|| priced.iter().sum());

            BomPlan { prime, gaps, covered, set_plat, cost_to_fill }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relics::{RelicIndex, RelicInfo, RelicReward};

    fn relic(display: &str, rewards: &[&str]) -> RelicInfo {
        let (tier, code) = display.split_once(' ').unwrap();
        RelicInfo {
            tier: tier.to_string(),
            code: code.to_string(),
            display: display.to_string(),
            rewards: rewards
                .iter()
                .map(|name| RelicReward {
                    item_name: name.to_string(),
                    rarity: "Uncommon".to_string(),
                    intact_chance: 0.0,
                    radiant_chance: 0.0,
                })
                .collect(),
        }
    }

    #[test]
    fn unmastered_primes_excludes_mastered_and_sorts_alphabetically() {
        let quantities = PartQuantities::from_entries_for_test(vec![
            ("Volt Prime".to_string(), "Blueprint".to_string(), 1),
            ("Ember Prime".to_string(), "Blueprint".to_string(), 1),
        ]);
        let mastery =
            MasterySet::from_xp([("/Lotus/Powersuits/Ember/EmberPrime".to_string(), 9_000_000)]);
        assert_eq!(unmastered_primes(&quantities, &mastery), vec!["Volt Prime".to_string()]);
    }

    #[test]
    fn buy_or_farm_plan_marks_a_part_covered_by_seen_only_relic_as_not_a_gap() {
        let quantities = PartQuantities::from_entries_for_test(vec![(
            "Afentis Prime".to_string(),
            "Blueprint".to_string(),
            1,
        )]);
        let idx = RelicIndex::new(vec![relic("Axi A22", &["Afentis Prime Blueprint"])]);
        let owned = HashMap::from([("Axi A22".to_string(), RelicEvidence::SeenOnly)]);

        let plans = buy_or_farm_plan(
            &owned,
            &HashMap::new(),
            &HashMap::new(),
            &idx,
            &MasterySet::default(),
            &quantities,
            &crate::OwnedPrimeParts::new(),
        );

        let afentis = plans.iter().find(|p| p.prime == "Afentis Prime").unwrap();
        assert_eq!(afentis.covered, 1);
        assert!(afentis.gaps.is_empty());
    }

    #[test]
    fn buy_or_farm_plan_treats_unscanned_owned_as_a_gap_absent_relic_evidence() {
        let quantities = PartQuantities::from_entries_for_test(vec![(
            "Kompressa Prime".to_string(),
            "Barrel".to_string(),
            1,
        )]);
        let idx = RelicIndex::new(Vec::new()); // no relic evidence at all
        let owned = HashMap::new();

        let plans = buy_or_farm_plan(
            &owned,
            &HashMap::new(),
            &HashMap::new(),
            &idx,
            &MasterySet::default(),
            &quantities,
            &crate::OwnedPrimeParts::new(),
        );

        let kompressa = plans.iter().find(|p| p.prime == "Kompressa Prime").unwrap();
        assert_eq!(kompressa.gaps.len(), 1);
        assert_eq!(kompressa.gaps[0].owned, None);
    }

    #[test]
    fn buy_or_farm_plan_marks_a_part_covered_when_owned_meets_build_quantity_absent_relic_evidence() {
        // No relic evidence at all, but the Inventory/Sell scan shows enough
        // already built — not a gap. (build_quantity is always known here,
        // since every part in this enumeration comes from PartQuantities
        // itself — the "unknown quantity" fallback described in the plan is
        // exercised instead by mastery_plan's own quantities.get() lookup,
        // see mastery_plan_carries_build_quantity_when_known_and_none_when_unknown.)
        let quantities = PartQuantities::from_entries_for_test(vec![(
            "Afuris Prime".to_string(),
            "Barrel".to_string(),
            1,
        )]);
        let idx = RelicIndex::new(Vec::new());
        let mut owned_parts = crate::OwnedPrimeParts::new();
        crate::owned_parts::apply_count(
            &mut owned_parts,
            &PrimePart { prime: "Afuris Prime".to_string(), part: "Barrel".to_string() },
            1,
        );

        let plans = buy_or_farm_plan(
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &idx,
            &MasterySet::default(),
            &quantities,
            &owned_parts,
        );

        let afuris = plans.iter().find(|p| p.prime == "Afuris Prime").unwrap();
        assert_eq!(afuris.covered, 1);
        assert!(afuris.gaps.is_empty());
    }

    #[test]
    fn buy_or_farm_plan_sums_cost_to_fill_from_cheapest_relic_per_gap_and_ignores_unpriced_gaps() {
        let quantities = PartQuantities::from_entries_for_test(vec![
            ("Rubico Prime".to_string(), "Barrel".to_string(), 1),
            ("Rubico Prime".to_string(), "Stock".to_string(), 1),
        ]);
        let idx = RelicIndex::new(vec![
            relic("Axi H3", &["Rubico Prime Barrel"]),
            relic("Lith V1", &["Rubico Prime Stock"]),
        ]);
        // No owned evidence for either relic — both parts are real gaps,
        // sourced from the full catalogue (not what's owned).
        let prices = HashMap::from([("axi_h3_relic".to_string(), Some(50))]);
        // Lith V1 (Stock's relic) deliberately unpriced.

        let plans = buy_or_farm_plan(
            &HashMap::new(),
            &prices,
            &HashMap::new(),
            &idx,
            &MasterySet::default(),
            &quantities,
            &crate::OwnedPrimeParts::new(),
        );

        let rubico = plans.iter().find(|p| p.prime == "Rubico Prime").unwrap();
        assert_eq!(rubico.gaps.len(), 2);
        assert_eq!(rubico.cost_to_fill, Some(50));
        let barrel = rubico.gaps.iter().find(|g| g.part.part == "Barrel").unwrap();
        assert_eq!(barrel.relics[0].relic_display, "Axi H3");
        assert_eq!(barrel.relics[0].plat, Some(50));
    }
}
