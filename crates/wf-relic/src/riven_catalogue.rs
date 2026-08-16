//! Reference data [`crate::riven_decode`] needs to turn a raw riven
//! fingerprint into a displayed stat line: per-weapon **Disposition** +
//! `omegaAttenuation`, and per-**Riven type** base stat-roll ranges. Per
//! [issue #95](https://github.com/albrektsson/warframe-lite/issues/95)'s
//! research (`docs/research/riven-disposition-and-stat-decoding.md`), both
//! live in WFCD's `warframe-items` dataset — already vendored per
//! [ADR-0011](../../../docs/adr/0011-warframe-items-for-prime-part-build-quantities.md)
//! and fetched (for a different purpose, Prime build quantities) by
//! [`crate::part_quantities`].
//!
//! This is a **separate** fetch pass, not an extension of
//! `part_quantities.rs`: that module only keeps `isPrime` items' `components`
//! (build-quantity data), which drops disposition/`omegaAttenuation` off
//! every weapon record it touches and skips non-Prime weapons entirely.
//! Riven decoding needs the opposite slice — every weapon (Prime or not, an
//! Unveiled riven can sit on either) that carries a `disposition`.
//!
//! Two modular-weapon cases needed live-verifying against the actual JSON
//! (not just trusting the research doc's spot-check of ordinary weapons):
//! **Kitguns** carry disposition/`omegaAttenuation` on their **Barrel**
//! component (e.g. Catchmoon = `SUModularSecondaryBarrelAPart`), `type:
//! "Kitgun Component"`, inside `Misc.json` — not `Secondary.json`. **Zaws**
//! carry it on their **Tip** component (e.g. Cyath, Dehtat), `type: "Zaw
//! Component"`, inside `Melee.json` — a file already fetched by
//! `part_quantities.rs`, but for build quantities its `isPrime`-only filter
//! silently drops every Zaw/Kitgun component. Confirmed live, 2026-08-15.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

const BASE: &str = "https://raw.githubusercontent.com/WFCD/warframe-items/master/data/json";
const CACHE_FILE: &str = "riven-catalogue-v1.json";

/// Files carrying weapon records with a `disposition` field. `Archwing.json`
/// (Archwing frames) has none — checked live, zero entries — so it's
/// intentionally excluded; Archwing *weapons* (Arch-Gun/Arch-Melee) do.
const WEAPON_CATEGORIES: &[&str] = &[
    "Primary",
    "Secondary",
    "Melee",
    "Arch-Gun",
    "Arch-Melee",
    "Misc",
];

/// The seven Riven-type keys DE's `Mods.json`/`ExportUpgrades.json` define
/// (confirmed exhaustive in `docs/research/riven-disposition-and-stat-decoding.md`
/// §1) — effectively "which weapon-shape a Riven mod rolls for."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RivenModCategory {
    Rifle,
    Shotgun,
    Pistol,
    Melee,
    Archgun,
    Kitgun,
    Zaw,
}

