//! Decode a raw riven fingerprint's encoded buff/curse `Value`s into
//! displayable stat lines (e.g. "+126% Crit Chance"), using
//! [`crate::riven_catalogue::RivenCatalogue`]'s per-weapon Disposition and
//! per-[`crate::riven_catalogue::RivenModCategory`] base stat ranges.
//!
//! Pure, no I/O — the raw fingerprint fields (`compat`/`buffs`/`curses`/…)
//! are parsed by `wf-mem`'s `riven.rs`, which this crate can't depend on
//! (`wf-mem` already depends on `wf-relic`, via `persist.rs` — see that
//! module's doc). So this module takes plain primitives ([`RawStat`], a
//! 1:1 mirror of `wf_mem::riven::RivenStat`'s shape) rather than importing
//! `wf-mem`'s own type; `wf-mem`'s `persist.rs` is the one place that
//! bridges the two.
//!
//! The formula (roll-quality decode, disposition-scaled attenuation,
//! buff/curse-count attenuation tables, rank scaling, non-percentage-tag
//! display rules) is confirmed identical across three independent primary
//! sources — WFHelper's `services/rivenFingerprint.ts`/`rivenConstants.ts`
//! (the most current and detailed of the three, and the one this module
//! follows most closely, including its `isMultiplier` display refinement
//! for faction-damage/combo-bonus tags), the tool WFHelper's own comment
//! credits as its ultimate source (`calamity-inc/warframe-riven-info`'s
//! `RivenParser.js`), and `docs/research/riven-disposition-and-stat-decoding.md`'s
//! own independent read of both. See that research doc for the source
//! citations and worked examples.

use serde::{Deserialize, Serialize};
use wf_data::Polarity;

use crate::riven_catalogue::{RivenCatalogue, RivenModCategory, WeaponRivenInfo};

/// Fingerprint values encode a roll-quality fraction as
/// `round(f * 0x3FFFFFFF)`, not an IEEE float.
const ROLL_INT_MAX: f64 = 0x3FFF_FFFF as f64; // 1_073_741_823
/// A riven's roll quality only ever varies its final stat by ±10% around
/// the nominal (0.5 roll-quality) value.
const ROLL_QUALITY_MIN: f64 = 0.9;
const ROLL_QUALITY_MAX: f64 = 1.1;
const SPECIFIC_FIT_ATTEN: f64 = 1.5;
const BASE_DRAIN: f64 = 10.0;
const CURSE_BONUS_BASE: f64 = 1.25;
/// Indexed by `min(count, len - 1)` — more buffs/curses slotted, smaller
/// each one's share.
const NUM_BUFFS_ATTEN: [f64; 6] = [0.0, 1.0, 0.660_000_03, 0.5, 0.400_000_01, 0.349_999_99];
/// Curse-specific attenuation, indexed by number of *buffs* (not curses).
const NUM_BUFFS_CURSE_ATTEN: [f64; 6] = [0.0, 1.0, 0.330_000_01, 0.5, 1.25, 1.5];

/// Stats displayed as a fixed-precision value or a multiplier instead of a
/// percentage. `docs/research/riven-disposition-and-stat-decoding.md`'s
/// citation of the simpler `RivenParser.js` only names these loosely as
/// "faction damage multipliers" and two melee-combo tags; this exact set is
/// WFHelper's own `rivenConstants.ts::NON_PERCENTAGE_TAGS`, fetched
/// directly and confirmed current as of this implementation.
const NON_PERCENTAGE_TAGS: &[&str] = &[
    "WeaponFactionDamageGrineer",
    "WeaponFactionDamageCorpus",
    "WeaponFactionDamageInfested",
    "WeaponMeleeFactionDamageGrineer",
    "WeaponMeleeFactionDamageCorpus",
    "WeaponMeleeFactionDamageInfested",
    "WeaponMeleeComboInitialBonusMod",
    "ComboDurationMod",
    "WeaponMeleeRangeIncMod",
];

/// One `buffs`/`curses` entry: a stat tag and its raw encoded roll value —
/// a 1:1 mirror of `wf_mem::riven::RivenStat` (see the module doc).
#[derive(Debug, Clone)]
pub struct RawStat {
    pub tag: String,
    pub value: i64,
}

