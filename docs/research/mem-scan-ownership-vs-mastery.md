# Does mem-scan equipment ownership data add anything `MasterySet` doesn't already have?

Research for [issue #61](https://github.com/albrektsson/warframe-lite/issues/61),
child of the wayfinder map [issue #55](https://github.com/albrektsson/warframe-lite/issues/55).
`docs/research/mobile-inventory-api-coverage.md` already found the mobile
`inventory.php` endpoint's raw equipment arrays (`Suits`, `LongGuns`,
`Pistols`, `Melee`, `Sentinels`, `SentinelWeapons`, `SpaceSuits`, `SpaceGuns`,
`SpaceMelee`, `OperatorAmps`, `MechSuits`) "redundant with, not a replacement
for" the existing mastery source, `crates/wf-relic/src/mastery.rs`'s
`MasterySet`, for **mastered** items — same information, two angles. What
that note left open: whether the public profile API's `XPInfo` array (the
source `MasterySet` actually reads) also covers items the player **owns but
hasn't put any affinity into yet** — a just-crafted, still-rank-0 Prime — or
whether `XPInfo` only lists items with `XP > 0`, making a rank-0 owned Prime
invisible to `MasterySet` and leaving the raw equipment arrays as the only
source that can answer "do I own this at all."

## Question

Does `XPInfo` include zero-affinity owned items, or only items with
`XP > 0`? And separately: does the public profile API's `LoadOutInventory`
expose full equipment ownership at all, by any field?

## Answer, in short

**Confirmed, from two independent real-account payloads and one independent
third-party parser's explicit design decision: `XPInfo` only lists items
with `XP > 0`, and it is not even a reliable ownership signal for items that
*do* have XP — WFHelper's own mastery-merge logic explicitly does not treat
an `XPInfo` entry as proof of current ownership, because affinity survives
selling the item.** A rank-0, freshly-crafted Prime is genuinely invisible
to `XPInfo`/`MasterySet`. Separately and just as decisively: the public
profile API's `LoadOutInventory` doesn't expose a full owned-equipment list
under *any* field — `Suits`/`Melee`/etc. there are the player's **currently
equipped loadout only** (one entry per slot), not the full inventory the
mobile `inventory.php` endpoint's same-named fields provide. So the raw
`mem-scan` equipment arrays are the only source in this app's reach that can
answer "do I own item X," independent of both affinity progress and current
loadout selection. **This is a genuinely new capability, not a redundant
second path — recommendation at the end.**

## 1. Two real captured `getProfileViewingData.php` payloads: zero `XP == 0` entries across 1,226 real `XPInfo` records