impl RivenModCategory {
    /// `warframe-items`' `Mods.json` `uniqueName` for this Riven type's base
    /// stat-value table — the same seven keys `riven.rs`'s own fixture and
    /// WFHelper's `RIVEN_MODS_BY_CATEGORY` use (research doc §1).
    fn mods_json_key(self) -> &'static str {
        match self {
            RivenModCategory::Rifle => "/Lotus/Upgrades/Mods/Randomized/LotusRifleRandomModRare",
            RivenModCategory::Shotgun => {
                "/Lotus/Upgrades/Mods/Randomized/LotusShotgunRandomModRare"
            }
            RivenModCategory::Pistol => "/Lotus/Upgrades/Mods/Randomized/LotusPistolRandomModRare",
            RivenModCategory::Melee => {
                "/Lotus/Upgrades/Mods/Randomized/PlayerMeleeWeaponRandomModRare"
            }
            RivenModCategory::Archgun => {
                "/Lotus/Upgrades/Mods/Randomized/LotusArchgunRandomModRare"
            }
            RivenModCategory::Kitgun => {
                "/Lotus/Upgrades/Mods/Randomized/LotusModularPistolRandomModRare"
            }
            RivenModCategory::Zaw => {
                "/Lotus/Upgrades/Mods/Randomized/LotusModularMeleeRandomModRare"
            }
        }
    }

    fn from_mods_json_key(key: &str) -> Option<Self> {
        [
            RivenModCategory::Rifle,
            RivenModCategory::Shotgun,
            RivenModCategory::Pistol,
            RivenModCategory::Melee,
            RivenModCategory::Archgun,
            RivenModCategory::Kitgun,
            RivenModCategory::Zaw,
        ]
        .into_iter()
        .find(|t| t.mods_json_key() == key)
    }

    /// Resolve a weapon record's Riven type from its source category file
    /// and in-file `type` string. The file is the primary signal, not
    /// `type` alone: `Melee.json` carries several weapons WFCD mislabels
    /// `type: "Rifle"` (e.g. Mk1-Bo, Prova — real melee weapons, live-checked)
    /// that would resolve wrong under a pure `type`-string mapping. `type`
    /// only refines *within* `Primary.json` (Shotgun vs. everything else,
    /// which is Rifle/Bow/Launcher/Sniper — all share the Rifle Riven type
    /// in-game) and flags the two modular cases (`"Kitgun Component"`,
    /// `"Zaw Component"`), which are file-independent signals.
    fn resolve(category: &str, item_type: &str) -> Option<Self> {
        if item_type == "Kitgun Component" {
            return Some(RivenModCategory::Kitgun);
        }
        if item_type == "Zaw Component" {
            return Some(RivenModCategory::Zaw);
        }
        match category {
            "Primary" => Some(if item_type == "Shotgun" {
                RivenModCategory::Shotgun
            } else {
                RivenModCategory::Rifle
            }),
            "Secondary" => Some(RivenModCategory::Pistol),
            // Zaw Component already handled above; every other Melee.json
            // entry (including the mislabeled ones) is a plain melee weapon.
            "Melee" | "Arch-Melee" => Some(RivenModCategory::Melee),
            "Arch-Gun" => Some(RivenModCategory::Archgun),
            // Misc.json carries far more than Kitgun components (Exalted
            // weapons, etc. also have a `disposition` field but aren't
            // riven-eligible) — only the explicit Kitgun Component check
            // above should ever match here.
            _ => None,
        }
    }
}

/// `(Riven type, [(stat tag, base value)])` pairs — `Mods.json`'s seven
/// base-value tables, pre-`collect`ion into `RivenCatalogue`'s internal
/// map. A type alias purely to keep this shape's several call sites
/// under clippy's `type_complexity` threshold.
type RivenBaseValues = Vec<(RivenModCategory, Vec<(String, f64)>)>;

/// A weapon (or Kitgun Barrel / Zaw Tip shape)'s riven-relevant facts,
/// keyed by its `uniqueName` — the same string a riven fingerprint's
/// `compat` field carries (see [`crate::riven_decode`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeaponRivenInfo {
    /// Display name, e.g. "Soma Prime" or "Catchmoon" — `warframe-items`'
    /// own `name` field, kept here since no other index in this repo maps a
    /// DE `uniqueName` to a display name (`ItemIndex` only does fuzzy
    /// name→market-slug matching, the opposite direction).
    pub name: String,
    /// 1-5, the in-game circle count.
    pub disposition: u8,
    /// The exact float DE's decode formula multiplies by — not always a
    /// simple function of `disposition` (see the research doc), so both are
    /// kept rather than deriving one from the other.
    pub omega_attenuation: f32,
    pub mod_category: RivenModCategory,
}

/// Decode reference data: per-weapon [`WeaponRivenInfo`] and per-[`RivenModCategory`]
/// base stat-roll values, both from WFCD `warframe-items`.
pub struct RivenCatalogue {
    weapons: HashMap<String, WeaponRivenInfo>,
    base_values: HashMap<RivenModCategory, HashMap<String, f64>>,
}

impl RivenCatalogue {
    fn new(data: CatalogueData) -> Self {
        Self {
            weapons: data.weapons.into_iter().collect(),
            base_values: data
                .base_values
                .into_iter()
                .map(|(t, entries)| (t, entries.into_iter().collect()))
                .collect(),
        }
    }

