//! Pure parsing of Riven details out of the raw `inventory.php` JSON body —
//! network/screen-agnostic, following the same convention as
//! [`crate::foundry::parse_foundry`]. Field names and shapes are per
//! `docs/research/mobile-inventory-api-coverage.md`'s coverage of
//! `Upgrades[]` (WFHelper's `rivenFingerprint.ts`, read from source): each
//! riven's `UpgradeFingerprint` is a JSON string — sometimes double-encoded —
//! carrying `compat` (weapon unique name), `pol` (polarity), `lvl`/`lvlReq`
//! (current rank / mastery requirement), `rerolls`, and `buffs`/`curses`
//! arrays of `{Tag, Value}` pairs, the same encoded roll values DE's own
//! client uses to render a riven's stat lines. This only extracts those
//! fields raw — it does not decode `Value` into a displayable stat (that
//! needs per-weapon disposition/riven-type tables this crate doesn't carry,
//! mirroring how `parse_foundry` leaves `item_type` as DE's raw path rather
//! than resolving a display name).
//!
//! `Upgrades[]` is not riven-only: every ordinary fused mod also carries a
//! minimal `UpgradeFingerprint` (just its fusion `lvl`, no `compat`, no
//! `challenge`) to track upgrade state — live-verified against a real
//! account, where these outnumbered actual rivens roughly 10 to 1.
//! [`parse_rivens`] tells a real riven (veiled *or* unveiled) apart from a
//! plain mod by `compat` or `challenge` being present: an **unveiled** riven
//! has a resolved `compat` and no `challenge`; a still-**veiled** riven has
//! `challenge` (its unveil requirement) and no `compat` yet. Both counted
//! toward the account's live Riven capacity reading during verification —
//! dropping veiled ones (as an earlier version of this parser did, matching
//! WFHelper's own `decodeSingleRiven`, which discards them because it can't
//! resolve a display name without one) undercounted by exactly the veiled
//! total.

use serde::Deserialize;
use wf_data::Polarity;

/// One `buffs`/`curses` entry: a stat tag and its raw encoded roll value
/// (not a percentage — see the module doc).
#[derive(Debug, Clone, PartialEq)]
pub struct RivenStat {
    pub tag: String,
    pub value: i64,
}

/// One real `Upgrades[]` riven, veiled or unveiled.
#[derive(Debug, Clone, PartialEq)]
pub struct Riven {
    /// DE's internal unique name for this riven mod, e.g.
    /// `/Lotus/Upgrades/Mods/Randomized/LotusRifleRandomModRare`.
    pub item_type: String,
    pub item_count: u32,
    /// The weapon this riven is attuned to (`compat`'s unique name), or
    /// `None` while still veiled — unidentified, no weapon resolved yet.
    pub weapon_unique_name: Option<String>,
    pub polarity: Option<Polarity>,
    pub mastery_req: Option<i64>,
    pub rank: Option<i64>,
    pub rerolls: Option<i64>,
    pub buffs: Vec<RivenStat>,
    pub curses: Vec<RivenStat>,
}

/// Parsed Riven state: every real riven found in `Upgrades[]`, veiled or
/// unveiled (plain fused mods are excluded — see the module doc).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RivenState {
    pub rivens: Vec<Riven>,
}

#[derive(Debug, Deserialize, Default)]
struct RawInventory {
    #[serde(default, rename = "Upgrades")]
    upgrades: Vec<RawEntry>,
}

#[derive(Debug, Deserialize, Default)]
struct RawEntry {
    #[serde(rename = "ItemType")]
    item_type: Option<String>,
    #[serde(rename = "ItemCount")]
    item_count: Option<i64>,
    #[serde(rename = "UpgradeFingerprint")]
    upgrade_fingerprint: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RawFingerprint {
    compat: Option<String>,
    pol: Option<String>,
    #[serde(rename = "lvlReq")]
    lvl_req: Option<i64>,
    lvl: Option<i64>,
    rerolls: Option<i64>,
    #[serde(default)]
    buffs: Vec<RawStat>,
    #[serde(default)]
    curses: Vec<RawStat>,
    /// Presence (any shape) marks a riven still awaiting its unveil
    /// challenge — only checked for presence, never inspected further.
    challenge: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Clone)]
struct RawStat {
    #[serde(rename = "Tag")]
    tag: String,
    #[serde(rename = "Value")]
    value: i64,
}

/// Parse Riven state out of a raw `inventory.php` JSON response body. An
/// entry only becomes a [`Riven`] when its `UpgradeFingerprint` parses and
/// carries `compat` and/or `challenge` — the signal that it's a real riven
/// (veiled or unveiled), not a plain fused mod's fingerprint (see the module
/// doc). Everything else (no fingerprint, unparseable fingerprint, a plain
/// mod's fingerprint) is skipped rather than erroring the whole parse — a
/// single bad or irrelevant entry shouldn't take down the rest of the
/// payload, mirroring `parse_foundry`'s "missing `ItemType` -> skip"
/// convention.
pub fn parse_rivens(raw_json: &str) -> anyhow::Result<RivenState> {
    let raw: RawInventory = serde_json::from_str(raw_json)?;

    let rivens = raw
        .upgrades
        .into_iter()
        .filter_map(|e| {
            let item_type = e.item_type?;
            let raw_fp = e.upgrade_fingerprint?;
            let fp = parse_fingerprint(&raw_fp)?;
            if fp.compat.is_none() && fp.challenge.is_none() {
                return None;
            }
            Some(Riven {
                item_type,
                item_count: item_count_or_default(e.item_count),
                weapon_unique_name: fp.compat,
                polarity: fp.pol.as_deref().map(Polarity::from_ap_code),
                mastery_req: fp.lvl_req,
                rank: fp.lvl,
                rerolls: fp.rerolls,
                buffs: fp.buffs.into_iter().map(Into::into).collect(),
                curses: fp.curses.into_iter().map(Into::into).collect(),
            })
        })
        .collect();

    Ok(RivenState { rivens })
}

