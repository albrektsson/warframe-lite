//! Mastery tracking via Digital Extremes' **public** profile API.
//!
//! `getProfileViewingData.php?playerId=<accountId>` returns public profile data
//! (the same the game shows when you inspect a player) — no authentication. Its
//! `LoadOutInventory.XPInfo` lists every item the player has earned affinity on,
//! by internal path and lifetime affinity. An item is **mastered** once its
//! lifetime affinity reaches the rank-30 cap (which never resets, even on Forma),
//! so `affinity >= cap` is a stable mastery test.
//!
//! Caps derive from the standard formula (cumulative affinity to rank 30):
//! weapons `1000·r²/2 = 450,000`; Warframes/companions/archwing `2×` that
//! `= 900,000`. Verified against a real high-MR profile.
//!
//! Sends [`wf_data::DE_USER_AGENT`] rather than this app's own honest
//! `warframe-lite/<version>` UA (see [`wf_data::http_client`]'s default).
//! Issue #100 (2026-08-16) originally switched this *to* the honest UA,
//! after finding a spoofed `Mozilla/5.0` and the honest UA get
//! byte-identical `409 Conflict` responses against a bogus `playerId` — but
//! that only checked this one endpoint's immediate response, not whatever
//! DE's edge does with the UA afterward. Reversed back to a spoofed UA once
//! we found two community tools that have called this same DE domain for
//! years without issue — WFHelper's `apiHelperRunner.ts` and the
//! `calamity-inc/Soup` library behind `Sainan/warframe-api-helper` — both
//! independently choosing a browser-style UA specifically for DE's
//! endpoints (never their own honest name there), the opposite of what
//! issue #100 concluded. See [`wf_data::DE_USER_AGENT`]'s docs.

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::part_quantities::PartQuantities;

/// PC profile endpoint (public, no auth).
const PC_ENDPOINT: &str = "https://api.warframe.com/cdn/getProfileViewingData.php";

const WEAPON_CAP: u64 = 450_000;
const FRAME_CAP: u64 = 900_000;

/// The set of items a player has mastered: each entry is a **flattened**
/// internal path leaf (lowercased, alphanumeric-only, original character order
/// preserved — no word splitting) for a `Prime` item whose lifetime affinity has
/// crossed its rank-30 cap, with any known development codename translated to
/// its display name (see [`CODENAME_TO_DISPLAY`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MasterySet {
    mastered: Vec<String>,
}

impl MasterySet {
    pub fn len(&self) -> usize {
        self.mastered.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mastered.is_empty()
    }

    /// Build a set from `(item_path, lifetime_affinity)` pairs. Only `Prime`
    /// paths are kept — relic rewards are always primes, and keeping the
    /// mastery test scoped to primes avoids a mastered *vanilla* item (e.g.
    /// plain Dethcube) being mistaken for its Prime counterpart.
    pub fn from_xp(entries: impl IntoIterator<Item = (String, u64)>) -> Self {
        let mut mastered = Vec::new();
        for (path, xp) in entries {
            if xp >= cap_for(&path) && path.to_ascii_lowercase().contains("prime") {
                mastered.push(translate_codename(&flatten(leaf_of(&path))));
            }
        }
        Self { mastered }
    }

    /// Whether the built item a reward part belongs to has been mastered.
    ///
    /// Internal path naming doesn't reliably line up with display names via any
    /// single string transform: word order varies (`PrimeGram` vs `Gram
    /// Prime`), some leaves have an extra suffix (`RubicoPrimeWeapon`), some
    /// split a display word differently (`PrimeAkBoltoWeapon` vs `Akbolto`), and
    /// some are outright unrelated development **codenames** (`IronFrame` for
    /// Hildryn, `Infestation` for Nidus, `PolearmWeapon` for Orthos — see
    /// [`CODENAME_TO_DISPLAY`]). So matching is substring containment of the
    /// reward's distinguishing word(s) — with codenames translated first —
    /// inside the flattened mastered leaf, rather than an exact-key lookup.
    pub fn is_mastered(&self, reward_item_name: &str) -> bool {
        let core = reward_core(reward_item_name);
        !core.is_empty() && self.mastered.iter().any(|m| m.contains(&core))
    }

