//! Per-Prime-Part market info (vaulted status + ducat value) for the
//! Equipment category-tree view.
//!
//! Both facts already live on the warframe.market item catalogue
//! ([`wf_data::items::Item`]) but nothing in `wf-browse` surfaced either
//! before this. [`PartQuantities`] only knows WFCD `warframe-items` component
//! labels (e.g. "Systems"), not the catalogue's full reward-name keying (e.g.
//! "Ember Prime Systems Blueprint"), so [`reward_label`] reconstructs that
//! label and [`resolve`] fuzzy-matches it against the catalogue via the same
//! [`ItemIndex::best_match`] resolution used elsewhere to turn OCR'd reward
//! text into a catalogue item — applied here to a programmatically
//! reconstructed label instead of a scanned one.

use std::collections::HashMap;

use crate::index::ItemIndex;
use crate::mastery::PrimePart;
use crate::part_quantities::PartQuantities;

/// A part's resolved market facts — both already fetched via the item
/// catalogue but never surfaced in `wf-browse` before the Equipment
/// category-tree view.
#[derive(Debug, Clone, Copy, Default)]
pub struct PartMarketInfo {
    pub vaulted: bool,
    pub ducats: Option<u32>,
}

/// The catalogue reward label a Prime Part builds, e.g. `PrimePart { prime:
/// "Ember Prime", part: "Systems" }` -> `"Ember Prime Systems Blueprint"`, or
/// `PrimePart { prime: "Ember Prime", part: "Blueprint" }` -> `"Ember Prime
/// Blueprint"` — the inverse of [`crate::mastery::part_name`].
pub fn reward_label(part: &PrimePart) -> String {
    if part.part.eq_ignore_ascii_case("blueprint") {
        format!("{} Blueprint", part.prime)
    } else {
        format!("{} {} Blueprint", part.prime, part.part)
    }
}

/// Resolve every part [`PartQuantities`] knows about against `items`, once at
/// launch — a per-frame fuzzy match would be far too slow for a tree redrawn
/// every frame. A part whose reconstructed label doesn't clear
/// [`ItemIndex::best_match`]'s similarity threshold is simply absent from the
/// result (no vaulted badge/ducat value shown), not guessed.
pub fn resolve(quantities: &PartQuantities, items: &ItemIndex) -> HashMap<PrimePart, PartMarketInfo> {
    let mut out = HashMap::new();
    for prime in quantities.primes() {
        for (part, _quantity) in quantities.parts_for(prime) {
            let pp = PrimePart { prime: prime.to_string(), part };
            if let Some(m) = items.best_match(&reward_label(&pp)) {
                out.insert(pp, PartMarketInfo { vaulted: m.item.vaulted, ducats: m.item.ducats });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::normalize;
    use wf_data::items::Item;

    fn item(name: &str, ducats: Option<u32>, vaulted: bool) -> Item {
        Item { slug: normalize(name), name: name.to_string(), ducats, tags: vec![], vaulted }
    }

    #[test]
    fn reward_label_handles_blueprint_and_named_parts() {
        assert_eq!(
            reward_label(&PrimePart { prime: "Ember Prime".to_string(), part: "Systems".to_string() }),
            "Ember Prime Systems Blueprint"
        );
        assert_eq!(
            reward_label(&PrimePart { prime: "Ember Prime".to_string(), part: "Blueprint".to_string() }),
            "Ember Prime Blueprint"
        );
    }

    #[test]
    fn resolve_finds_matches_and_skips_unmatched_parts() {
        let quantities = PartQuantities::from_entries_for_test(vec![
            ("Ember Prime".to_string(), "Systems".to_string(), 1),
            ("Ember Prime".to_string(), "Nonexistent".to_string(), 1),
        ]);
        let items = ItemIndex::new(vec![item("Ember Prime Systems Blueprint", Some(45), true)]);
        let info = resolve(&quantities, &items);

        let systems = PrimePart { prime: "Ember Prime".to_string(), part: "Systems".to_string() };
        let resolved = info.get(&systems).expect("Systems should have resolved");
        assert_eq!(resolved.ducats, Some(45));
        assert!(resolved.vaulted);

        let nonexistent = PrimePart { prime: "Ember Prime".to_string(), part: "Nonexistent".to_string() };
        assert!(!info.contains_key(&nonexistent));
    }
}
