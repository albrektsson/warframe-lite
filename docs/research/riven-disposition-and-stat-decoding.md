# Where Riven Disposition and per-stat base ranges come from

Research for [issue #95](https://github.com/albrektsson/warframe-lite/issues/95),
child of the wayfinder map [issue #94](https://github.com/albrektsson/warframe-lite/issues/94).
`crates/wf-mem/src/riven.rs`'s `parse_rivens` extracts raw *encoded* buff/curse
values off `Upgrades[].UpgradeFingerprint` (e.g. `823451120`) but explicitly
does not decode them into a displayable stat line (e.g. "+126% Crit Chance") —
that decode needs (a) each weapon's **Disposition** and (b) each stat tag's
base roll range for that weapon's **Riven type**, neither of which the module
carries today (see its module doc, lines 1-14).

## Question

Does WFCD's `warframe-items` dataset (already fetched by this repo, per
[ADR-0011](../adr/0011-warframe-items-for-prime-part-build-quantities.md) and
`crates/wf-relic/src/part_quantities.rs`) carry weapon disposition and/or
per-riven-type base stat ranges? Does DE publish disposition anywhere itself,
programmatically? What does WFHelper's `services/rivenFingerprint.ts` — the
file this repo's `riven.rs` module doc already cites — actually pull
disposition/base-range data from, and is the raw `Value` decode formula known?
Is there a maintained WFCD (or WFCD-adjacent) riven/disposition dataset beyond
`warframe-items`? Everything had to be a fetchable API or bundleable data
file — no HTML scraping, matching this repo's existing API-only policy.

## Answer, in short

**WFCD's `warframe-items` — the dataset this repo already fetches per
ADR-0011 — carries everything needed, and the raw-`Value`-to-percentage
formula is fully known and confirmed from two independent primary sources
that agree byte-for-byte.** No new data source or dependency is required;
this is an extension of infrastructure the repo already has.

- **Disposition**: every weapon entry in `warframe-items`' per-category files
  (`Primary.json`, `Secondary.json`, `Melee.json`, `Archwing.json`, `Arch-Gun.json`,
  the same files `part_quantities.rs` already fetches) carries a `disposition`
  field (the 1-5 in-game circle count as an integer, e.g. Soma Prime = `3`)
  **and** an `omegaAttenuation` field (the exact float DE's own formula
  multiplies by, e.g. Soma Prime = `1.1`) — confirmed by fetching
  `Primary.json` directly and reading Soma Prime's, Braton's, Braton Prime's,
  and Boltor Prime's records (§1).
- **Per-riven-type base stat ranges**: `warframe-items`' `Mods.json` (part of
  the same dataset, not currently in `part_quantities.rs`'s fetched category
  list) carries exactly seven `/Lotus/Upgrades/Mods/Randomized/*RandomModRare`
  entries — one per Riven type (Rifle, Shotgun, Pistol, Melee, Archgun,
  Kitgun, Zaw) — each with an `upgradeEntries[]` array of `{tag, prefixTag,
  suffixTag, upgradeValues: [{value, locTag}]}`, where `upgradeValues[0].value`
  is the base roll value a fingerprint's encoded `Value` scales against (§1).
- **The decode formula** (fixed-point roll fraction combined with disposition,
  base value, buff/curse counts, and rank through a specific formula — not a
  simple linear percentage) is fully documented in source and independently
  confirmed identical in two places: WFHelper's own `services/rivenFingerprint.ts`
  (via `services/rivenData.ts` + `services/rivenConstants.ts`) and the tool
  WFHelper's own comment credits as its ultimate source,
  `calamity-inc/warframe-riven-info`'s `RivenParser.js` (the engine behind
  `browse.wf/rivencalc`, published on npm as `warframe-riven-info`) — see §3
  for the exact formula and constants.
- **DE does not publish disposition through any fetchable structured
  API/file.** The field originates in DE's own Public Export game-data dump
  (`omegaAttenuation` in `ExportWeapons`), but DE's raw Public Export is a
  compressed, non-trivial-to-parse asset manifest, not a simple JSON
  endpoint — every community consumer (including `warframe-items`) re-publishes
  an already-parsed copy of that same DE-originated field rather than reading
  DE's raw export directly. Disposition *changes* (DE tuning them per hotfix)
  are announced only as prose in patch notes / forum posts — genuinely no
  structured export of "what changed" exists, but this doesn't matter for
  warframe-lite since `warframe-items` re-syncs from the current export daily
  (confirmed: same-day commits as this research) and always reflects the
  current live value, not a diff log.