    /// Whether the item at internal-path `item_type` — as `Suits`/`LongGuns`/
    /// etc. equipment-array entries and `XPInfo` entries alike name it, e.g.
    /// `/Lotus/Powersuits/Excalibur/ExcaliburPrimeSuit` — has been mastered.
    /// Unlike [`Self::is_mastered`] (which matches a relic-reward *display*
    /// name via substring containment, since reward text and internal paths
    /// don't line up word-for-word), this applies the exact same leaf+flatten+
    /// codename-translate pipeline [`Self::from_xp`] built `mastered` from, so
    /// it compares a raw owned-equipment path by equality — no containment
    /// heuristic needed, since both sides go through the same transform of
    /// the same underlying DE internal name.
    pub fn is_mastered_by_path(&self, item_type: &str) -> bool {
        let key = translate_codename(&flatten(leaf_of(item_type)));
        !key.is_empty() && self.mastered.contains(&key)
    }
}

/// Fetch and build a [`MasterySet`] for `account_id` (24-hex) on PC.
pub async fn fetch(client: &reqwest::Client, account_id: &str) -> anyhow::Result<MasterySet> {
    let url = format!("{PC_ENDPOINT}?playerId={account_id}");
    tracing::debug!("GET {url}");
    let body = client
        .get(&url)
        .header("User-Agent", wf_data::DE_USER_AGENT)
        .send()
        .await?
        .error_for_status()
        .context("profile request failed (is the account id correct?)")?
        .json::<ProfileResponse>()
        .await
        .context("parsing profile JSON")?;

    let result = body
        .results
        .into_iter()
        .next()
        .context("profile response had no Results")?;
    let set = MasterySet::from_xp(
        result
            .loadout
            .xp_info
            .into_iter()
            .map(|e| (e.item_type, e.xp)),
    );
    tracing::info!("mastery: {} mastered items for {account_id}", set.len());
    Ok(set)
}

/// Fetch just the public `DisplayName` for `account_id`, used to verify that a
/// candidate account id (e.g. scraped from `EE.log`) actually belongs to the
/// local player. Returns `None` if the profile has no name / does not exist.
pub async fn fetch_display_name(
    client: &reqwest::Client,
    account_id: &str,
) -> anyhow::Result<Option<String>> {
    let url = format!("{PC_ENDPOINT}?playerId={account_id}");
    let body = client
        .get(&url)
        .header("User-Agent", wf_data::DE_USER_AGENT)
        .send()
        .await?
        .error_for_status()
        .context("profile request failed")?
        .json::<ProfileResponse>()
        .await
        .context("parsing profile JSON")?;
    let name = body
        .results
        .into_iter()
        .next()
        .map(|r| r.display_name)
        .filter(|n| !n.is_empty());
    Ok(name)
}

/// Load the mastered set from a disk cache when fresh (younger than `ttl`),
/// otherwise refetch. Falls back to a stale cache on network failure, and to an
/// empty set if there is nothing cached and the fetch fails.
pub async fn load_cached(
    client: &reqwest::Client,
    account_id: &str,
    ttl: std::time::Duration,
) -> MasterySet {
    // `-v3`: the on-disk set stores normalized keys, and the normalization
    // (see `canonical`) changed — bump the name so old caches are ignored.
    // `-v4`: matching moved from a canonical-key set to substring containment
    // with codename translation — bump so old-format cached keys aren't reused.
    // `-v5`: `cap_for` no longer gives every sentinel *weapon* the 900k frame
    // cap meant only for the sentinel's own body — bump so a cache built
    // under the old bug doesn't keep hiding a mastered sentinel weapon
    // between 450k-900k XP (issue #65).
    let file = format!("mastery-v5-{account_id}.json");
    if let Some(cached) = wf_cache::load_blob::<MasterySet>(&file) {
        if cached.age() < ttl {
            tracing::info!("mastery from cache ({} items)", cached.value.len());
            return cached.value;
        }
        match fetch(client, account_id).await {
            Ok(set) => {
                let _ = wf_cache::save_blob(&file, &set);
                return set;
            }
            Err(e) => {
                tracing::warn!("mastery refresh failed ({e:#}); using stale cache");
                return cached.value;
            }
        }
    }
    match fetch(client, account_id).await {
        Ok(set) => {
            let _ = wf_cache::save_blob(&file, &set);
            set
        }
        Err(e) => {
            tracing::warn!("mastery fetch failed: {e:#}");
            MasterySet::default()
        }
    }
}