fn item_count_or_default(count: Option<i64>) -> u32 {
    count.and_then(|c| u32::try_from(c).ok()).unwrap_or(1)
}

impl From<RawStat> for RivenStat {
    fn from(s: RawStat) -> Self {
        RivenStat { tag: s.tag, value: s.value }
    }
}

/// `UpgradeFingerprint` is a JSON string, occasionally double-stringified
/// (WFHelper's own `parseFingerprint` guards for exactly this) — parse once,
/// and if the result is itself a string, parse again.
fn parse_fingerprint(raw: &str) -> Option<RawFingerprint> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    match value {
        serde_json::Value::String(inner) => serde_json::from_str(&inner).ok(),
        other => serde_json::from_value(other).ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> String {
        fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/riven_inventory.json"
        ))
        .expect("fixture reads")
    }

    #[test]
    fn parses_the_unveiled_riven_from_the_fixture() {
        let state = parse_rivens(&fixture()).expect("parses");

        // 6 Upgrades[] entries in the fixture: 2 unveiled rivens, 1 veiled
        // riven, 1 plain mod, 1 plain-mod-shaped fingerprint (rerolls only,
        // no compat/challenge), 1 compat-but-nulled-challenge edge case.
        assert_eq!(state.rivens.len(), 3);

        let riven = &state.rivens[0];
        assert_eq!(riven.item_type, "/Lotus/Upgrades/Mods/Randomized/LotusRifleRandomModRare");
        assert_eq!(
            riven.weapon_unique_name.as_deref(),
            Some("/Lotus/Weapons/Tenno/Rifle/Soma/SomaPrimeRifle")
        );
        assert_eq!(riven.polarity, Some(Polarity::Madurai));
        assert_eq!(riven.mastery_req, Some(10));
        assert_eq!(riven.rank, Some(8));
        assert_eq!(riven.rerolls, Some(3));
        assert_eq!(
            riven.buffs,
            vec![
                RivenStat { tag: "WeaponCritChanceMod".to_string(), value: 823_451_120 },
                RivenStat { tag: "WeaponDamageAmountMod".to_string(), value: 512_009_887 },
            ]
        );
        assert_eq!(
            riven.curses,
            vec![RivenStat { tag: "WeaponRecoilReductionMod".to_string(), value: 190_442_017 }]
        );
    }

    #[test]
    fn parses_a_double_stringified_fingerprint_riven() {
        let riven = &parse_rivens(&fixture()).expect("parses").rivens[1];
        assert_eq!(
            riven.item_type,
            "/Lotus/Upgrades/Mods/Randomized/LotusModularMeleeRandomModRare"
        );
        assert_eq!(
            riven.weapon_unique_name.as_deref(),
            Some("/Lotus/Weapons/Tenno/Melee/Zaws/LongSword/ZawLongSword")
        );
        assert_eq!(riven.rerolls, Some(5));
        assert_eq!(riven.buffs.len(), 1);
    }

    #[test]
    fn parses_a_still_veiled_riven_with_no_compat_yet() {
        let riven = &parse_rivens(&fixture()).expect("parses").rivens[2];
        assert_eq!(
            riven.item_type,
            "/Lotus/Upgrades/Mods/Randomized/LotusPistolRandomModRare"
        );
        assert_eq!(riven.weapon_unique_name, None);
        assert!(riven.buffs.is_empty() && riven.curses.is_empty());
    }

    #[test]
    fn skips_an_upgrades_entry_with_no_fingerprint() {
        let json = r#"{"Upgrades":[{"ItemType":"/Lotus/Upgrades/Mods/SomePlainMod","ItemCount":1}]}"#;
        let state = parse_rivens(json).unwrap();
        assert!(state.rivens.is_empty());
    }

    #[test]
    fn skips_an_entry_missing_item_type() {
        let json = r#"{"Upgrades":[{"UpgradeFingerprint":"{\"compat\":\"x\"}"}]}"#;
        let state = parse_rivens(json).unwrap();
        assert!(state.rivens.is_empty());
    }

    #[test]
    fn skips_a_plain_mods_fingerprint_with_no_compat_or_challenge() {
        // The fixture's VitalityMod entry: a fused-but-not-riven mod whose
        // fingerprint is only `{"rerolls":0}`, double-stringified — the
        // shape that turned out to dominate a real account's Upgrades[].
        let state = parse_rivens(&fixture()).expect("parses");
        assert!(!state
            .rivens
            .iter()
            .any(|r| r.item_type.contains("VitalityMod")));
    }

    #[test]
    fn skips_an_entry_with_an_unparseable_fingerprint() {
        let json = r#"{"Upgrades":[{"ItemType":"/Lotus/Foo","UpgradeFingerprint":"not json"}]}"#;
        let state = parse_rivens(json).unwrap();
        assert!(state.rivens.is_empty());
    }

    #[test]
    fn ignores_unrelated_top_level_inventory_fields() {
        let json = r#"{"Suits":[{"ItemType":"/Lotus/Warframe"}],"Upgrades":[]}"#;
        let state = parse_rivens(json).unwrap();
        assert!(state.rivens.is_empty());
    }
}