[`WFCD/profile-parser`](https://github.com/WFCD/profile-parser) — the
Warframe Community Developers org's own parser for this exact endpoint — ships
two **real, live-captured** player payloads as test fixtures (not synthetic):
[`test/data/Tobiah.json`](https://raw.githubusercontent.com/WFCD/profile-parser/main/test/data/Tobiah.json)
and
[`test/data/OrnsteinTheSlayer.json`](https://raw.githubusercontent.com/WFCD/profile-parser/main/test/data/OrnsteinTheSlayer.json)
(both fetched directly, 2026-08-11). These are exactly the shape
`mastery.rs` parses (`Results[0].LoadOutInventory.XPInfo`, `{ItemType, XP}`
pairs) — real DE responses, not a reconstruction from a consumer's model.

Parsed directly:

| Fixture | `XPInfo` entries | Entries with `XP == 0` | Lowest `XP` seen |
|---|---|---|---|
| `Tobiah.json` | 702 | **0** | 450,005 (a min-maxed account — every entry already above the weapon mastery cap) |
| `OrnsteinTheSlayer.json` | 524 | **0** | 138,024 (6 entries sit below the 450,000 weapon cap, so partial-affinity items *do* appear) |

Two findings from this, together:

- **Partial-affinity items are present** (Ornstein's account has 6 entries
  between 138,024 and 407,301 XP, all below the weapon cap — confirms
  `XPInfo` is not "mastered items only," it does track in-progress affinity,
  consistent with `mastery.rs`'s own doc comment).
- **Zero-affinity items are absent.** Across both accounts' combined 1,226
  entries, not one carries `XP == 0` (nor is the `XP` field ever missing —
  every entry has it). If DE listed every owned item in `XPInfo` regardless
  of affinity, a large PC account like Tobiah's (702 mastered/near-mastered
  entries) would be expected to also carry unranked, freshly-acquired, or
  never-equipped items at 0 XP — plausible for accounts with hundreds of
  Prime parts sitting built-but-unused. None appear in either sample.

This is inference from absence across two samples, not a documented DE
guarantee — see Caveats — but it directly matches
[`WFCD/profile-parser`'s own `XpInfo.ts`](https://raw.githubusercontent.com/WFCD/profile-parser/main/src/XpInfo.ts)
doc comment, written by the same org that captured these fixtures:

```ts
/**
 * An item that has contributed to a player's mastery rank
 * @module
 */
export default class XpInfo {
```

"Contributed" — not "owned" — is the operative word DE's own community
maintainers chose.
[`LoadOutInventory.ts`](https://raw.githubusercontent.com/WFCD/profile-parser/main/src/LoadOutInventory.ts)
phrases the parent field the same way: `xpInfo: XpInfo[]` is documented as
"Items that have **counted towards** the players mastery rank" — an item
that hasn't been played yet has counted toward nothing.

## 2. `LoadOutInventory`'s other fields are the *equipped loadout*, not the inventory

A second, independent finding from the same fixtures, relevant regardless of
the `XP == 0` question: `Tobiah.json`'s `LoadOutInventory` object has exactly
four top-level keys — `WeaponSkins`, `Suits`, `Melee`, `XPInfo` — and its
`Suits` array has **one entry** (the equipped Warframe, Trapper/Hildryn
Prime, with full config/polarity/skin data — clearly "this player's current
loadout slot," not "every Warframe this player owns"). `Pistols` and
`LongGuns` aren't even present (this account's loadout preset has no
secondary/primary configured). This matches
[`WFCD/profile-parser`'s `RawLoadOut` interface](https://raw.githubusercontent.com/WFCD/profile-parser/main/src/LoadOutInventory.ts)
declaring `Suits: RawLoadOutItem[]` non-optional but `Pistols?`/`LongGuns?`/
`Melee?` optional — consistent with "whatever's currently equipped," not a
full-category inventory dump. So even setting the affinity question aside,
the public profile API has **no field, anywhere**, that lists full owned
equipment the way the mobile `inventory.php` endpoint's identically-named
`Suits`/`LongGuns`/etc. arrays do (per
`docs/research/mobile-inventory-api-coverage.md` §2 — those are `ItemCount`-
bearing true inventory entries, not a 1-slot loadout snapshot).

## 3. Independent corroboration: WFHelper explicitly does not treat `XPInfo` as an ownership signal

[`WFHelper/WFHelper`'s `services/masteryHelper.ts`](https://raw.githubusercontent.com/WFHelper/WFHelper/main/services/masteryHelper.ts)
(fetched directly, 2026-08-11) is a second, independent, real-world consumer
of this same data — and its mastery-merge logic settles the question a third
way, more strongly than the payload sampling above. It builds an
`ownedMap: Map<uniqueName, OwnedMasteryRecord>` in two passes:

```ts
for (const [invKey, maxRank] of Object.entries(INV_CATEGORIES)) {
  const arr = inventoryData[invKey];
  if (!Array.isArray(arr)) continue;
  ...
  for (const entry of arr as InventoryMasteryEntry[]) {
    const record = readOwnedMasteryRecord(entry, maxRank as number, true, affinityPerRankSquared);
    // owned = true
```

`INV_CATEGORIES` here is exactly the mobile `inventory.php` equipment-array
key set (`Suits`, `LongGuns`, `Pistols`, `Melee`, `Sentinels`,
`SentinelWeapons`, `SpaceSuits`, `SpaceGuns`, `SpaceMelee`, `OperatorAmps`,
`MechSuits`, plus a few pet/hoverboard categories) — and every record built
from one of these arrays is marked `owned: true`. Then, separately:

```ts
// XPInfo: items sold but XP still counts
const xpInfo = inventoryData.XPInfo;
if (Array.isArray(xpInfo)) {
  for (const entry of xpInfo as InventoryMasteryEntry[]) {
    ...
    const record = readOwnedMasteryRecord(
      entry, existing?.maxRank ?? MAX_ITEM_RANK, false, ...
      //                                          ^^^^^ owned = false
```

and inside `readOwnedMasteryRecord`:

```ts
if (!owned) record.fromXPInfo = true;
```

WFHelper's own inline comment states the reason explicitly: **"items sold
but XP still counts."** This is a stronger claim than the payload-sampling
evidence above — it says `XPInfo` isn't just missing zero-affinity owned
items, it can also carry entries for items **no longer owned at all**
(affinity is permanent once earned, per `mastery.rs`'s own comment about the
rank-30 cap "never reset[ting], even on Forma" — the same permanence applies
even after the item itself is sold or traded away). WFHelper's design
treats `XPInfo`-only records as a `fromXPInfo: true` fallback, explicitly
distinct from and lower-confidence than an entry backed by a real inventory
array — the same distinction this ticket is asking about.

## 4. What this means for `mastery.rs`'s current design

None of the above is a defect in `mastery.rs` as it stands —
`crates/wf-relic/src/mastery.rs:1-12` scopes itself correctly to *mastery*
("mastered once its lifetime affinity reaches the rank-30 cap"), not to
ownership, and `XPInfo` is the right, sufficient source for that narrower
question (a mastered item, by definition, has `XP` far above 0, so it always
appears). The gap is specifically at the "have I built this Prime at all"
layer one step before mastery, which `mastery.rs` was never asked to answer
and structurally cannot answer from its current data source — not even in
principle, since the public profile API has no full-inventory field to fall
back to (§2).

## Caveats and gaps

- The "no `XP == 0` entries" finding (§1) is drawn from **two** real
  accounts, not a DE-documented guarantee. It's possible some other account
  shape (e.g. a brand-new player, or an item type DE tracks differently)
  produces a 0-XP `XPInfo` entry that neither sample happened to contain.
  The finding should be read as "strong, convergent, real-payload evidence
  against zero-XP entries, corroborated by an independent parser's explicit
  design choice (§3)," not as a DE API contract.
- No DE-authored documentation of `getProfileViewingData.php` was found (as
  `docs/research/mobile-inventory-api-coverage.md` also noted for
  `inventory.php`) — every claim here is from real captured payloads plus
  two independent open-source consumers' source code and design decisions,
  not DE's own word.
- This app's own live POC data (issue #52) isn't usable to double-check this
  specific question: per this repo's standing rule, the real captured
  `inventory.php` payload from #52 was never committed (it contains live
  account data), and in any case #52 captured the **mobile** endpoint, not
  the **public profile** endpoint `mastery.rs` uses — the two were never
  cross-referenced against each other on the same account. A future live
  check that (a) crafts a fresh Prime part to rank 0, (b) fetches both
  endpoints for the same account, and (c) confirms the item is present in
  `inventory.php`'s `Recipes`/equipment arrays but absent from
  `getProfileViewingData.php`'s `XPInfo` would upgrade this from "strong
  convergent evidence" to "directly confirmed" — worth doing opportunistically
  if a follow-up ticket does further live verification, but not blocking.

## Direct answer

**Yes — raw equipment-array ownership is a genuinely new capability, not a
redundant second path to data `MasterySet` already has.** Two independent,
compounding reasons: (1) `XPInfo` only lists items with `XP > 0` (§1, real
payloads) and isn't even a reliable *current*-ownership signal for items it
does list, since sold items keep their XP (§3, WFHelper's explicit design);
(2) the public profile API has no full-inventory field at all to fall back
on — `LoadOutInventory`'s equipment fields are the equipped loadout, not the
owned catalogue (§2). A freshly-built, still-unplayed Prime is invisible to
this app's current mastery pipeline end to end, and no amount of
reinterpreting `XPInfo` differently can fix that — the data simply isn't
there.

**Recommendation: "Prime/weapon ownership wiring" should graduate from fog
into a real ticket.** What it should expose: **raw ownership, not
ownership+XP and not a `MasterySet` replacement.** Concretely:
- A new, separate `mem-scan` output — e.g. an `OwnedEquipment` set built from
  the raw `Suits`/`LongGuns`/`Pistols`/`Melee`/`Sentinels`/
  `SentinelWeapons`/`SpaceSuits`/`SpaceGuns`/`SpaceMelee`/`OperatorAmps`/
  `MechSuits` arrays' `ItemType` presence, following `parse_foundry`/
  `parse_rivens`/`parse_level_keys`'s established convention (pure,
  network-agnostic parse functions per issue #55's map, one per category or
  one combined; raw exposure, no interpretation) — answering exactly "is
  this Prime/weapon in my inventory," independent of affinity.
- Per-item `XP` on these same entries is already redundant with `MasterySet`
  (per `mobile-inventory-api-coverage.md` §6) and shouldn't be re-surfaced
  as a second mastery source — `MasterySet` stays the one mastery source;
  the new ownership set stays scoped to presence/absence only, keeping the
  two concerns (owned vs. mastered) cleanly separate rather than building a
  second, overlapping mastery pipeline.
- Natural pairing: cross-referencing ownership against `MasterySet` would let
  the app show "built but not yet mastered" — a state currently invisible —
  which is plausibly the actual product motivation behind this fog item in
  the first place.