/// One decoded, displayable stat line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecodedStat {
    pub tag: String,
    /// The rounded display value — a percentage for most tags (e.g. `45.7`
    /// meaning "+45.7%"), or the tag's own fixed-precision/multiplier value
    /// for [`NON_PERCENTAGE_TAGS`] (see [`display_value`]).
    pub value: f64,
    /// A curse's stat direction is the *opposite* of its base value's sign
    /// (a damage curse shows negative; a curse on an inherently negative
    /// stat like Recoil shows positive, i.e. "more recoil") — already
    /// folded into `value`'s sign here, so a caller never needs to also
    /// consult whether the source entry was a buff or curse.
    pub is_positive: bool,
    /// Faction-damage and melee-combo-bonus tags display as a `(1 + value)`
    /// multiplier (e.g. `1.25`) rather than a raw percentage/fixed value.
    pub is_multiplier: bool,
    /// True for every [`NON_PERCENTAGE_TAGS`] entry, including the two
    /// (`ComboDurationMod`, `WeaponMeleeRangeIncMod`) that are non-percentage
    /// but *not* [`Self::is_multiplier`] — those display `value` as a plain
    /// fixed-precision number, not a percentage or a multiplier. A caller
    /// only needs to check `is_multiplier` first, then this, to pick the
    /// right unit/format.
    pub is_non_percentage: bool,
}

/// One decoded Unveiled riven, ready to display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecodedRiven {
    pub weapon_name: String,
    pub weapon_unique_name: String,
    pub mod_category: RivenModCategory,
    pub polarity: Option<Polarity>,
    pub mastery_req: Option<i64>,
    pub rank: i64,
    pub rerolls: i64,
    pub stats: Vec<DecodedStat>,
}

/// One Unveiled riven's raw fingerprint fields — a 1:1 mirror of
/// `wf_mem::riven::Riven`'s Unveiled shape (see the module doc), bundled so
/// [`decode`] stays under clippy's `too_many_arguments` threshold.
pub struct RawRiven<'a> {
    pub weapon_unique_name: &'a str,
    pub polarity: Option<Polarity>,
    pub mastery_req: Option<i64>,
    pub rank: Option<i64>,
    pub rerolls: Option<i64>,
    pub buffs: &'a [RawStat],
    pub curses: &'a [RawStat],
}

/// Decode one Unveiled riven's raw fingerprint fields into a [`DecodedRiven`],
/// or `None` if `raw.weapon_unique_name` isn't in `catalogue` (an
/// unrecognized or not-yet-riven-eligible weapon — nothing to decode
/// against). A stat tag `catalogue` has no base value for still produces a
/// `DecodedStat` (value `0.0`, or `1.0` if it's a multiplier tag) rather
/// than being silently dropped, mirroring WFHelper's own `baseValue ?? 0`
/// fallback — an unknown tag is shown as "no signal," not hidden.
pub fn decode(raw: RawRiven, catalogue: &RivenCatalogue) -> Option<DecodedRiven> {
    let weapon = catalogue.weapon(raw.weapon_unique_name)?;
    let rank = raw.rank.unwrap_or(0);
    let num_buffs = raw.buffs.len();
    let num_curses = raw.curses.len();

    let mut stats: Vec<DecodedStat> = raw
        .buffs
        .iter()
        .map(|s| decode_buff(s, &weapon, catalogue, num_buffs, num_curses, rank))
        .collect();
    stats.extend(
        raw.curses
            .iter()
            .map(|s| decode_curse(s, &weapon, catalogue, num_buffs, num_curses, rank)),
    );

    Some(DecodedRiven {
        weapon_name: weapon.name.clone(),
        weapon_unique_name: raw.weapon_unique_name.to_string(),
        mod_category: weapon.mod_category,
        polarity: raw.polarity,
        mastery_req: raw.mastery_req,
        rank,
        rerolls: raw.rerolls.unwrap_or(0),
        stats,
    })
}

