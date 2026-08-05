# Equipment category taxonomy: warframe.market tags vs WFCD `warframe-items`

Research for [issue #42](https://github.com/albrektsson/warframe-lite/issues/42),
scoping an Equipment window matching WFinfo's six groupings (Warframe /
Primary / Secondary / Melee / Archwing / Companion — with Companion further
split into Sentinels, Sentinel weapons, Kubrows, Kavats, and Moas) ahead of
reworking `crates/wf-browse/src/main.rs`'s flat `mastery_tab`.

## Question

Does warframe.market v2 `/v2/items`'s `tags` field already carry a clean,
complete equipment category matching WFinfo's six groupings? If not, does
WFCD's `warframe-items` dataset (already fetched by
`crates/wf-relic/src/part_quantities.rs` per ADR-0011) fill the gap?

## Answer, in short

**No — warframe.market's tags cleanly cover Warframe / Primary / Secondary /
Melee / Archwing, but break down for Companion.** Three of Companion's five
WFinfo sub-groups have partial or zero coverage: classic Sentinel weapons
(Deconstructor, Sweeper, Artax, …) aren't in the catalogue at all (not
tradable), live Kubrow/Kavat/Moa pets aren't in the catalogue at all (bred,
not built/traded), and a companion's own tradable weapon (Hound
brackets/stabilizers) is mistagged as a plain `secondary` weapon with no tag
distinguishing it from a real Secondary. **WFCD's `warframe-items` fills every
one of these gaps** via its per-category JSON files, which the repo already
fetches a subset of. Recommendation: **use WFCD as the primary source of
category** (keyed by which file/record `category` an item came from), with
warframe.market's tags kept only as a supplementary cross-check for the four
categories where they're already clean (Warframe/Primary/Secondary/Melee) —
not for Companion or Archwing.

## Source 1: warframe.market v2 `/v2/items`

Fetched live (2026-08-05) via `curl -s -H "Language: en"
https://api.warframe.market/v2/items` — the exact request shape
`crates/wf-data/src/items.rs::fetch_items()` sends. 3,837 items returned.

### What a clean whole-item tag combo looks like

Tags line up well **at the whole-item level** — the `_set` item and the
top-level `_blueprint` item for a build — for four of the six categories:

| WFinfo category | warframe.market tag combo (on `_set`/root `_blueprint`, excluding `mod`) | Example |
|---|---|---|
| Warframe | `warframe` + NOT `mod` | `ash_prime_set`: `["set","prime","warframe"]` |
| Primary | `primary` + `weapon` | `lex_prime_blueprint`... no — `zhuge_prime_set`: `["primary","prime","weapon","set"]` |
| Secondary | `secondary` + `weapon` | `lex_prime_set`: `["weapon","prime","set","secondary"]` |
| Melee | `melee` + `weapon` | `bo_prime_set`: `["weapon","prime","set","melee"]` |
| Archwing | `archwing` (suits have no `weapon` tag; archgun/archmelee weapons add `weapon`) | `itzal_set`: `["set","archwing"]`; `kaszas_blade`: `["component","weapon","archwing"]` |

The `mod` exclusion matters: **the category tags (`warframe`, `primary`,
`secondary`, `melee`, `archwing`, `sentinel`, `kubrow`, `kavat`, `moa`) are
reused on mod items**, not just gear. E.g. `adaptation` (a Warframe mod) is
tagged `["mod","warframe","rare"]` — same `warframe` tag as an actual frame's
blueprint. Any category filter must exclude `mod` first.

**Individual weapon component parts drop the category tag.** `braton_prime_barrel`,
`lex_prime_barrel`, `bo_prime_handle`, `aklex_prime_link` all carry only
`["component","weapon","prime"]` — no `primary`/`secondary`/`melee`. Only the
`_set` and root `_blueprint` items keep the specific tag. **Warframe
components are the exception** — `zephyr_prime_chassis_blueprint` keeps
`["component","prime","warframe","blueprint"]`, `warframe` tag intact. So:
category-tag lookups must run against the whole item (`_set`/root
`_blueprint`), never against a bare component, for weapons — this is exactly
how the repo's existing `PartQuantities` already groups by `prime` name, so
it's not a new problem, just worth flagging for anyone tempted to tag
components directly.

Full observed tag vocabulary (top of the frequency list from all 3,837
items, `jq -r '.data[].tags[]' | sort | uniq -c | sort -rn`):

```
1392 mod        767 weapon      428 primary     288 melee       230 set
 919 rare       744 prime       388 blueprint   226 secondary   208 augment
 604 warframe   608 component   356 uncommon    140 archwing    111 sentinel
  96 pistol      87 shotgun      85 stance       75 rifle        75 legendary
  52 necramech   36 archgun      33 kubrow       26 companion    22 k_drive
  21 hound       20 archmelee    19 kavat        15 pet          8 moa
```