- No separate WFCD (or WFCD-adjacent) riven-dataset repo needs to be added.
  `calamity-inc/warframe-riven-info` exists and is real, but it's strictly
  smaller than what's already in `warframe-items` (it has base values, not
  disposition, and hasn't had a data commit since 2024-09) — see §4.

**Net conclusion: no fallback / hand-maintained table is needed.** Add
`Mods.json` to `part_quantities.rs`'s `CATEGORIES` list (or fetch it
separately), pull `disposition`/`omegaAttenuation` off each weapon record
already being fetched, and apply the formula in §3 to `riven.rs`'s existing
raw `RivenStat { tag, value }` pairs.

## 1. WFCD `warframe-items` — disposition and base ranges, read from the actual JSON

Fetched directly, 2026-08-15:

**Disposition**, from
[`data/json/Primary.json`](https://raw.githubusercontent.com/WFCD/warframe-items/master/data/json/Primary.json)
(the exact file `part_quantities.rs`'s `CATEGORIES` already lists, line 27-37):

```json
{
  "name": "Soma Prime",
  "uniqueName": "/Lotus/Weapons/Tenno/LongGuns/PrimeSoma/PrimeSomaRifle",
  ...
  "omegaAttenuation": 1.1,
  ...
  "disposition": 3,
  ...
}
```

Spot-checked across four weapons in the same file:

| Weapon | `disposition` (circles) | `omegaAttenuation` (raw multiplier) |
|---|---|---|
| Soma Prime | 3 | 1.1 |
| Boltor Prime | 4 | 1.2 |
| Braton | 5 | 1.35 |
| Braton Prime | 4 | 1.25 |

Both fields are present on every weapon record already covered by
`part_quantities.rs`'s existing `CATEGORIES` fetch list (`Warframes`,
`Primary`, `Secondary`, `Melee`, `Sentinels`, `SentinelWeapons`, `Archwing`,
`Arch-Gun`, `Arch-Melee`, `Pets` — `crates/wf-relic/src/part_quantities.rs`
lines 26-37) — no new host or endpoint needed, just reading two extra fields
off records already being fetched for build-quantity purposes.

**Per-riven-type base values**, from
[`data/json/Mods.json`](https://raw.githubusercontent.com/WFCD/warframe-items/master/data/json/Mods.json)
(not currently in `part_quantities.rs`'s `CATEGORIES` — would need adding).
Exactly seven entries match `/Lotus/Upgrades/Mods/Randomized/*RandomModRare`
and carry a populated `upgradeEntries[]`:

```
/Lotus/Upgrades/Mods/Randomized/LotusRifleRandomModRare       ("Rifle Riven Mod")
/Lotus/Upgrades/Mods/Randomized/LotusShotgunRandomModRare     ("Shotgun Riven Mod")
/Lotus/Upgrades/Mods/Randomized/LotusPistolRandomModRare      ("Pistol Riven Mod")
/Lotus/Upgrades/Mods/Randomized/PlayerMeleeWeaponRandomModRare("Melee Riven Mod")
/Lotus/Upgrades/Mods/Randomized/LotusArchgunRandomModRare     ("Archgun Riven Mod")
/Lotus/Upgrades/Mods/Randomized/LotusModularPistolRandomModRare("Kitgun Riven Mod")
/Lotus/Upgrades/Mods/Randomized/LotusModularMeleeRandomModRare ("Zaw Riven Mod")
```

These are the exact same seven keys `riven.rs`'s own fixture and WFHelper's
`RIVEN_MODS_BY_CATEGORY`/`SHOTGUN_RIVEN_KEY`/`KITGUN_RIVEN_KEY`/`ZAW_RIVEN_KEY`
constants use (`rivenData.ts` lines 157-169, quoted in §2). One entry, melee,
read in full:

```json
{
  "uniqueName": "/Lotus/Upgrades/Mods/Randomized/PlayerMeleeWeaponRandomModRare",
  "name": "Melee Riven Mod",
  "upgradeEntries": [
    {
      "tag": "WeaponMeleeDamageMod",
      "prefixTag": "visi",
      "suffixTag": "ata",
      "upgradeValues": [
        { "value": 0.018300001, "locTag": "|val|% Melee Damage" }
      ]
    },
    {
      "tag": "WeaponArmorPiercingDamageMod",
      "prefixTag": "insi",
      "suffixTag": "cak",
      "upgradeValues": [
        { "value": 0.0133, "locTag": "|val|% <DT_PUNCTURE_COLOR>Puncture" },
        { "value": 0.066500001 }
      ]
    },
    ...
```

`upgradeValues[0].value` is the base value the decode formula (§3) multiplies
by. This is the *exact same shape* WFHelper's `rivenData.ts` builds its
`UpgradeEntry.baseValue` index from (compare `ensureBuilt()`,
`rivenData.ts` line 246: `const baseValue = ue.upgradeValues?.[0]?.value ?? 0;`)
— `warframe-items`' `Mods.json` and the `warframe-public-export-plus` package
WFHelper actually consumes (§2) are two independently-published re-exports of
the same underlying DE game data, and they match field-for-field.

`Mods.json` is a moderate add: it's one file among the same `data/json/`
directory `part_quantities.rs` already fetches from
(`https://raw.githubusercontent.com/WFCD/warframe-items/master/data/json/`,
`part_quantities.rs` line 13), not a new host, cache mechanism, or fetch
pattern.

## 2. WFHelper's `rivenFingerprint.ts` — traced to its actual data source

`crates/wf-mem/src/riven.rs`'s module doc (lines 1-14) already points at
WFHelper's `services/rivenFingerprint.ts` as the reference for the raw
fingerprint field shapes. Fetched directly, 2026-08-15
([raw.githubusercontent.com/WFHelper/WFHelper/main/services/rivenFingerprint.ts](https://raw.githubusercontent.com/WFHelper/WFHelper/main/services/rivenFingerprint.ts)):
its imports are

```ts
import * as rivenData from "./rivenData";
import * as rivenGrading from "./rivenGrading";
import {
  NUM_BUFFS_ATTEN, NUM_BUFFS_CURSE_ATTEN, SPECIFIC_FIT_ATTEN,
  BASE_DRAIN, NON_PERCENTAGE_TAGS,
} from "./rivenConstants";
```

`decodeSingleRiven` (lines 273-422) calls
`rivenData.getWeaponDisposition(weaponName)`,
`rivenData.resolveRivenType(weaponName)`, and
`rivenData.findUpgradeEntry(rivenTypeKey, tag)` — i.e. disposition and
base-value lookups are delegated entirely to `services/rivenData.ts`, fetched
and read directly
([raw.githubusercontent.com/.../services/rivenData.ts](https://raw.githubusercontent.com/WFHelper/WFHelper/main/services/rivenData.ts)).
That file's `ensureBuilt()` (lines 200-283) shows exactly where the data
actually comes from — not a WFHelper-maintained table, and not WFCD:

```ts
const pep = require("warframe-public-export-plus") as Record<string, any>;
const dict: Record<string, string> = pep.dict_en || {};
const weapons: Record<string, Record<string, any>> = pep.ExportWeapons || {};
const upgrades: Record<string, Record<string, any>> = pep.ExportUpgrades || {};
...
_weaponByNameLc.set(name.toLowerCase(), {
  uniqueName,
  omegaAttenuation: w.omegaAttenuation,
  productCategory: w.productCategory || "",
  ...
```

and

```ts
export function getWeaponDisposition(weaponName: string): number | null {
  ensureBuilt();
  const info = _weaponByNameLc.get(weaponName.toLowerCase());
  return info ? info.omegaAttenuation : null;
}
```

`warframe-public-export-plus` is an npm package
([registry.npmjs.org/warframe-public-export-plus](https://registry.npmjs.org/warframe-public-export-plus),
checked directly: `latest: 0.6.8`, `repository: git+https://github.com/calamity-inc/warframe-public-export-plus.git`).
Its GitHub repo is
[calamity-inc/warframe-public-export-plus](https://github.com/calamity-inc/warframe-public-export-plus)
— the same `calamity-inc` org behind `Sentinel-for-Warframe`, already noted
in `docs/research/mobile-inventory-api-coverage.md` §4. Its root directory
(checked via the GitHub API) is a flat set of `Export*.json` files fetchable
individually over raw HTTPS, no npm install needed:
`ExportWeapons.json`, `ExportUpgrades.json`, `ExportRelics.json`, etc.
Fetched `ExportWeapons.json` directly (1.6MB) and confirmed the same
`omegaAttenuation` field, e.g. `/Lotus/Weapons/VoidTrader/VTDetron` →
`omegaAttenuation: 1.15`, `productCategory: "Pistols"`. Its `README.md`
(fetched directly) states outright: *"Kitgun Chambers also have a
`primeOmegaAttenuation` field, this is the Riven Disposition for when the
Kitgun is a primary instead of secondary weapon"* — confirming
`omegaAttenuation`/`primeOmegaAttenuation` is documented, by name, as riven
disposition. Fetched `ExportUpgrades.json` directly and confirmed
`/Lotus/Upgrades/Mods/Randomized/LotusArchgunRandomModRare` carries the exact
`upgradeEntries[].tag` / `upgradeValues[0].value` shape `rivenData.ts` reads
`baseValue` from — the same shape found independently in WFCD `Mods.json`
(§1).

**So the chain is**: `riven.rs`'s cited reference (WFHelper's
`rivenFingerprint.ts`) → `rivenData.ts` → npm package
`warframe-public-export-plus` → GitHub `calamity-inc/warframe-public-export-plus`
→ raw `ExportWeapons.json`/`ExportUpgrades.json`, fetchable directly over
HTTPS, no scraping. This independently corroborates that WFCD `warframe-items`
(§1) carries the same two pieces of data warframe-lite would otherwise have
gone and fetched from this second source — the repo doesn't need to add
`warframe-public-export-plus` as a second dependency when `warframe-items` is
already vendored and already carries equivalent fields.

## 3. The raw `Value` decode formula — confirmed from two independent primary sources

WFHelper's `services/rivenFingerprint.ts` (fetched directly, quoted above)
computes:

```ts
function rivenIntToFloat(i: number): number {
  const f = i / 0x3fffffff; // 1073741823
  return f >= 0.0 && f <= 1.0 ? f : 0.0;
}

function computeBuffValue(baseValue, disposition, rollFloat, numBuffs, numCurses, lvl): number {
  const attenuation = SPECIFIC_FIT_ATTEN * disposition * BASE_DRAIN;
  const buffsAtten = NUM_BUFFS_ATTEN[Math.min(numBuffs, NUM_BUFFS_ATTEN.length - 1)];
  const curseBonus = Math.pow(1.25, numCurses);
  const rollMul = lerp(0.9, 1.1, rollFloat);
  return baseValue * attenuation * curseBonus * rollMul * buffsAtten * (lvl + 1);
}
```

with (from `services/rivenConstants.ts`, fetched directly):

```ts
export const NUM_BUFFS_ATTEN = [0, 1, 0.66000003, 0.5, 0.40000001, 0.34999999];
export const NUM_BUFFS_CURSE_ATTEN = [0, 1, 0.33000001, 0.5, 1.25, 1.5];
export const SPECIFIC_FIT_ATTEN = 1.5;
export const BASE_DRAIN = 10;
```

and the code comment directly above `rivenIntToFloat` in
`rivenFingerprint.ts` itself credits its source:

```
// Fingerprint Values encode rolls as Math.round(f * 0x3FFFFFFF), not IEEE floats.
// Source: browse.wf/rivencalc -> RivenParser.js `rivenIntToFloat`.
```

That cited source, `calamity-inc/warframe-riven-info`'s `RivenParser.js`
(the engine behind `browse.wf/rivencalc`, also published on npm as
`warframe-riven-info`), was fetched and read directly
([raw.githubusercontent.com/.../RivenParser.js](https://raw.githubusercontent.com/calamity-inc/warframe-riven-info/senpai/RivenParser.js))
— and matches WFHelper's reimplementation constant-for-constant:

```js
function rivenIntToFloat(i) {
    const f = i / 0x3FFFFFFF; // 1073741823
    if (f >= 0.0 && f <= 1.0) { return f; }
    return 0.0;
}

const numBuffsAtten = [0, 1, .66000003, .5, .40000001, .34999999];
const numBuffsCurseAtten = [0, 1, .33000001, .5, 1.25, 1.5];

function parseRiven(rivenType, fingerprint, omegaAttenuation) {
    const curseAtten = Math.pow(1.25, fingerprint.curses.length);
    let attenuation = 1;
    attenuation *= 1.5; // SPECIFIC_FIT_ATTENUATION
    attenuation *= omegaAttenuation;
    attenuation *= 10; // getBaseDrain(RIVEN_BASE_DRAIN)

    for (const buff of fingerprint.buffs) {
        let upgradeValue = riven_tags[rivenType].find(x => x.tag == buff.Tag).value;
        upgradeValue *= attenuation;
        upgradeValue *= curseAtten;
        upgradeValue *= lerp(0.9, 1.1, rivenIntToFloat(buff.Value));
        upgradeValue *= numBuffsAtten[Math.min(fingerprint.buffs.length, numBuffsAtten.length - 1)];
        upgradeValue *= fingerprint.lvl + 1;
        ...
    }
    for (const curse of fingerprint.curses) {
        let upgradeValue = riven_tags[rivenType].find(x => x.tag == curse.Tag).value * -1.0;
        upgradeValue *= attenuation;
        upgradeValue *= lerp(0.9, 1.1, rivenIntToFloat(curse.Value));
        upgradeValue *= numBuffsCurseAtten[Math.min(fingerprint.buffs.length, numBuffsCurseAtten.length - 1)];
        upgradeValue *= numBuffsAtten[Math.min(fingerprint.curses.length, numBuffsAtten.length - 1)];
        upgradeValue *= fingerprint.lvl + 1;
        ...
    }
}

function valueToDisplayValue(tag, value) {
    // a handful of tags (faction damage, combo-related) round/scale differently
    if (isFactionDamageTag(tag)) return Math.round(value * 100) / 100;
    if (tag == "WeaponMeleeComboInitialBonusMod" || tag == "ComboDurationMod" || tag == "WeaponMeleeRangeIncMod")
        return Math.round(value * 10) / 10;
    return Math.round(value * 1000) / 10; // the common case: percentage, one decimal
}
```

**Two independent implementations, one from WFHelper's own TypeScript
reimplementation and one from the tool WFHelper's own comment names as its
source, agree on every constant** (`0x3FFFFFFF`, `1.5`, `10`,
`[0, 1, .66000003, .5, .40000001, .34999999]`,
`[0, 1, .33000001, .5, 1.25, 1.5]`, the `0.9..1.1` roll-quality lerp band, the
`1.25^numCurses` buff bonus, the `(lvl + 1)` rank scaling). This is not a
simple linear percentage or a plain fixed-point unscale — it's:

1. Decode the raw integer `Value` into a roll-quality fraction in `[0, 1]`:
   `rollFloat = Value / 0x3FFFFFFF` (clamped).
2. Map that fraction onto a `[0.9, 1.1]` multiplier band via linear
   interpolation (`lerp`) — a riven's roll quality only ever varies the final
   stat by ±10% around its nominal value.
3. Multiply the riven type's base value (§1/§2) by: `1.5 * disposition * 10`
   (the disposition-scaled attenuation), the roll-quality multiplier, a
   buff-count attenuation table (more buffs slotted = smaller each), a
   curse-count bonus (`1.25^numCurses`, buffs only), and `(rank + 1)`
   (rank 0-8, so ×1 to ×9 as the riven is fused up).
4. Round and scale for display — the common case is `round(value * 1000) / 10`
   (a percentage to one decimal), but a handful of tags (faction damage
   multipliers, `WeaponMeleeComboInitialBonusMod`, `ComboDurationMod`,
   `WeaponMeleeRangeIncMod`) use different rounding/display conventions
   (`NON_PERCENTAGE_TAGS` in `rivenConstants.ts`, matched by
   `valueToDisplayValue`'s tag checks in `RivenParser.js`).

This fully answers the "is the formula known" half of the original question:
yes, in complete and mutually-corroborated detail, with every constant
sourced from actual code, not a blog summary.

## 4. Other WFCD / WFCD-adjacent riven datasets checked

- **`calamity-inc/warframe-riven-info`** (found via search, then verified by
  reading its actual contents): a small, purpose-built repo — `RivenParser.js`
  (the formula in §3) plus a 25KB `riven_tags.json` carrying exactly the same
  seven riven-type base-value tables found in `warframe-items`' `Mods.json`
  and `warframe-public-export-plus`'s `ExportUpgrades.json` (§1/§2). Also
  published on npm as `warframe-riven-info` (`registry.npmjs.org`, checked
  directly: latest `0.1.2`, published 2024-09-06). It does **not** carry
  disposition at all — `parseRiven`'s signature takes `omegaAttenuation` as a
  caller-supplied parameter, so a disposition source would still be needed
  alongside it. Its last GitHub commit (checked via the API) was 2025-06-01
  ("Update links"); its last data-bearing commit was 2024-09-06 — plausible
  staleness risk if DE ever adds a new stat tag, though riven base values
  themselves essentially never change once a tag exists. Strictly a subset of
  what `warframe-items` already provides for this repo's purposes.
- **The WFCD GitHub org itself** (`gh api orgs/WFCD/repos`, full listing
  read directly): no `warframe-riven-info`, `warframe-rivens`, or similarly
  named repo exists under `WFCD` proper. Riven data appears to live folded
  into `warframe-items` (§1) and, per a web search, a `/riven/data` route on
  WFCD's `warframe-hub`/`hub.warframestat.us` project — not independently
  verified further here since `warframe-items` alone was already confirmed
  sufficient and is the dataset already integrated in this repo.
- **DE's own Public Export**: per the Warframe Wiki's own
  `Public Export` page and the `warframe-public-export-plus` README (§2),
  `omegaAttenuation` originates in DE's raw asset export
  (`content.warframe.com/PublicExport/Manifest/...`), which is DE's actual
  source of truth — but that raw export is a compressed manifest format
  requiring purpose-built extraction tooling (the wiki cites
  `Puxtril/Warframe-Exporter` for this), not a simple JSON GET. No search
  turned up a DE-hosted, already-parsed JSON endpoint for weapon data or
  disposition specifically. Disposition-tuning patch notes are prose-only
  (dev workshops / hotfix posts on the Warframe forums) — no structured
  "disposition changelog" feed exists anywhere that was found. None of this
  changes the recommendation, since `warframe-items` already re-publishes the
  parsed field and updates same-day with DE's changes (its most recent commit
  at research time, 2026-08-15, is the same day as this research).

## Caveats and gaps

- `Mods.json` is not currently in `part_quantities.rs`'s `CATEGORIES` list
  (lines 26-37) — implementing the decode will need to add it (or fetch it
  standalone), plus handle its larger size relative to the per-category files
  already fetched (not measured here, but `Mods.json` covers all mods, not
  just rivens, so likely larger than the equipment category files).
- Resolving *which* of the seven Riven types a given weapon uses (the
  Kitgun-vs-Pistol, Zaw-vs-Melee, Shotgun-vs-Rifle disambiguation) needs
  weapon-level category/tag data beyond the flat `disposition` field — this
  wasn't independently re-derived here beyond confirming WFHelper does it via
  `productCategory`/`holsterCategory`/`compatibilityTags`
  (`rivenData.ts`'s `resolveRivenType`, lines 306-331) and that
  `warframe-items`' own equipment records already carry comparable category
  fields (used elsewhere in this repo for `EquipmentCategory` bucketing, per
  `part_quantities.rs`'s `category_for_file`) — worth confirming those
  specific fields (or an equivalent) exist on `warframe-items`' weapon
  records before implementing, since this note only spot-checked
  `disposition`/`omegaAttenuation` presence, not the full category-tag set.
- A small, unrelated discovery worth flagging for whoever implements this:
  `riven.rs`'s own test fixture
  (`crates/wf-mem/tests/fixtures/riven_inventory.json`, explicitly marked
  `"Synthetic/sanitized"` in its own `_comment` field) uses the curse tag
  `WeaponRecoilMod`. Every real data source checked here — WFCD's
  `riven_tags.json`-equivalent (`Mods.json`'s `upgradeEntries`),
  `warframe-public-export-plus`'s `ExportUpgrades.json`, and
  `calamity-inc/warframe-riven-info`'s `riven_tags.json` — uses
  `WeaponRecoilReductionMod` instead; `WeaponRecoilMod` doesn't appear in any
  of them. This is harmless for `parse_rivens` itself (it doesn't validate
  tags), but a decode implementation should not expect `WeaponRecoilMod` to
  resolve against real base-value tables, and the fixture's synthetic tag
  should probably be corrected to `WeaponRecoilReductionMod` when this work
  is picked up, so the fixture matches what a real riven would carry.
- This research verified field *presence and shape* across four primary
  sources by fetching and reading their actual current content (2026-08-15),
  not by writing or running a decode implementation end-to-end. Before
  shipping, the formula in §3 should be exercised against `riven.rs`'s own
  raw values (e.g. `WeaponCritChanceMod: 823451120` for a disposition-3
  weapon) and checked against a value a player can see in-game, to confirm no
  transcription error survived from source to this note.
