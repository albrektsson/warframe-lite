//! The hand-curated equipment wishlist: Prime Parts the player has flagged as
//! wanted, independent of mastery status. Unlike every other persisted set in
//! this crate, there's no scan or API source behind it — the player's stated
//! intent *is* the data (see ADR-0004, CONTEXT.md's "Wishlisted part").

use std::collections::HashSet;

use crate::mastery::PrimePart;

/// The persisted wishlist: a flat set of Prime Part display names (see
/// [`key`]), serialised to `wishlist.json` (see [`WISHLIST_FILE`]).
pub type Wishlist = HashSet<String>;

/// The file `wishlist.json` is cached under, via `wf_cache::load_blob`/
/// `save_blob` — same pattern as `owned-relics.json`. Unlike that file,
/// `wf-browse` may write this one directly (a narrow, documented exception
/// to ADR-0003; see ADR-0004): a hand-curated wishlist has no scan-derived
/// ground truth to diverge from.
pub const WISHLIST_FILE: &str = "wishlist.json";

/// A Prime Part's wishlist key, e.g. `PrimePart { prime: "Ember Prime",
/// part: "Systems" }` → `"Ember Prime Systems"`. The same key is used both to
/// mark/unmark a part on `wf-browse`'s Mastery tab and to test membership
/// against a reward row's matched name (via [`crate::mastery::prime_part`]).
pub fn key(part: &PrimePart) -> String {
    format!("{} {}", part.prime, part.part)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_combines_prime_and_part() {
        let part = PrimePart { prime: "Ember Prime".to_string(), part: "Systems".to_string() };
        assert_eq!(key(&part), "Ember Prime Systems");
    }

    #[test]
    fn wishlist_roundtrips_through_json() {
        let mut wishlist = Wishlist::new();
        wishlist.insert("Ember Prime Systems".to_string());
        let json = serde_json::to_string(&wishlist).unwrap();
        let back: Wishlist = serde_json::from_str(&json).unwrap();
        assert!(back.contains("Ember Prime Systems"));
        assert!(!back.contains("Ember Prime Chassis"));
    }
}