### Where it breaks down: Companion

- **Sentinels (the pet itself)**: clean — `sentinel` + NOT `mod`, at the
  `_set`/root `_blueprint` level (e.g. `carrier_prime_set`:
  `["sentinel","prime","set"]`). Sentinel build **components** drop the tag
  just like weapon components do: `carrier_prime_carapace` is tagged only
  `["component","weapon","prime"]` — misleadingly `weapon`, not `sentinel`.
- **Sentinel weapons** (Deconstructor, Sweeper, Artax, Cryotra, Prisma Burst
  Laser, Prisma Dual Decurion, …): **absent from the catalogue entirely.**
  Searched by name and slug — zero hits. These are Credit-market purchases,
  not farmed/tradable, so warframe.market simply never lists them.
- **Kubrows / Kavats / Moas as pets**: **absent from the catalogue entirely**
  — bred via Incubator, never built or traded, so there's no tradable item to
  tag. The `kubrow`/`kavat`/`moa` tags that *do* exist in the vocabulary are
  applied only to **precept/link mod** items (e.g. `dig`:
  `["mod","rare","kubrow","sahasa_kubrow"]`, `whiplash_mine`:
  `["mod","common","sentinel","moa"]`) — never to a pet item, because there
  isn't one.
- **A companion's own tradable weapon** (Hound brackets/stabilizers — Cela
  Bracket, Wanz Stabilizer, Zubb Bracket, Hec Hound): these ARE tradable and
  present, but tagged `["blueprint","secondary","hound"]` /
  `["secondary","hound"]` — i.e. warframe.market files them under the same
  `secondary` tag as a real Secondary weapon. Nothing about the tag set
  marks it as companion equipment except also having the `hound` tag, which
  a naive "match on `secondary`" filter would ignore.

**Net effect**: filtering warframe.market's tags for `sentinel` recovers only
the Sentinel-pet sub-bucket. Sentinel weapons, Kubrows, Kavats, and Moas are
either missing outright or bleed into other categories. Companion is not
resolvable from this source alone.

## Source 2: WFCD `warframe-items`

Fetched live (2026-08-05) from the same `raw.githubusercontent.com` base
`part_quantities.rs::BASE` already uses:
`https://raw.githubusercontent.com/WFCD/warframe-items/master/data/json/`.
Directory listing (`api.github.com/repos/WFCD/warframe-items/contents/data/json`)
confirms one JSON file per equipment category, plus `Pets.json`,
`Arcanes.json`, `Mods.json`, etc. — a much finer split than the 8 files
`CATEGORIES` in `part_quantities.rs` currently fetches (`Warframes`,
`Primary`, `Secondary`, `Melee`, `Sentinels`, `SentinelWeapons`, `Archwing`,
`Arch-Gun`) — note `Pets.json` and `Arch-Melee.json` are **not** in that
const today.

### Per-file `category` field is clean — with one quirk

Every record carries `type` and `category` fields. Sampling each relevant
file:

| File | in-record `category` | in-record `type` (sample) | Notes |
|---|---|---|---|
| `Warframes.json` | `"Warframes"` | `"Warframe"` | clean |
| `Primary.json` | `"Primary"` | `"Rifle"`, etc. | clean |
| `Secondary.json` | `"Secondary"` | `"Pistol"`, `"Dual Pistols"`, `"Throwing"` | clean |
| `Melee.json` | `"Melee"` | `"Melee"` | clean |
| `Archwing.json` | `"Archwing"` | `"Archwing"` | clean (suits) |
| `Arch-Gun.json` | `"Arch-Gun"` | `"Arch-Gun"` | clean |
| `Arch-Melee.json` | `"Arch-Melee"` | `"Arch-Melee"` | clean |
| `Sentinels.json` | `"Sentinels"` | `"Sentinel"` | clean |
| `SentinelWeapons.json` | **`"Primary"`** (bug/quirk) | `"Companion Weapon"` | **don't trust the `category` field for this one file** — trust the file identity or `type` instead |
| `Pets.json` | `"Pets"` uniformly | `"Pets"` / `"Pet Resource"` / `"Pet Parts"` | one bucket, no breed split in `category`/`type` |

So: **the file an item comes from is the reliable category signal** (matching
how `part_quantities.rs` already partitions by `CATEGORIES` file name), not
the in-record `category` string in isolation — `SentinelWeapons.json` proves
the in-record field alone can't be trusted blindly.

