# Spec: riven browse tab (decode, price, and rank Unveiled rivens)

Destination of wayfinder map [issue #94](https://github.com/albrektsson/warframe-lite/issues/94).
Every decision below was resolved on that map; each section links back to the
ticket that made the call, and the ticket is the primary source if this spec
and it ever disagree. This document is the hand-off artifact for whoever
implements the tab — it is not itself code, and nothing here has landed on
`main` (see [ADR-0001](../adr/0001-observe-only-never-touch-game-process.md),
[ADR-0003](../adr/0003-browse-gui-is-read-only.md)).

Domain vocabulary (Riven, Unveiled/Veiled riven, Disposition, Riven type,
Floor/Ceiling price, Verdict) is defined in `CONTEXT.md`'s
["Rivens"](../../CONTEXT.md) subheading — read that first; this spec uses
those terms without re-defining them.

## What ships

A new `wf-browse` tab that:

1. Reads every **Unveiled riven** already surfaced by `parse_rivens`
   (`crates/wf-mem/src/riven.rs`).
2. Decodes each one's raw encoded buff/curse values into a displayable stat
   line (e.g. "+126% Crit Chance"), using per-weapon Disposition and
   per-Riven-type base ranges.
3. Groups Unveiled rivens by **Riven type**. For each group, fetches a live
   warframe.market auction snapshot and computes **Floor price**, **Ceiling
   price**, and a **Verdict** ("likely dissolve/transmute" / "likely keep" /
   "insufficient data").
4. Renders the group headers (Floor/Ceiling/Verdict, stated once per type)
   with each owned copy's decoded stats nested underneath.

The app never dissolves or transmutes anything itself — recommendation only,
per ADR-0001/ADR-0003.

## Out of scope

Ruled out while charting the map; see [issue #94](https://github.com/albrektsson/warframe-lite/issues/94#top)'s
"Out of scope" section for the authoritative list:

- **Veiled rivens** — nothing to price until the player unveils them in-game.
- **Performing the dissolve/transmute action.**
- **Attribute/roll-matched auction valuation** — this spec prices a Riven
  *type* (weapon-level), not a specific owned roll. May become its own future
  map if Floor/Ceiling alone proves insufficient.
- **Community weapon-tier signal** — no source ([overframe.gg](https://overframe.gg)
  or otherwise) clears the "real API, not scraping" bar
  ([issue #97](https://github.com/albrektsson/warframe-lite/issues/97)).

## 1. Decoding: weapon, Disposition, and stats

Raw material: `crates/wf-mem/src/riven.rs`'s `parse_rivens` already extracts,
per Unveiled riven, `weapon_unique_name`, `polarity`, `mastery_req`, `rank`,
`rerolls`, and `buffs`/`curses` as `{tag, value}` pairs where `value` is
DE's raw encoded roll — not yet a percentage.

Per [issue #95](https://github.com/albrektsson/warframe-lite/issues/95),
decoding needs two additional data points, both already available without a
new dependency:

- **Disposition**: WFCD's `warframe-items` (already vendored per
  [ADR-0011](../adr/0011-warframe-items-for-prime-part-build-quantities.md))
  carries `disposition` (1-5 circle count) and `omegaAttenuation` (the exact
  float DE's formula multiplies by) on every weapon record in the
  per-category files (`Primary.json`, `Secondary.json`, `Melee.json`,
  `Archwing.json`, `Arch-Gun.json`) — the same files
  `crates/wf-relic/src/part_quantities.rs` already fetches.
- **Per-Riven-type base stat ranges**: the same `warframe-items` dataset's
  `Mods.json`, in its seven `*RandomModRare` entries (not currently fetched
  by this repo — same host/dataset as what's already vendored).

The raw-`Value`-to-percentage decode formula itself is fully specified and
cross-confirmed against two independent primary sources (WFHelper's
`rivenFingerprint.ts` and `calamity-inc/warframe-riven-info`'s
`RivenParser.js`) — full formula, constants, and worked examples are in
`docs/research/riven-disposition-and-stat-decoding.md`. That research doc is
the implementation reference for this step; this spec does not restate the
formula to avoid the two drifting out of sync (see the map's "Not yet
specified" — translating that formula into code is a drafting/implementation
step, not an open decision).

**Known fixture bug to fix when `riven.rs` is next touched** (noted in the
map's Notes, not part of this feature): its test fixture uses curse tag
`WeaponRecoilMod`, which doesn't match any real data source —
`WeaponRecoilReductionMod` is correct.

## 2. Pricing: warframe.market riven auctions

Per [issue #96](https://github.com/albrektsson/warframe-lite/issues/96),
riven auctions are a **separate API surface** from `crates/wf-data/src/market.rs`'s
existing `MarketClient` (which hits `/v2/orders/item/{slug}` for Prime Part
orders) — this needs a new client path, not an extension of that one:

- **Weapon slug catalog**: `GET /v2/riven/weapons` — *not* `/v2/items`. A
  Riven type's slug can differ from its weapon's regular item slug (confirmed
  live: plain `"toxocyst"` doesn't exist in this catalog at all, only
  `"dual_toxocyst"` does — CONTEXT.md's Riven-type example already reflects
  this).
- **Listings**: `GET /v1/auctions/search?type=riven&weapon_url_name={slug}`
  (v1 — never migrated to v2, and still works today even though v1 `/orders`
  now 403s). Up to 500 listings per call, hard-capped, no working pagination.
- Each listing carries: `is_direct_sell` (`true` = fixed buyout via
  `buyout_price`, `false` = real bidding auction with an optional `top_bid`),
  full rolled stats, `owner.status` (`online`/`ingame`/`offline`), and an
  `updated` timestamp.
- **No historical/completed-sale data exists for rivens anywhere in the
  API** — unlike Prime Parts, which have a real `/v1/items/{slug}/statistics`
  endpoint. Floor/Ceiling must be computed from the live-listing snapshot
  alone (see §3). A live pull for `dual_toxocyst` surfaced a real
  888,888-plat joke listing — outlier trimming is not a theoretical concern.

Full endpoint detail, live example payloads, and everything tested:
`docs/research/warframe-market-riven-pricing-api.md`.

## 3. Floor / Ceiling / Verdict algorithm

Settled across [issue #98](https://github.com/albrektsson/warframe-lite/issues/98),
amended by [issue #103](https://github.com/albrektsson/warframe-lite/issues/103),
and the Verdict threshold from [issue #102](https://github.com/albrektsson/warframe-lite/issues/102).
This is the full algorithm, computed **per Riven type** (a group-level fact,
not per owned copy — see §4 and CONTEXT.md's Verdict entry):

### 3.1 Listing filter (superseded by #103)

Keep a listing only if:

```
updated within the last RECENCY_WINDOW_DAYS days   // placeholder: 30
```

`owner.status` (online/ingame/offline) plays **no role** in this filter —
this is the one place this spec diverges from `market.rs`'s `is_active`
filter (`visible && status in {ingame, online}`), which #98 initially copied
and #103 walked back. Rationale: seller reachability answers "can I trade
with them right now," not "is this price stale" — those are different
questions, and filtering on status starved/skewed the set during off-peak
hours (real sampled data: median listing age 22-29 days; even a 1-day window
would leave niche weapons with 2-3 listings). `owner.status` does not surface
anywhere else in this spec (no "N sellers online now" indicator) — flagged in
#103 as fog for a possible future ticket, not decided here.

`RECENCY_WINDOW_DAYS = 30` is a placeholder named constant, same spirit as
the other two below — not calibrated against real distribution data.

### 3.2 Price extraction

For each listing surviving §3.1:

- `is_direct_sell == true` → price = `buyout_price`.
- `is_direct_sell == false` → price = `top_bid`, **only if `top_bid` is
  present**. A bidding listing with no bid yet carries zero price signal and
  is dropped entirely — it does not count toward the minimum-listing
  threshold in §3.3 either.

### 3.3 Aggregation

Over the resulting price set for the Riven type:

- **Floor** = 10th percentile.
- **Ceiling** = 90th percentile.
- Both computed with standard linear interpolation between ranks (NumPy-default
  style), not nearest-rank — so small samples don't collapse to exactly
  min/max.
- **`MIN_LISTINGS = 5`** (placeholder named constant): checked against the
  *price-bearing* set from §3.2 (post-filter, post-extraction — not raw
  listing count).
  - Below it: **Floor is hard-gated** — the Verdict abstains with an
    explicit "insufficient data" state instead of guessing. Floor still
    displays its (caveated) computed number alongside the abstained Verdict.
  - **Ceiling is never gated** — it always computes, down to a single
    listing, but renders visually flagged low-confidence below the same
    `MIN_LISTINGS` line. Ceiling is informational (upside), not the
    load-bearing number the Verdict depends on, so thin data flags it rather
    than suppressing it.

### 3.4 Verdict

Strictly binary, plus the abstain state above — no "marginal"/uncertain
middle state:

```
if price-bearing set size < MIN_LISTINGS:
    Verdict = "insufficient data"
elif Floor < WORTHLESS_THRESHOLD_PLAT:      // placeholder: 15
    Verdict = "likely dissolve/transmute"
else:
    Verdict = "likely keep"
```

`WORTHLESS_THRESHOLD_PLAT` is a **fixed absolute plat value on Floor price**
— explicitly *not* scaled to the weapon's own Ceiling or Median, and not
user-configurable. A relative-ratio threshold was tried first and rejected:
sampled across four weapons of very different real value (`dual_toxocyst`,
`miter`, `brakk`, `kulstar`), Floor/Median sat in a near-constant 40-62% band
regardless of the weapon's actual worth — that consistency disqualifies a
ratio approach, since it wouldn't discriminate a worthless weapon from a
valuable one. Because the Verdict is a Riven-type-level fact (§4), "worthless"
can only mean "not worth real plat" — an inherently absolute question.

All three placeholder constants (`RECENCY_WINDOW_DAYS = 30`, `MIN_LISTINGS =
5`, `WORTHLESS_THRESHOLD_PLAT = 15`) are explicitly uncalibrated — real
threshold tuning needs distribution data this map didn't collect. Name them
as named constants, not magic numbers, so they're easy to find and tune
later.

The Verdict is always rendered alongside the raw Floor/Ceiling numbers it's
derived from — never a standalone badge (mirrors the existing Ducat
efficiency view).

## 4. Layout: grouped by Riven type

Settled in [issue #99](https://github.com/albrektsson/warframe-lite/issues/99)
via `/prototype`, picked live against a running build (three variants — flat
table, one-card-per-riven, grouped — over seven mock rivens covering an
insufficient-data case and a low-confidence-Ceiling case).

**Variant C — grouped by Riven type — won.** Rivens sharing a Riven type nest
under one (collapsing) group header stating Floor, Ceiling, and Verdict
*once*, since those three are Riven-type-level facts (CONTEXT.md), not
per-copy ones. Each owned copy renders underneath showing only what's
actually per-copy: its decoded stat line, polarity, mastery requirement,
rank, and rerolls.

This avoids the flat-table and per-card variants' repetition problem, where
identical price/verdict data would be restated on every owned copy of the
same type — visible in the prototype's three-Soma-Prime-copies mock case.

Reference-only prototype code (not on `main`, per this map's destination
being a spec): branch `prototype/riven-tab-layout-99`,
`crates/wf-browse/src/riven_prototype.rs`.

## 5. Nav placement: own top-level group

Settled in [issue #101](https://github.com/albrektsson/warframe-lite/issues/101).
The tab gets its own top-level `Group`, following `crates/wf-browse/src/lib.rs`'s
existing "groups split by what they act on" rule (see the doc comments on
`Tab`/`Group` in that file):

- `Group::Relics` is every owned-*relic* action (crack, sell, farm, catalogue
  reference) — rivens never get those actions (read-only, per
  ADR-0001/ADR-0003), so folding in would stretch the group past its
  item-type axis.
- `Group::Ducats` is specifically the one post-crack Prime-Part action. A
  riven isn't a Prime Part — the surface similarity (both are single-tab
  "is this worth anything" valuation views) is a UI-pattern match, not the
  item-type match the existing taxonomy splits on.

Confirms the #99 prototype's placeholder as the real decision. Positioned
between `Relics` and `Ducats` (owned-relic economy → riven economy →
prime-part economy):

```rust
const GROUPS: [Group; 5] =
    [Group::Home, Group::Progress, Group::Relics, Group::Rivens, Group::Ducats];

// Group::Rivens: single tab, no children — same shape as Group::Ducats.
```

Exact `Tab` variant naming and label text are left to the implementation
step, not part of this nav-placement decision.

## 6. Persistence and caching

Per the map's Notes, following existing patterns rather than inventing new
ones:

- **Riven identity/decoded stats** (weapon, polarity, mastery req, rank,
  rerolls, decoded buff/curse lines) persist to disk the same way
  `crates/wf-mem/src/persist.rs` already persists owned-relic/Prime-Part
  state — a new `rivens.json`-shaped file, written via the same
  decode+snapshot+apply+save pattern as `write_owned_relics`/
  `write_owned_parts`.
- **Price/Verdict data stays live-only** — never persisted to the owned-state
  file. It goes through `wf-cache`'s `KeyedCache` (`crates/wf-cache/src/lib.rs`),
  the same stale-serves-instantly pattern already used for Prime Part prices
  (`wf_relic::cached_plat` / `wf_relic::PriceCache`, wired through
  `fetch_prices` in `crates/wf-browse/src/lib.rs`) — keyed per Riven type.

### Request volume

The DOS-politeness risk here is **request count, not payload size**: one
`/v1/auctions/search` call per distinct Riven type the player has Unveiled
rivens for (plausibly dozens), against a single community-run API with no
documented rate limit. Each individual response (up to 500 small JSON
listings) is not itself a concern.

Mitigate with the pattern already proven for relic/Prime Part prices in
`crates/wf-browse/src/lib.rs`'s `fetch_prices`:

- A TTL-gated `KeyedCache` entry per Riven type — skip the network entirely
  once fresh.
- A bounded concurrency cap via `buffer_unordered(PRICE_FETCH_CONCURRENCY)`
  (currently `8`), not per-request payload optimization — payload size isn't
  the actual risk.

See also [issue #100](https://github.com/albrektsson/warframe-lite/issues/100),
which hardens the app's other external-API call sites (Fissure polling)
against the same class of concern — apply the same spirit here, not
necessarily the same code.

## Implementation-step details (not decided by this map)

Explicitly left open, per the map's "Not yet specified" — these are drafting/
implementation details, not open decisions:

- Translating the confirmed decode formula
  (`docs/research/riven-disposition-and-stat-decoding.md`) into actual
  `wf-mem`/`wf-relic` code, including exact per-stat-tag mapping through
  `omegaAttenuation` and `Mods.json`'s base ranges.
- Exact `Tab` variant name and label text for the new tab (§5).
- Exact `rivens.json` field/struct shapes (§6) — follow
  `crates/wf-mem/src/persist.rs`'s existing `RelicsWriteReport`-style pattern.

## References

- Map: [issue #94](https://github.com/albrektsson/warframe-lite/issues/94)
- Tickets: [#95](https://github.com/albrektsson/warframe-lite/issues/95) (decode data source),
  [#96](https://github.com/albrektsson/warframe-lite/issues/96) (pricing API shape),
  [#97](https://github.com/albrektsson/warframe-lite/issues/97) (no community tier API),
  [#98](https://github.com/albrektsson/warframe-lite/issues/98) (Floor/Ceiling algorithm),
  [#99](https://github.com/albrektsson/warframe-lite/issues/99) (layout),
  [#101](https://github.com/albrektsson/warframe-lite/issues/101) (nav placement),
  [#102](https://github.com/albrektsson/warframe-lite/issues/102) (Verdict threshold),
  [#103](https://github.com/albrektsson/warframe-lite/issues/103) (recency filter amendment)
- Research: `docs/research/riven-disposition-and-stat-decoding.md`,
  `docs/research/warframe-market-riven-pricing-api.md`,
  `docs/research/riven-tier-ranking-sources.md`
- ADRs: [ADR-0001](../adr/0001-observe-only-never-touch-game-process.md) (observe-only),
  [ADR-0003](../adr/0003-browse-gui-is-read-only.md) /
  [ADR-0004](../adr/0004-wishlist-write-is-a-narrow-exception-to-adr-0003.md)
  (`wf-browse` is read-only), [ADR-0011](../adr/0011-warframe-items-for-prime-part-build-quantities.md)
  (`warframe-items` vendoring), [ADR-0013](../adr/0013-token-relay-session-nonce-is-not-a-credential.md)
  (token-relay nonce)