/// `Value / 0x3FFFFFFF`, clamped to `[0, 1]` (an out-of-range raw value
/// decodes to `0.0` rather than panicking or wrapping).
fn roll_quality(raw_value: i64) -> f64 {
    let f = raw_value as f64 / ROLL_INT_MAX;
    if (0.0..=1.0).contains(&f) {
        f
    } else {
        0.0
    }
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn attenuation(weapon: &WeaponRivenInfo) -> f64 {
    SPECIFIC_FIT_ATTEN * weapon.omega_attenuation as f64 * BASE_DRAIN
}

fn is_non_percentage(tag: &str) -> bool {
    NON_PERCENTAGE_TAGS.contains(&tag)
}

fn is_multiplier_tag(tag: &str) -> bool {
    is_non_percentage(tag)
        && (tag.contains("FactionDamage") || tag == "WeaponMeleeComboInitialBonusMod")
}

/// Non-percentage tags round to 2 decimals of the raw value directly;
/// percentage tags round to 1 decimal of the raw value expressed as a
/// percentage (`raw * 100`).
fn round_stat_value(raw: f64, is_non_pct: bool) -> f64 {
    if is_non_pct {
        (raw * 100.0).round() / 100.0
    } else {
        (raw * 1000.0).round() / 10.0
    }
}

fn decode_buff(
    stat: &RawStat,
    weapon: &WeaponRivenInfo,
    catalogue: &RivenCatalogue,
    num_buffs: usize,
    num_curses: usize,
    rank: i64,
) -> DecodedStat {
    let base_value = catalogue
        .base_value(weapon.mod_category, &stat.tag)
        .unwrap_or(0.0);
    let is_non_pct = is_non_percentage(&stat.tag);
    let is_multiplier = is_multiplier_tag(&stat.tag);

    if base_value == 0.0 {
        return DecodedStat {
            tag: stat.tag.clone(),
            value: if is_multiplier { 1.0 } else { 0.0 },
            is_positive: true,
            is_multiplier,
            is_non_percentage: is_non_pct,
        };
    }

    let roll_mul = lerp(ROLL_QUALITY_MIN, ROLL_QUALITY_MAX, roll_quality(stat.value));
    let buffs_atten = NUM_BUFFS_ATTEN[num_buffs.min(NUM_BUFFS_ATTEN.len() - 1)];
    let curse_bonus = CURSE_BONUS_BASE.powi(num_curses as i32);
    let raw = base_value
        * attenuation(weapon)
        * curse_bonus
        * roll_mul
        * buffs_atten
        * (rank as f64 + 1.0);

    let value = round_stat_value(raw, is_non_pct);
    let value = if is_multiplier {
        ((1.0 + value) * 100.0).round() / 100.0
    } else {
        value
    };

    DecodedStat {
        tag: stat.tag.clone(),
        value,
        is_positive: true,
        is_multiplier,
        is_non_percentage: is_non_pct,
    }
}

fn decode_curse(
    stat: &RawStat,
    weapon: &WeaponRivenInfo,
    catalogue: &RivenCatalogue,
    num_buffs: usize,
    num_curses: usize,
    rank: i64,
) -> DecodedStat {
    let base_value = catalogue
        .base_value(weapon.mod_category, &stat.tag)
        .unwrap_or(0.0);
    let is_non_pct = is_non_percentage(&stat.tag);
    let is_multiplier = is_multiplier_tag(&stat.tag);

    if base_value == 0.0 {
        return DecodedStat {
            tag: stat.tag.clone(),
            value: if is_multiplier { 1.0 } else { 0.0 },
            is_positive: false,
            is_multiplier,
            is_non_percentage: is_non_pct,
        };
    }

    let roll_mul = lerp(ROLL_QUALITY_MIN, ROLL_QUALITY_MAX, roll_quality(stat.value));
    // Curse-specific attenuation tables, swapped relative to a buff's own:
    // the curse's *own* count reuses the buff-count table, and the buff
    // count present alongside it uses the curse-count table (see the module
    // doc's WFHelper citation — `computeCurseValue`).
    let curses_in_buff_table = NUM_BUFFS_ATTEN[num_curses.min(NUM_BUFFS_ATTEN.len() - 1)];
    let buffs_in_curse_table =
        NUM_BUFFS_CURSE_ATTEN[num_buffs.min(NUM_BUFFS_CURSE_ATTEN.len() - 1)];
    let raw = base_value.abs()
        * attenuation(weapon)
        * roll_mul
        * buffs_in_curse_table
        * curses_in_buff_table
        * (rank as f64 + 1.0);

    let magnitude = round_stat_value(raw, is_non_pct);

    if is_multiplier {
        let value = ((1.0 - magnitude) * 100.0).round() / 100.0;
        return DecodedStat {
            tag: stat.tag.clone(),
            value,
            is_positive: false,
            is_multiplier,
            is_non_percentage: is_non_pct,
        };
    }
    // A curse's displayed direction is the opposite of the stat's own base
    // value sign: a curse on a normally-positive stat (e.g. Crit Chance)
    // shows negative; a curse on an inherently negative-base stat (e.g.
    // Recoil, where "more" is worse) shows positive.
    let (value, is_positive) = if base_value > 0.0 {
        (-magnitude, false)
    } else {
        (magnitude, true)
    };
    DecodedStat {
        tag: stat.tag.clone(),
        value,
        is_positive,
        is_multiplier,
        is_non_percentage: is_non_pct,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn soma_prime() -> (RivenCatalogue, &'static str) {
        let unique_name = "/Lotus/Weapons/Tenno/LongGuns/PrimeSoma/PrimeSomaRifle";
        let catalogue = RivenCatalogue::from_parts_for_test(
            vec![(
                unique_name.to_string(),
                WeaponRivenInfo {
                    name: "Soma Prime".to_string(),
                    disposition: 3,
                    omega_attenuation: 1.1,
                    mod_category: RivenModCategory::Rifle,
                },
            )],
            vec![(
                RivenModCategory::Rifle,
                vec![
                    ("WeaponCritChanceMod".to_string(), 1.5),
                    ("WeaponDamageAmountMod".to_string(), 1.65),
                    ("WeaponRecoilReductionMod".to_string(), 0.9),
                ],
            )],
        );
        (catalogue, unique_name)
    }

    #[test]
    fn unknown_weapon_decodes_to_none() {
        let catalogue = RivenCatalogue::empty();
        let result = decode(
            RawRiven {
                weapon_unique_name: "/Lotus/Unknown",
                polarity: None,
                mastery_req: None,
                rank: Some(8),
                rerolls: Some(3),
                buffs: &[RawStat {
                    tag: "WeaponCritChanceMod".to_string(),
                    value: 823_451_120,
                }],
                curses: &[],
            },
            &catalogue,
        );
        assert!(result.is_none());
    }

    #[test]
    fn decodes_a_known_weapon_with_buffs_and_a_curse() {
        let (catalogue, unique_name) = soma_prime();
        let riven = decode(
            RawRiven {
                weapon_unique_name: unique_name,
                polarity: Some(Polarity::Madurai),
                mastery_req: Some(10),
                rank: Some(8),
                rerolls: Some(3),
                buffs: &[
                    RawStat {
                        tag: "WeaponCritChanceMod".to_string(),
                        value: 823_451_120,
                    },
                    RawStat {
                        tag: "WeaponDamageAmountMod".to_string(),
                        value: 512_009_887,
                    },
                ],
                curses: &[RawStat {
                    tag: "WeaponRecoilReductionMod".to_string(),
                    value: 190_442_017,
                }],
            },
            &catalogue,
        )
        .expect("known weapon decodes");

        assert_eq!(riven.weapon_name, "Soma Prime");
        assert_eq!(riven.mod_category, RivenModCategory::Rifle);
        assert_eq!(riven.rank, 8);
        assert_eq!(riven.rerolls, 3);
        assert_eq!(riven.stats.len(), 3);
        // Buffs are positive; magnitudes are non-zero and finite.
        assert!(riven.stats[0].is_positive && riven.stats[0].value > 0.0);
        assert!(riven.stats[1].is_positive && riven.stats[1].value > 0.0);
        // The curse is negative, since WeaponRecoilReductionMod's base value
        // is positive in this test catalogue (a curse flips a
        // positive-base stat negative).
        assert!(!riven.stats[2].is_positive && riven.stats[2].value < 0.0);
    }

    #[test]
    fn a_missing_stat_tag_falls_back_to_zero_rather_than_being_dropped() {
        let (catalogue, unique_name) = soma_prime();
        let riven = decode(
            RawRiven {
                weapon_unique_name: unique_name,
                polarity: None,
                mastery_req: None,
                rank: Some(0),
                rerolls: Some(0),
                buffs: &[RawStat {
                    tag: "SomeUnrecognizedTag".to_string(),
                    value: 500_000_000,
                }],
                curses: &[],
            },
            &catalogue,
        )
        .expect("known weapon decodes");
        assert_eq!(riven.stats.len(), 1);
        assert_eq!(riven.stats[0].value, 0.0);
    }

    #[test]
    fn more_buffs_slotted_shrinks_each_buffs_own_share() {
        let (catalogue, unique_name) = soma_prime();
        let one_buff = decode(
            RawRiven {
                weapon_unique_name: unique_name,
                polarity: None,
                mastery_req: None,
                rank: Some(8),
                rerolls: Some(0),
                buffs: &[RawStat {
                    tag: "WeaponCritChanceMod".to_string(),
                    value: 700_000_000,
                }],
                curses: &[],
            },
            &catalogue,
        )
        .unwrap();
        let two_buffs = decode(
            RawRiven {
                weapon_unique_name: unique_name,
                polarity: None,
                mastery_req: None,
                rank: Some(8),
                rerolls: Some(0),
                buffs: &[
                    RawStat {
                        tag: "WeaponCritChanceMod".to_string(),
                        value: 700_000_000,
                    },
                    RawStat {
                        tag: "WeaponDamageAmountMod".to_string(),
                        value: 700_000_000,
                    },
                ],
                curses: &[],
            },
            &catalogue,
        )
        .unwrap();
        assert!(two_buffs.stats[0].value < one_buff.stats[0].value);
    }

    #[test]
    fn higher_rank_scales_the_stat_up() {
        let (catalogue, unique_name) = soma_prime();
        let rank_0 = decode(
            RawRiven {
                weapon_unique_name: unique_name,
                polarity: None,
                mastery_req: None,
                rank: Some(0),
                rerolls: Some(0),
                buffs: &[RawStat {
                    tag: "WeaponCritChanceMod".to_string(),
                    value: 700_000_000,
                }],
                curses: &[],
            },
            &catalogue,
        )
        .unwrap();
        let rank_8 = decode(
            RawRiven {
                weapon_unique_name: unique_name,
                polarity: None,
                mastery_req: None,
                rank: Some(8),
                rerolls: Some(0),
                buffs: &[RawStat {
                    tag: "WeaponCritChanceMod".to_string(),
                    value: 700_000_000,
                }],
                curses: &[],
            },
            &catalogue,
        )
        .unwrap();
        assert!(rank_8.stats[0].value > rank_0.stats[0].value);
    }

    #[test]
    fn combo_duration_tag_is_non_percentage_but_not_a_multiplier() {
        let weapons = vec![(
            "/Lotus/Weapons/TestMelee".to_string(),
            WeaponRivenInfo {
                name: "Test Melee".to_string(),
                disposition: 3,
                omega_attenuation: 1.1,
                mod_category: RivenModCategory::Melee,
            },
        )];
        let base_values = vec![(
            RivenModCategory::Melee,
            vec![("ComboDurationMod".to_string(), 1.2)],
        )];
        let catalogue = RivenCatalogue::from_parts_for_test(weapons, base_values);
        let riven = decode(
            RawRiven {
                weapon_unique_name: "/Lotus/Weapons/TestMelee",
                polarity: None,
                mastery_req: None,
                rank: Some(8),
                rerolls: Some(0),
                buffs: &[RawStat {
                    tag: "ComboDurationMod".to_string(),
                    value: 700_000_000,
                }],
                curses: &[],
            },
            &catalogue,
        )
        .unwrap();
        assert!(riven.stats[0].is_non_percentage);
        assert!(!riven.stats[0].is_multiplier);
    }

    #[test]
    fn faction_damage_tags_display_as_a_multiplier() {
        let weapons = vec![(
            "/Lotus/Weapons/Test".to_string(),
            WeaponRivenInfo {
                name: "Test Weapon".to_string(),
                disposition: 3,
                omega_attenuation: 1.1,
                mod_category: RivenModCategory::Rifle,
            },
        )];
        let base_values = vec![(
            RivenModCategory::Rifle,
            vec![("WeaponFactionDamageGrineer".to_string(), 1.2)],
        )];
        let catalogue = RivenCatalogue::from_parts_for_test(weapons, base_values);
        let riven = decode(
            RawRiven {
                weapon_unique_name: "/Lotus/Weapons/Test",
                polarity: None,
                mastery_req: None,
                rank: Some(8),
                rerolls: Some(0),
                buffs: &[RawStat {
                    tag: "WeaponFactionDamageGrineer".to_string(),
                    value: 700_000_000,
                }],
                curses: &[],
            },
            &catalogue,
        )
        .unwrap();
        assert!(riven.stats[0].is_multiplier);
        // A multiplier displays as (1 + magnitude), so it should read above 1.0.
        assert!(riven.stats[0].value > 1.0);
    }

    #[test]
    fn roll_quality_clamps_out_of_range_values_to_zero() {
        assert_eq!(roll_quality(-1), 0.0);
        assert_eq!(roll_quality(i64::MAX), 0.0);
        assert_eq!(roll_quality(0), 0.0);
    }
}