`SentinelWeapons.json` (24 entries: Akaten, Artax, Burst Laser, Burst Laser
Prime, Cryotra, …) is exactly the gap warframe.market left open — full
coverage of Sentinel weapons, correctly separated from the pet.

### Pets.json covers Kubrow/Kavat/Moa, but the split is by internal path, not a field

66 total records; 22 have `type: "Pets"` (actual companions, vs. `"Pet
Resource"`/`"Pet Parts"` which are crafting materials/imprint items). All 22
share `category: "Pets"` — no direct `"Kubrow"` vs `"Kavat"` vs `"Moa"` field.
However every record's `uniqueName` (internal DE path) reliably encodes the
breed family as a path segment:

| Breed family | `uniqueName` substring | Examples |
|---|---|---|
| Kubrow | `/KubrowPet/` | Chesa Kubrow, Huras Kubrow, Sunika Kubrow, Sahasa Kubrow, Raksa Kubrow, Helminth Charger |
| Kavat | `/CatbrowPet/` | Smeeta Kavat, Adarza Kavat, Vasca Kavat |
| Moa | `/MoaPets/` | Lambeo Moa, Nychus Moa, Oloro Moa, Para Moa |
| Hound | `/ZanukaPets/` | Bhaira Hound, Dorma Hound, Hec Hound |
| Deimos modular (Vulpaphyla/Predasite) | `/CreaturePets/` | Panzer Vulpaphyla, Sly Vulpaphyla, Crescent Vulpaphyla, Medjay/Pharaoh/Vizier Predasite |

This is a workable, if unofficial, sub-split (string-match on `uniqueName`),
not a documented/stable field — flag it as such if the implementation relies
on it. Vulpaphyla/Predasite don't map onto any of WFinfo's named sub-groups
(Kubrow/Kavat/Moa) at all — see "uncategorizable" below.

`Pets.json` also **correctly keeps the Hound's own weapon parts** (Cela
Bracket, Wanz Stabilizer, Zubb Bracket, Hec Hound, Drimper/Frak/Gauth/Hona/
Jonsin/Tian/Urga Bracket-or-Stabilizer) filed as `type: "Pet Resource"` inside
`Pets.json` — i.e. under Companion, not Secondary — resolving exactly the
ambiguity warframe.market's `secondary`+`hound` tag combo left open.

`productCategory` in `Pets.json` is **not reliable** — many pet-resource
records show `"productCategory": "Pistols"`, an apparent WFCD data-quality
default/bug, not a meaningful value. Don't use it.

## Recommendation

**Use WFCD `warframe-items` as the primary category source**, keyed by which
per-category file (equivalently, a `category`/`type` field trusted per-file
rather than globally) a record came from:

- `Warframes.json` → Warframe
- `Primary.json` → Primary
- `Secondary.json` → Secondary
- `Melee.json` → Melee
- `Archwing.json` + `Arch-Gun.json` + `Arch-Melee.json` → Archwing (one UI bucket, matching WFinfo)
- `Sentinels.json` + `SentinelWeapons.json` + `Pets.json` → Companion (one UI bucket; sub-group Sentinel / Sentinel weapon / Kubrow / Kavat / Moa via file identity for the first two and the `uniqueName` path substring for Pets.json)

This requires widening `part_quantities.rs`'s `CATEGORIES` const (currently
missing `Pets` and `Arch-Melee`) if the same fetch path is reused for
category data, or a parallel fetch — either way, no new dataset/URL, just
more of the one already in use per ADR-0011.

Keep warframe.market's tags as a **supplementary cross-check only** for
Warframe/Primary/Secondary/Melee (where both sources already agree at the
whole-item level) — e.g. to resolve a warframe.market slug to a category
without a second network round-trip when WFCD data is still loading. Do
**not** rely on warframe.market tags for Archwing-vs-suit-vs-weapon splitting
or for any part of Companion; that source has zero coverage of Sentinel
weapons and live Kubrow/Kavat/Moa pets, and actively mistags Hound weapons as
`secondary`.

### Items to route to a fallback/"Other" bucket, not guess at

- **Deimos modular companions** (Vulpaphyla, Predasite) — mechanically
  Kubrow-like/Kavat-like respectively, but not literally either breed family
  WFinfo names; don't force them into Kubrow or Kavat.
- **Necramechs** (Bonewidow, Voidrig, …) — outside WFinfo's six categories
  entirely; both sources tag/categorize them separately (`necramech` tag on
  warframe.market mods; no dedicated WFCD category file was fetched/checked
  here since it's out of scope) — exclude rather than misfile under Warframe.
- Anything present in one source and not the other for a given category
  (e.g. a very new item WFCD hasn't synced yet) should fall back to
  whichever source *does* have it rather than being dropped silently.