#[derive(Deserialize)]
struct ProfileResponse {
    #[serde(rename = "Results", default)]
    results: Vec<ProfileResult>,
}

#[derive(Deserialize)]
struct ProfileResult {
    #[serde(rename = "LoadOutInventory", default)]
    loadout: LoadOut,
    #[serde(rename = "DisplayName", default)]
    display_name: String,
}

#[derive(Deserialize, Default)]
struct LoadOut {
    #[serde(rename = "XPInfo", default)]
    xp_info: Vec<XpEntry>,
}

#[derive(Deserialize)]
struct XpEntry {
    #[serde(rename = "ItemType")]
    item_type: String,
    #[serde(rename = "XP", default)]
    xp: u64,
}

/// Rank-30 affinity cap for an item, chosen by its category path. Sentinel
/// **bodies** get the frame cap (`/SentinelPowersuits/`, e.g.
/// `.../Sentinels/SentinelPowersuits/PrimeWyrmPowerSuit`) same as a Warframe;
/// sentinel **weapons** (`.../Sentinels/SentinelWeapons/PrimeLaserRifle`) are
/// still weapons and must fall through to the weapon cap — matching on the
/// broader `/sentinels/` parent segment used to catch both and wrongly gave
/// every sentinel weapon a 900k cap instead of 450k, live-verified as a real
/// false "not mastered" for a weapon already at 453,843 XP (issue #65).
fn cap_for(path: &str) -> u64 {
    let p = path.to_ascii_lowercase();
    if p.contains("/powersuits/")
        || p.contains("/sentinelpowersuits/")
        || p.contains("kubrow")
        || p.contains("catbrow")
        || p.contains("necromech")
    {
        FRAME_CAP
    } else {
        WEAPON_CAP
    }
}