    /// An empty catalogue — every lookup returns `None`. For tests and
    /// callers with no fetch available yet.
    pub fn empty() -> Self {
        Self {
            weapons: HashMap::new(),
            base_values: HashMap::new(),
        }
    }

    /// Build a catalogue directly from known weapon/base-value entries, for
    /// tests elsewhere in the crate (e.g. `riven_decode`) that need a known
    /// catalogue without going through a fetch — mirrors
    /// [`crate::part_quantities::PartQuantities::from_entries_for_test`].
    pub fn from_parts_for_test(
        weapons: Vec<(String, WeaponRivenInfo)>,
        base_values: RivenBaseValues,
    ) -> Self {
        Self::new(CatalogueData {
            weapons,
            base_values,
        })
    }

    pub fn weapon(&self, unique_name: &str) -> Option<WeaponRivenInfo> {
        self.weapons.get(unique_name).cloned()
    }

    /// The base roll value a fingerprint's encoded stat `Value` scales
    /// against, for `tag` under `mod_category`.
    pub fn base_value(&self, mod_category: RivenModCategory, tag: &str) -> Option<f64> {
        self.base_values.get(&mod_category)?.get(tag).copied()
    }

    /// Fetch + cache (weekly TTL, stale-served on failure), mirroring
    /// [`crate::part_quantities::PartQuantities::load_cached`].
    pub async fn load_cached(client: &reqwest::Client, ttl: Duration) -> anyhow::Result<Self> {
        if let Some(cached) = wf_cache::load_blob::<CatalogueData>(CACHE_FILE) {
            if cached.age() < ttl {
                tracing::info!(
                    "riven catalogue from cache ({} weapons, {} riven types)",
                    cached.value.weapons.len(),
                    cached.value.base_values.len()
                );
                return Ok(Self::new(cached.value));
            }
            match fetch(client).await {
                Ok(data) => {
                    let _ = wf_cache::save_blob(CACHE_FILE, &data);
                    return Ok(Self::new(data));
                }
                Err(e) => {
                    tracing::warn!("riven catalogue refresh failed ({e}); using stale cache");
                    return Ok(Self::new(cached.value));
                }
            }
        }
        let data = fetch(client).await?;
        let _ = wf_cache::save_blob(CACHE_FILE, &data);
        Ok(Self::new(data))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CatalogueData {
    weapons: Vec<(String, WeaponRivenInfo)>,
    base_values: RivenBaseValues,
}

#[derive(Debug, Deserialize)]
struct RawWeapon {
    name: String,
    #[serde(rename = "uniqueName")]
    unique_name: String,
    #[serde(default, rename = "type")]
    item_type: String,
    disposition: Option<u8>,
    #[serde(rename = "omegaAttenuation")]
    omega_attenuation: Option<f32>,
}

fn parse_weapon_category(
    body: &str,
    category: &str,
) -> anyhow::Result<Vec<(String, WeaponRivenInfo)>> {
    let items: Vec<RawWeapon> = serde_json::from_str(body)?;
    Ok(items
        .into_iter()
        .filter_map(|w| {
            let disposition = w.disposition?;
            let omega_attenuation = w.omega_attenuation?;
            let mod_category = RivenModCategory::resolve(category, &w.item_type)?;
            Some((
                w.unique_name,
                WeaponRivenInfo {
                    name: w.name,
                    disposition,
                    omega_attenuation,
                    mod_category,
                },
            ))
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct RawMod {
    #[serde(rename = "uniqueName")]
    unique_name: String,
    #[serde(default, rename = "upgradeEntries")]
    upgrade_entries: Vec<RawUpgradeEntry>,
}

#[derive(Debug, Deserialize)]
struct RawUpgradeEntry {
    tag: String,
    #[serde(default, rename = "upgradeValues")]
    upgrade_values: Vec<RawUpgradeValue>,
}

#[derive(Debug, Deserialize)]
struct RawUpgradeValue {
    value: f64,
}

/// Every seven Riven-type base-value tables out of `Mods.json`'s much larger
/// mod list — matched by [`RivenModCategory::from_mods_json_key`], everything else
/// (ordinary fusable mods) ignored.
fn parse_mods(body: &str) -> anyhow::Result<RivenBaseValues> {
    let items: Vec<RawMod> = serde_json::from_str(body)?;
    Ok(items
        .into_iter()
        .filter_map(|m| {
            let mod_category = RivenModCategory::from_mods_json_key(&m.unique_name)?;
            let entries = m
                .upgrade_entries
                .into_iter()
                .filter_map(|e| e.upgrade_values.first().map(|v| (e.tag, v.value)))
                .collect();
            Some((mod_category, entries))
        })
        .collect())
}

async fn fetch(client: &reqwest::Client) -> anyhow::Result<CatalogueData> {
    let mut weapons = Vec::new();
    for category in WEAPON_CATEGORIES {
        let url = format!("{BASE}/{category}.json");
        tracing::debug!("GET {url}");
        let resp = match client
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
        {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!("riven catalogue: fetching {category} failed: {e}");
                continue;
            }
        };
        let body = match resp.text().await {
            Ok(body) => body,
            Err(e) => {
                tracing::warn!("riven catalogue: reading {category} failed: {e}");
                continue;
            }
        };
        match parse_weapon_category(&body, category) {
            Ok(mut parsed) => weapons.append(&mut parsed),
            Err(e) => tracing::warn!("riven catalogue: parsing {category} failed: {e}"),
        }
    }

    let mods_url = format!("{BASE}/Mods.json");
    tracing::debug!("GET {mods_url}");
    let base_values = match client
        .get(&mods_url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
    {
        Ok(resp) => match resp.text().await {
            Ok(body) => parse_mods(&body).unwrap_or_else(|e| {
                tracing::warn!("riven catalogue: parsing Mods.json failed: {e}");
                Vec::new()
            }),
            Err(e) => {
                tracing::warn!("riven catalogue: reading Mods.json failed: {e}");
                Vec::new()
            }
        },
        Err(e) => {
            tracing::warn!("riven catalogue: fetching Mods.json failed: {e}");
            Vec::new()
        }
    };

    if weapons.is_empty() && base_values.is_empty() {
        anyhow::bail!("no riven catalogue data fetched");
    }
    Ok(CatalogueData {
        weapons,
        base_values,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_weapon_category_keeps_only_entries_with_a_disposition() {
        let body = r#"[
            {"name": "Soma Prime", "uniqueName": "/Lotus/Weapons/Tenno/LongGuns/PrimeSoma/PrimeSomaRifle",
             "type": "Rifle", "disposition": 3, "omegaAttenuation": 1.1},
            {"name": "Some Non-Weapon", "uniqueName": "/Lotus/Foo"}
        ]"#;
        let entries = parse_weapon_category(body, "Primary").unwrap();
        assert_eq!(entries.len(), 1);
        let (name, info) = &entries[0];
        assert_eq!(
            name,
            "/Lotus/Weapons/Tenno/LongGuns/PrimeSoma/PrimeSomaRifle"
        );
        assert_eq!(info.disposition, 3);
        assert_eq!(info.omega_attenuation, 1.1);
        assert_eq!(info.mod_category, RivenModCategory::Rifle);
    }

    #[test]
    fn primary_json_shotgun_type_resolves_to_shotgun_riven_type() {
        let body = r#"[{"name": "Sobek", "uniqueName": "/Lotus/Weapons/Tenno/Shotgun/DoubleBarrelShotgun",
            "type": "Shotgun", "disposition": 5, "omegaAttenuation": 1.33}]"#;
        let entries = parse_weapon_category(body, "Primary").unwrap();
        assert_eq!(entries[0].1.mod_category, RivenModCategory::Shotgun);
    }

    #[test]
    fn melee_json_ignores_the_mislabeled_type_field_and_resolves_to_melee() {
        // Mk1-Bo is a real melee weapon that WFCD mislabels `type: "Rifle"`
        // inside Melee.json — the file, not the in-record type, must win.
        let body = r#"[{"name": "Mk1-Bo", "uniqueName": "/Lotus/Weapons/MK1Series/MK1Bo",
            "type": "Rifle", "disposition": 5, "omegaAttenuation": 1.35}]"#;
        let entries = parse_weapon_category(body, "Melee").unwrap();
        assert_eq!(entries[0].1.mod_category, RivenModCategory::Melee);
    }

    #[test]
    fn melee_json_zaw_component_resolves_to_zaw() {
        let body = r#"[{"name": "Cyath", "uniqueName": "/Lotus/Weapons/Ostron/Melee/ModularMelee01/Tip/TipFour",
            "type": "Zaw Component", "disposition": 3, "omegaAttenuation": 1.0}]"#;
        let entries = parse_weapon_category(body, "Melee").unwrap();
        assert_eq!(entries[0].1.mod_category, RivenModCategory::Zaw);
    }

    #[test]
    fn misc_json_kitgun_component_resolves_to_kitgun_and_other_disposition_entries_are_dropped() {
        let body = r#"[
            {"name": "Catchmoon", "uniqueName": "/Lotus/Weapons/SolarisUnited/Secondary/SUModularSecondarySet1/Barrel/SUModularSecondaryBarrelAPart",
             "type": "Kitgun Component", "disposition": 1, "omegaAttenuation": 0.75},
            {"name": "Artemis Bow", "uniqueName": "/Lotus/Weapons/SomeExalted",
             "type": "Exalted Weapon", "disposition": 3, "omegaAttenuation": 1.0}
        ]"#;
        let entries = parse_weapon_category(body, "Misc").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].1.mod_category, RivenModCategory::Kitgun);
    }

    #[test]
    fn parse_mods_extracts_only_the_seven_riven_type_tables() {
        let body = r#"[
            {"uniqueName": "/Lotus/Upgrades/Mods/Randomized/LotusRifleRandomModRare",
             "upgradeEntries": [{"tag": "WeaponCritChanceMod", "upgradeValues": [{"value": 1.5}]}]},
            {"uniqueName": "/Lotus/Upgrades/Mods/SomeOrdinaryMod",
             "upgradeEntries": [{"tag": "Foo", "upgradeValues": [{"value": 9.9}]}]}
        ]"#;
        let entries = parse_mods(body).unwrap();
        assert_eq!(entries.len(), 1);
        let (mod_category, values) = &entries[0];
        assert_eq!(*mod_category, RivenModCategory::Rifle);
        assert_eq!(values, &vec![("WeaponCritChanceMod".to_string(), 1.5)]);
    }

    #[test]
    fn catalogue_lookups_roundtrip_through_the_cached_data_shape() {
        let data = CatalogueData {
            weapons: vec![(
                "/Lotus/Weapons/Tenno/LongGuns/PrimeSoma/PrimeSomaRifle".to_string(),
                WeaponRivenInfo {
                    name: "Soma Prime".to_string(),
                    disposition: 3,
                    omega_attenuation: 1.1,
                    mod_category: RivenModCategory::Rifle,
                },
            )],
            base_values: vec![(
                RivenModCategory::Rifle,
                vec![("WeaponCritChanceMod".to_string(), 1.5)],
            )],
        };
        let catalogue = RivenCatalogue::new(data);
        assert_eq!(
            catalogue
                .weapon("/Lotus/Weapons/Tenno/LongGuns/PrimeSoma/PrimeSomaRifle")
                .map(|i| i.disposition),
            Some(3)
        );
        assert_eq!(catalogue.weapon("/Lotus/Unknown"), None);
        assert_eq!(
            catalogue.base_value(RivenModCategory::Rifle, "WeaponCritChanceMod"),
            Some(1.5)
        );
        assert_eq!(
            catalogue.base_value(RivenModCategory::Rifle, "UnknownTag"),
            None
        );
    }

    #[test]
    fn empty_catalogue_returns_none_for_every_lookup() {
        let catalogue = RivenCatalogue::empty();
        assert_eq!(catalogue.weapon("/Lotus/Anything"), None);
        assert_eq!(
            catalogue.base_value(RivenModCategory::Melee, "Anything"),
            None
        );
    }
}