/// The leaf of an internal path: `/Lotus/Powersuits/Ember/EmberPrime` → `EmberPrime`.
fn leaf_of(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// Lowercase and strip to ASCII alphanumerics, preserving character order (no
/// word splitting) — so `"PrimeDethCubePowerSuit"` → `"primedethcubepowersuit"`.
fn flatten(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// Known **development codenames** that share no substring with the shipped
/// display name, so no string transform of the internal path can derive the
/// display name — these need an explicit translation. Each pattern is the
/// flattened (see [`flatten`]) internal leaf with any leading/trailing "prime"
/// trimmed; each replacement is the flattened display base name. Extracted from
/// `warframe-public-export-plus` (icon filenames encode the true display name
/// even where the path/lang-key doesn't) and cross-checked against every real
/// relic reward name in WFCD `warframe-drop-data` to confirm each pair actually
/// corresponds to a droppable prime — entries that couldn't be confirmed this
/// way were dropped rather than risk a wrong translation.
const CODENAME_TO_DISPLAY: &[(&str, &str)] = &[
    ("akimboshotgun", "akbronco"),
    ("dualmagnus", "akmagnus"),
    ("akprimevastopistol", "akvasto"),
    ("cobraandcraneweapon", "cobracrane"),
    ("cronuslongsword", "dakra"),
    ("zorenaxeweapon", "dualzoren"),
    ("allnew1hsg", "euphona"),
    ("ironframe", "hildryn"),
    ("krisdagger", "karyst"),
    ("tonfacontestwinnerprimeweapon", "kronen"),
    ("infestation", "nidus"),
    ("nikondi", "ninkondi"),
    ("paladin", "oberon"),
    ("jetpack", "odonata"),
    ("polearmweapon", "orthos"),
    ("huntingbow", "paris"),
    ("vorunaaxeweapon", "sarofang"),
    ("lidagger", "spira"),
    ("lightninggun", "vadarya"),
    ("trapper", "vauban"),
    ("ventoscythe", "venato"),
];

/// Replace a known codename substring (see [`CODENAME_TO_DISPLAY`]) in a
/// flattened internal leaf with its display-name equivalent, if present.
fn translate_codename(flat: &str) -> String {
    for (codename, display) in CODENAME_TO_DISPLAY {
        if flat.contains(codename) {
            return flat.replace(codename, display);
        }
    }
    flat.to_string()
}

/// The distinguishing word(s) of a reward display name for mastery matching:
/// lowercase, alphanumeric-only, in original order, with `"prime"` and
/// component/suffix words dropped — e.g. `"Dethcube Prime Carapace"` →
/// `"dethcube"`, `"Cobra & Crane Prime"` → `"cobracrane"`.
fn reward_core(reward_item_name: &str) -> String {
    reward_item_name
        .split_whitespace()
        .filter(|w| {
            let lw = w.to_ascii_lowercase();
            lw != "prime" && !COMPONENTS.contains(&lw.as_str())
        })
        .map(flatten)
        .collect()
}

/// Component words stripped from a reward part name to get the built item's
/// base name.
// Sourced (and cross-checked against every "<Weapon> Prime <suffix...>" reward
// name in the WFCD drop tables) so `built_name`/`canonical` collapse every
// prime's parts to one group — otherwise a part whose suffix isn't listed here
// creates a spurious separate "prime" (e.g. "Ninkondi Prime Chain" used to stay
// distinct from "Ninkondi Prime").
const COMPONENTS: &[&str] = &[
    "blueprint", "systems", "chassis", "neuroptics", "barrel", "receiver", "stock", "link",
    "blade", "blades", "handle", "hilt", "guard", "grip", "head", "string", "limb", "lower",
    "upper", "ornament", "boot", "gauntlet", "carapace", "cerebrum", "wings", "harness", "pouch",
    "star", "stars", "disc", "band", "buckle", "clamp", "collar", "chain", "kubrow",
];

/// The built prime's **display** name for a reward part, e.g.
/// `"Ember Prime Systems Blueprint"` → `"Ember Prime"`. Used to dedup and label
/// a relic's rewards by the item they build into.
pub fn built_name(reward: &str) -> String {
    reward
        .split_whitespace()
        .filter(|w| !COMPONENTS.contains(&w.to_ascii_lowercase().as_str()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// One craftable component of a Built prime (CONTEXT.md's "Prime Part"): its
/// own Blueprint, or a piece like Chassis/Systems/Neuroptics (or a weapon's
/// equivalent). One level below `built_name`'s Built prime.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrimePart {
    /// Built prime display name, e.g. "Ember Prime".
    pub prime: String,
    /// Component label, e.g. "Systems", "Blueprint".
    pub part: String,
}

/// The distinguishing component word of a reward's name, e.g. `"Ember
/// Prime Systems Blueprint"` → `"Systems"`. Falls back to `"Blueprint"` when
/// "Blueprint" is the only component word present (a frame/weapon's own build
/// blueprint, as opposed to one of its components) or when the reward has no
/// component word at all (a bare `"<Weapon> Prime"` reward names that
/// weapon's own — implicit — blueprint).
pub fn part_name(reward: &str) -> String {
    reward
        .split_whitespace()
        .find(|w| {
            let lw = w.to_ascii_lowercase();
            lw != "blueprint" && COMPONENTS.contains(&lw.as_str())
        })
        .map(|w| w.to_string())
        .unwrap_or_else(|| "Blueprint".to_string())
}

/// A reward's [`PrimePart`] identity: which prime it builds, and which
/// component of it.
pub fn prime_part(reward: &str) -> PrimePart {
    PrimePart { prime: built_name(reward), part: part_name(reward) }
}

/// Resolve an Inventory/Sell screen card's raw OCR'd label to its
/// [`PrimePart`] identity, e.g. `"Ember Prime Systems"` →
/// `PrimePart { prime: "Ember Prime", part: "Systems" }`.
///
/// Uses the same [`prime_part`] split relic reward strings already go
/// through, but validates the result's `prime` against `quantities`'s
/// WFCD-derived keyspace ([`PartQuantities::has_prime`]) as the authoritative
/// check — this *replaces* a raw `"Prime"` substring heuristic rather than
/// layering on top of it, since Prime Part names are far more distinctive
/// than relic codes and need no fuzzy matching (unlike
/// [`crate::RelicIndex::best_match`]). A label whose prime isn't in the
/// catalogue is dropped (`None`), not counted (see issue #37's
/// catalog-matching decision, mirroring [`PartQuantities::get`]'s existing
/// "unknown key → unknown, never guessed" behavior).
pub fn inventory_prime_part(label: &str, quantities: &PartQuantities) -> Option<PrimePart> {
    let pp = prime_part(label);
    quantities.has_prime(&pp.prime).then_some(pp)
}

/// `wf-mem`'s raw owned-part internal-name prefixes (issue #81) —
/// deliberately duplicated from `wf_mem::owned_parts`'s private constants of
/// the same names rather than shared: `wf-mem` depends on `wf-relic` (for
/// `write_owned_parts`'s catalogue cross-reference), so the dependency can't
/// run the other way, and these two short literals are cheap to keep in
/// sync compared to a shared micro-crate.
const WARFRAME_PART_PREFIX: &str = "/Lotus/Types/Recipes/WarframeRecipes/";
const WEAPON_PART_PREFIX: &str = "/Lotus/Types/Recipes/Weapons/WeaponParts/";

/// Resolve one `wf-mem`-sourced raw owned-part entry's internal `item_type`
/// (`wf_mem::owned_parts::OwnedPartRaw`, issue #80) to its [`PrimePart`]
/// identity, if it names a Prime component `quantities`'s WFCD-derived
/// keyspace recognizes.
///
/// Mirrors [`inventory_prime_part`]'s OCR-label decode, but starting from
/// wf-mem's raw internal path instead of an OCR'd screen label: strips the
/// `WarframeRecipes/.../Component` or `Weapons/WeaponParts/...` prefix (and
/// `Component` suffix), camelCase-splits the remaining leaf into words
/// (`"VorunaPrimeSystems"` -> `"Voruna Prime Systems"`), and applies DE's
/// internal `Helmet` -> display `Neuroptics` rename before validating
/// against `quantities` the same way `inventory_prime_part` does. A
/// non-Prime frame/weapon part (`AshChassisComponent`,
/// `GorgonWraithBarrel`) or an entry `quantities` doesn't recognize is
/// dropped (`None`) rather than surfaced under a wrong or guessed identity
/// — same "unknown, never guessed" precedent as `inventory_prime_part`
/// (ADR-0011). This is also how non-Prime parts get filtered out of
/// `owned-prime-parts.json`: they're never looked up by any `PrimePart` key
/// in the first place, so no explicit "is this a Prime" gate exists
/// upstream of this function.
pub fn owned_part_from_item_type(item_type: &str, quantities: &PartQuantities) -> Option<PrimePart> {
    let label = item_type_to_label(item_type)?;
    inventory_prime_part(&label, quantities)
}

/// The `{FrameName}{Part}` or `{WeaponName}{Part}` leaf of a raw owned-part
/// `item_type`, camelCase-split into a reward-shaped label and with DE's
/// `Helmet` internal label renamed to its display equivalent, `Neuroptics`.
/// `None` when `item_type` matches neither of `wf_mem::owned_parts`'s two
/// naming patterns, or the leaf left after stripping is empty.
fn item_type_to_label(item_type: &str) -> Option<String> {
    let leaf = if let Some(rest) = item_type.strip_prefix(WARFRAME_PART_PREFIX) {
        rest.strip_suffix("Component").filter(|mid| !mid.is_empty())?
    } else {
        item_type.strip_prefix(WEAPON_PART_PREFIX).filter(|rest| !rest.is_empty())?
    };
    let spaced = camel_case_split(leaf);
    Some(
        spaced
            .split_whitespace()
            .map(|w| if w == "Helmet" { "Neuroptics" } else { w })
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// Insert a space before each uppercase letter that follows a lowercase
/// letter or digit, e.g. `"VorunaPrimeSystems"` -> `"Voruna Prime Systems"`.
/// Mirrors `wf-lite`'s own `readable_item_name` camelCase-splitting
/// convention (`src/main.rs`), duplicated here rather than shared for the
/// same reason as [`WARFRAME_PART_PREFIX`] above — `wf-lite`'s binary can't
/// be a library dependency of `wf-relic`.
fn camel_case_split(leaf: &str) -> String {
    let mut out = String::new();
    let mut prev: Option<char> = None;
    for c in leaf.chars() {
        if c.is_uppercase() && prev.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit()) {
            out.push(' ');
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_by_category() {
        assert_eq!(cap_for("/Lotus/Powersuits/Ember/EmberPrime"), FRAME_CAP);
        assert_eq!(cap_for("/Lotus/Weapons/Tenno/LongGuns/BratonPrime"), WEAPON_CAP);
    }

    #[test]
    fn caps_a_sentinel_body_at_the_frame_cap_but_its_weapon_at_the_weapon_cap() {
        // Both nest under the same `/Sentinels/` parent — only the powersuit
        // (the pet body) gets the frame cap; its weapon is still a weapon.
        assert_eq!(
            cap_for("/Lotus/Types/Sentinels/SentinelPowersuits/PrimeWyrmPowerSuit"),
            FRAME_CAP
        );
        assert_eq!(
            cap_for("/Lotus/Types/Sentinels/SentinelWeapons/PrimeLaserRifle"),
            WEAPON_CAP
        );
    }

    #[test]
    fn reward_core_strips_prime_and_components_in_order() {
        assert_eq!(reward_core("Gram Prime Blueprint"), "gram");
        assert_eq!(reward_core("Ember Prime Systems Blueprint"), "ember");
        assert_eq!(reward_core("Cobra & Crane Prime"), "cobracrane");
        assert_ne!(reward_core("Gram Prime"), reward_core("Rubico Prime"));
    }

    #[test]
    fn part_name_picks_the_specific_component_over_blueprint() {
        assert_eq!(part_name("Ember Prime Systems Blueprint"), "Systems");
        assert_eq!(part_name("Akstiletto Prime Barrel"), "Barrel");
    }

    #[test]
    fn part_name_falls_back_to_blueprint() {
        // The frame/weapon's own build blueprint: "Blueprint" is the only
        // component word present.
        assert_eq!(part_name("Ember Prime Blueprint"), "Blueprint");
        // No component word at all: WFCD's raw reward text for some weapons'
        // own blueprint omits the word "Blueprint" entirely.
        assert_eq!(part_name("Cobra & Crane Prime"), "Blueprint");
    }

    #[test]
    fn prime_part_pairs_built_name_and_part_name() {
        assert_eq!(
            prime_part("Ember Prime Systems Blueprint"),
            PrimePart { prime: "Ember Prime".to_string(), part: "Systems".to_string() }
        );
        assert_eq!(
            prime_part("Ember Prime Blueprint"),
            PrimePart { prime: "Ember Prime".to_string(), part: "Blueprint".to_string() }
        );
    }

    #[test]
    fn inventory_prime_part_accepts_a_known_prime_and_drops_an_unknown_one() {
        let quantities = PartQuantities::from_entries_for_test(vec![(
            "Ember Prime".to_string(),
            "Systems".to_string(),
            1,
        )]);
        assert_eq!(
            inventory_prime_part("Ember Prime Systems", &quantities),
            Some(PrimePart { prime: "Ember Prime".to_string(), part: "Systems".to_string() })
        );
        // Not a known Prime in the catalogue — dropped, not guessed.
        assert_eq!(inventory_prime_part("Volnus Prime Blueprint", &quantities), None);
    }

    #[test]
    fn inventory_prime_part_checks_the_prime_not_the_exact_part_pair() {
        // Afuris Prime is known, but only its Barrel quantity is in the
        // catalogue below — Link should still resolve, since only `prime`
        // (not the exact (prime, part) pair) is the authoritative check.
        let quantities = PartQuantities::from_entries_for_test(vec![(
            "Afuris Prime".to_string(),
            "Barrel".to_string(),
            2,
        )]);
        assert_eq!(
            inventory_prime_part("Afuris Prime Link", &quantities),
            Some(PrimePart { prime: "Afuris Prime".to_string(), part: "Link".to_string() })
        );
    }

    #[test]
    fn owned_part_from_item_type_decodes_a_warframe_component_and_renames_helmet() {
        let quantities = PartQuantities::from_entries_for_test(vec![
            ("Voruna Prime".to_string(), "Systems".to_string(), 1),
            ("Voruna Prime".to_string(), "Neuroptics".to_string(), 1),
        ]);
        assert_eq!(
            owned_part_from_item_type(
                "/Lotus/Types/Recipes/WarframeRecipes/VorunaPrimeSystemsComponent",
                &quantities
            ),
            Some(PrimePart { prime: "Voruna Prime".to_string(), part: "Systems".to_string() })
        );
        // DE's internal label is Helmet; the display/catalogue label is Neuroptics.
        assert_eq!(
            owned_part_from_item_type(
                "/Lotus/Types/Recipes/WarframeRecipes/VorunaPrimeHelmetComponent",
                &quantities
            ),
            Some(PrimePart { prime: "Voruna Prime".to_string(), part: "Neuroptics".to_string() })
        );
    }

    #[test]
    fn owned_part_from_item_type_drops_a_non_prime_warframe_component() {
        let quantities = PartQuantities::from_entries_for_test(vec![(
            "Voruna Prime".to_string(),
            "Systems".to_string(),
            1,
        )]);
        // Ash is not a Prime — not in the catalogue, so dropped, not guessed.
        assert_eq!(
            owned_part_from_item_type(
                "/Lotus/Types/Recipes/WarframeRecipes/AshChassisComponent",
                &quantities
            ),
            None
        );
    }

    #[test]
    fn owned_part_from_item_type_decodes_a_weapon_part_with_no_component_suffix() {
        let quantities = PartQuantities::from_entries_for_test(vec![(
            "Rubico Prime".to_string(),
            "Receiver".to_string(),
            1,
        )]);
        assert_eq!(
            owned_part_from_item_type(
                "/Lotus/Types/Recipes/Weapons/WeaponParts/RubicoPrimeReceiver",
                &quantities
            ),
            Some(PrimePart { prime: "Rubico Prime".to_string(), part: "Receiver".to_string() })
        );
    }

    #[test]
    fn owned_part_from_item_type_drops_a_non_prime_weapon_part() {
        let quantities = PartQuantities::from_entries_for_test(vec![(
            "Rubico Prime".to_string(),
            "Receiver".to_string(),
            1,
        )]);
        // Gorgon Wraith is not a Prime.
        assert_eq!(
            owned_part_from_item_type(
                "/Lotus/Types/Recipes/Weapons/WeaponParts/GorgonWraithBarrel",
                &quantities
            ),
            None
        );
    }

    #[test]
    fn owned_part_from_item_type_decodes_a_sentinel_part_under_the_weapon_prefix() {
        // Sentinels use `Systems` under the weapon-parts namespace too (#79),
        // distinct from the Warframe pattern's own `Systems`.
        let quantities = PartQuantities::from_entries_for_test(vec![(
            "Shade Prime".to_string(),
            "Systems".to_string(),
            1,
        )]);
        assert_eq!(
            owned_part_from_item_type(
                "/Lotus/Types/Recipes/Weapons/WeaponParts/ShadePrimeSystems",
                &quantities
            ),
            Some(PrimePart { prime: "Shade Prime".to_string(), part: "Systems".to_string() })
        );
    }

    #[test]
    fn owned_part_from_item_type_rejects_an_item_type_matching_neither_naming_pattern() {
        let quantities = PartQuantities::from_entries_for_test(vec![(
            "Voruna Prime".to_string(),
            "Systems".to_string(),
            1,
        )]);
        assert_eq!(
            owned_part_from_item_type("/Lotus/Types/Items/MiscItems/EnduringEndo", &quantities),
            None
        );
    }

    #[test]
    fn translate_codename_only_touches_known_patterns() {
        assert_eq!(translate_codename("primeironframeprime"), "primehildrynprime");
        assert_eq!(translate_codename("primeinfestationweapon"), "primenidusweapon");
        assert_eq!(translate_codename("primeemberprime"), "primeemberprime"); // untouched
    }

    #[test]
    fn mastery_matches_reversed_suffixed_split_and_codenamed_paths() {
        let set = MasterySet::from_xp([
            // Reversed word order.
            ("/Lotus/Weapons/Tenno/Melee/Swords/PrimeGram/PrimeGram".to_string(), 16_000_000),
            // Trailing -Weapon suffix.
            ("/Lotus/Weapons/Tenno/LongGuns/RubicoPrime/RubicoPrimeWeapon".to_string(), 3_000_000),
            // Internal camelCase splits the display word (Ak+Bolto).
            ("/Lotus/Weapons/Tenno/Pistols/PrimeAkbolto/PrimeAkBoltoWeapon".to_string(), 648_728),
            // Internal camelCase splits differently too (Deth+Cube).
            (
                "/Lotus/Types/Sentinels/SentinelPowersuits/PrimeDethCubePowerSuit".to_string(),
                138_609_285,
            ),
            // True development codenames, unrelated to the display name.
            ("/Lotus/Powersuits/IronFrame/IronFramePrime".to_string(), 11_649_465), // Hildryn
            ("/Lotus/Powersuits/Infestation/InfestationPrime".to_string(), 3_286_782), // Nidus
            (
                "/Lotus/Weapons/Tenno/Melee/Polearms/PrimePolearmWeapon".to_string(),
                9_000_000,
            ), // Orthos
            ("/Lotus/Powersuits/Ember/EmberPrime".to_string(), 9_000_000),
            ("/Lotus/Weapons/Tenno/LongGuns/BratonPrime".to_string(), 100_000), // below cap
            // A mastered *vanilla* (non-Prime) item must not count as its Prime:
            // vanilla Boltor mastered, but Boltor Prime is never listed here.
            ("/Lotus/Weapons/Tenno/LongGuns/Boltor/Boltor".to_string(), 500_000),
        ]);
        assert!(set.is_mastered("Gram Prime Blueprint"));
        assert!(set.is_mastered("Rubico Prime Blueprint"));
        assert!(set.is_mastered("Akbolto Prime Blueprint"));
        assert!(set.is_mastered("Dethcube Prime Carapace"));
        assert!(set.is_mastered("Hildryn Prime Neuroptics Blueprint"));
        assert!(set.is_mastered("Nidus Prime Blueprint"));
        assert!(set.is_mastered("Orthos Prime Blueprint"));
        assert!(set.is_mastered("Ember Prime Systems Blueprint"));
        assert!(!set.is_mastered("Braton Prime Receiver")); // below cap → not mastered
        assert!(!set.is_mastered("Volnus Prime Blueprint")); // absent
        assert!(!set.is_mastered("Boltor Prime Blueprint")); // vanilla-only mastery ≠ Prime
    }

    #[test]
    fn is_mastered_by_path_matches_the_exact_path_that_built_the_set() {
        let set = MasterySet::from_xp([(
            "/Lotus/Powersuits/Excalibur/ExcaliburPrimeSuit".to_string(),
            900_000,
        )]);
        assert!(set.is_mastered_by_path("/Lotus/Powersuits/Excalibur/ExcaliburPrimeSuit"));
        assert!(!set.is_mastered_by_path("/Lotus/Powersuits/Volt/VoltPrimeSuit"));
        // Unrelated to `from_xp`'s substring-containment counterpart —
        // exact-key matching, so a differently-shaped path for the same item
        // doesn't accidentally satisfy it just by sharing a substring.
        assert!(!set.is_mastered_by_path("/Lotus/Powersuits/Excalibur/Excalibur"));
    }

    #[test]
    fn is_mastered_by_path_translates_a_codename_path_like_from_xp_does() {
        // Hildryn's dev codename is "IronFrame" (see `CODENAME_TO_DISPLAY`) —
        // `from_xp` translates it when building `mastered`, so the query side
        // must apply the same translation or the two never agree.
        let set = MasterySet::from_xp([(
            "/Lotus/Powersuits/IronFrame/IronFramePrime".to_string(),
            900_000,
        )]);
        assert!(set.is_mastered_by_path("/Lotus/Powersuits/IronFrame/IronFramePrime"));
    }

    #[test]
    fn is_mastered_by_path_treats_a_sentinel_weapon_as_mastered_at_the_weapon_cap() {
        // Regression for the `cap_for` bug this ticket's live cross-reference
        // surfaced: a sentinel weapon at 453,843 XP (above the 450k weapon
        // cap, below the 900k frame cap it was wrongly given) read as
        // "not mastered" on a real account despite being mastered in-game.
        let set = MasterySet::from_xp([(
            "/Lotus/Types/Sentinels/SentinelWeapons/PrimeLaserRifle".to_string(),
            453_843,
        )]);
        assert!(set.is_mastered_by_path("/Lotus/Types/Sentinels/SentinelWeapons/PrimeLaserRifle"));
    }

    #[test]
    fn is_mastered_by_path_is_false_for_an_owned_but_below_cap_prime() {
        let set = MasterySet::from_xp([(
            "/Lotus/Weapons/Tenno/LongGuns/BratonPrime".to_string(),
            100_000, // below the weapon cap — not mastered
        )]);
        assert!(!set.is_mastered_by_path("/Lotus/Weapons/Tenno/LongGuns/BratonPrime"));
    }
}
