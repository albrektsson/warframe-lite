# What does warframe.market's riven auction API actually return?

Research for [issue #96](https://github.com/albrektsson/warframe-lite/issues/96),
child of the wayfinder map [issue #94](https://github.com/albrektsson/warframe-lite/issues/94).
`crates/wf-data/src/market.rs`'s `MarketClient` only calls the v2 live
order-book endpoint (`GET /v2/orders/item/{slug}`), which prices prime parts
via fixed-price sell/buy orders. Rivens don't trade through that endpoint —
per `CONTEXT.md`'s **Riven type**/**Floor price**/**Ceiling price** entries,
they trade through a separate auction mechanism, and pricing them needs a
Floor/Ceiling computed from listing depth rather than a single outlier
listing.

## Question

What is the actual current warframe.market API endpoint for riven auctions?
How is a **Riven type** (e.g. "Toxocyst" vs. "Dual Toxocyst") identified in
that endpoint? What does each listing carry — price shape (open-auction vs.
buyout-only), rolled stats, seller status, mastery requirement, polarity,
rank, re-rolls? Is there enough listing depth per Riven type to compute a
defensible, outlier-resistant Floor/Ceiling? And critically: does the API
expose any historical/completed-sale data for rivens anywhere, or is
everything limited to the current live-auction snapshot?

## Answer, in short

**There is no official documentation for riven auctions at all** —
`docs.warframe.market`'s sitemap documents Orders, Users, Groups,
Achievements, Auth, Dashboard, Manifests, and WebSockets, but nothing for
Auctions or Rivens (§1). The only endpoint shape ever documented is a
community-maintained, unofficial OpenAPI reconstruction
(`WFCD/market-api-spec`) sourced from a third-party Google Doc, not from DE
or warframe.market themselves (§2).

**Live-tested directly against the real API (2026-08-15, all requests
below are real, unredacted, public GET responses):** riven auctions never
migrated to v2 — they still live under the **v1** base URL, and that
specific v1 surface still works even though v1's *orders* endpoint returns
403 (matching this repo's existing v1-is-dead assumption for prime parts,
which turns out to be endpoint-specific, not API-wide):

```
GET https://api.warframe.market/v1/auctions/search?type=riven&weapon_url_name={slug}
```

A **Riven type** is identified by `weapon_url_name`, a slug from a *separate*
catalog — `GET https://api.warframe.market/v2/riven/weapons` (this one *is*
v2, live, 200 OK, 418 entries) — not the general `/v2/items` catalog
`MarketClient` already uses for prime parts. Confirmed live with the
repo's own example: **"toxocyst" does not exist as a riven-capable weapon at
all** (absent from both `/v2/items` and `/v2/riven/weapons`); only
**"dual_toxocyst"** does. So the CONTEXT.md example pair doesn't actually
hold today — Toxocyst currently has no Riven type on warframe.market, only
Dual Toxocyst does (§3).

Each listing (§4) carries `starting_price`, a nullable `buyout_price`, a
nullable `top_bid`, and an `is_direct_sell` boolean that is the actual
distinguishing field: `is_direct_sell: true` means a fixed "buy now" listing
priced at `buyout_price`; `false` means a real bidding auction, where
`top_bid` (if any bids exist) is the current high bid and `buyout_price` may
optionally still be set as an instant-buy escape hatch or left `null` for an
uncapped auction. The rolled stats are in full fidelity
(`item.attributes[]` with `value`/`positive`/`url_name` per stat,
`item.polarity`, `item.mod_rank`, `item.mastery_level`, `item.re_rolls`), and
seller reachability uses the exact same `online`/`ingame`/`offline` status
vocabulary `MarketClient` already filters prime-part orders on
(`owner.status`).

**Depth is good for popular weapons but hard-capped at 500 listings per
request with no working pagination** — confirmed by testing multiple
weapons: `dual_toxocyst` and `miter` both returned exactly 500 (the ceiling,
so their real total is unknown and *at least* 500), while less-traded
`brakk` (318) and `kulstar` (189) returned their true, lower counts. 500 live
listings is comfortably enough depth to trim outliers and compute a
percentile-based Floor/Ceiling rather than trusting `min()`/`max()` — and the
live `dual_toxocyst` pull below directly demonstrates *why* trusting
`max()` would be wrong (§5).

**Critical finding: there is no historical or completed-sale data for
rivens anywhere in the API, and there structurally can't be one shaped like
the existing prime-part statistics endpoint.** Regular items *do* have real
sale history — `GET /v1/items/{slug}/statistics` is live and returns genuine
closed-order aggregates (`statistics_closed.90days`: daily volume,
min/max/avg/median/weighted-avg price) — but that only works because a prime
part is a fungible SKU. Rivens are not: every roll is unique, rivens are
absent from `/v2/items` entirely, and no analogous statistics endpoint
exists for them under any path or parameter guessed and tested (§6). The app
has no choice but to approximate Floor/Ceiling/confidence purely from the
current live-listing snapshot's depth and seller online-status — exactly the
fallback CONTEXT.md's Floor/Ceiling definitions already anticipate ("weighted
by listing depth ... rather than a single listing taken at face value"),
just with zero historical signal available to cross-check it against.

## 1. Official documentation: confirmed absent

`docs.warframe.market` is the only place warframe.market itself documents
its API (linked from `https://warframe.market/api_docs`, which redirects
there). Its `sitemap.xml`, fetched directly, lists every documented page:

```
/docs/api/achievements  /docs/api/authentication  /docs/api/dashboard
/docs/api/groups        /docs/api/manifests       /docs/api/orders
/docs/api/overview      /docs/api/users           /docs/data-models
/docs/oauth/overview    /docs/rules/overview      /docs/websockets/*
```

No `auctions` or `rivens` page exists in that list. The `/docs/intro` page's
own text says "New API endpoints are added gradually. Some areas are already
documented, while others may still be missing or incomplete" — riven
auctions are one of the missing ones, as of this research. This isn't a
guess about where docs might be; it's every URL the docs site's own sitemap
declares.

## 2. The only documented shape: an unofficial community reconstruction

[`WFCD/market-api-spec`'s `openapi.yaml`](https://github.com/WFCD/market-api-spec/blob/master/openapi.yaml)
(fetched directly) is a hand-maintained OpenAPI 3.0.1 spec, `contact: KycKyc`
(a WFCD/community maintainer, not warframe.market staff), sourced from an
`externalDocs` link to a **Google Doc**, not an official spec feed. It
targets the **v1** base URL (`https://api.warframe.market/{base_url}`,
default `v1`) and documents one auctions path:

```yaml
/auctions:
  get:
    tags: [Rivens]
    summary: Get all current auction items
```

— no search/filter parameters documented on it at all (the real, working
endpoint uses `/auctions/search` with query params, found only by testing —
§3). Its `auction` and `auction_item` schemas do describe the field shapes
that later matched the live response almost exactly (`buyout_price`,
`starting_price`, `top_bid`, `is_direct_sale`/`is_direct_sell`, `closed`,
`attributes[]` with `value`/`positive`/`url_name`, `polarity`, `mod_rank`,
`re_rolls`, `mastery_level`, `weapon_url_name`) — useful as a field-name
cross-check, but explicitly annotated by its own authors as reverse-engineered
in places (e.g. `top_bid: # needs check`, and `is_marked_for`/
`marked_operation_at`: "Couldn't find any meaning for this. No values were
non-null."). Treat it as corroboration, not a source of truth.

## 3. The real, live endpoint and how a Riven type is identified

### 3.1 Auctions never moved to v2 — they're still under v1, and v1 auctions still works

`crates/wf-data/src/market.rs`'s comment states "the legacy v1 endpoint now
returns 403" for prime-part orders. Verified live: `GET
https://api.warframe.market/v1/items/mirage_prime_set/orders` really does
return `403`. But `GET
https://api.warframe.market/v1/auctions/search?type=riven&weapon_url_name=dual_toxocyst`
returns a real **`200`** with a full payload (§4) — v1's death is
endpoint-specific, not API-wide, and riven auctions apparently never got a
v2 replacement. No `/v2/auctions*` path returns anything but `404` (tried
`/v2/auctions/search`, `/v2/riven/auctions`, `/v2/auctions/riven`,
`/v2/auctions/item/{slug}`, `/v2/rivens/auctions`, `/v2/riven/weapons/{slug}/auctions`,
`/v2/riven/auctions/{slug}` — all `404 page not found`, Go-style 404 body
distinct from v1's JSON `{"error": "..."}` 404 shape, confirming they hit a
different backend/router entirely). The correct, working call is:

```
GET https://api.warframe.market/v1/auctions/search?type=riven&weapon_url_name={slug}&platform=pc
```

(`platform` filters correctly — tested and confirmed all returned auctions'
`platform` field matched; `sort_by=price_desc` and `buyout_policy=direct`
also work as real filters, e.g. `buyout_policy=direct` narrowed a mixed
346-direct/154-bidding result set down to 500/500 direct-sell-only.)

### 3.2 Riven type is `weapon_url_name`, from a separate v2 catalog — not `/v2/items`

The weapon slug for a riven search is **not** the same catalog
`MarketClient` already uses for prime parts (`/v2/items`, confirmed live to
hold 3837 tradeable items). It's a dedicated riven-weapon catalog:

```
GET https://api.warframe.market/v2/riven/weapons
```

Live, `200 OK`, 418 entries, e.g.:

```json
{"id":"5c5ca81696e8d2003834fdc1","slug":"dual_toxocyst",
 "gameRef":"/Lotus/Weapons/Infested/Pistols/InfVomitGun/InfVomitGunWep",
 "group":"secondary","rivenType":"pistol","disposition":1.35,
 "reqMasteryRank":11,
 "i18n":{"en":{"name":"Dual Toxocyst", ...}}}
```

(This endpoint also happens to carry `disposition` directly — noted only as
a secondary confirmation; [issue #95's research](riven-disposition-and-stat-decoding.md)
already settled `warframe-items`' `disposition`/`omegaAttenuation` fields as
the actual source this repo should use, decoupled from any live API call.)

Testing this repo's own worked example directly against that 418-entry list:

```python
[it for it in weapons if 'toxocyst' in it['slug']]
# => [{'slug': 'dual_toxocyst', 'rivenType': 'pistol', 'disposition': 1.35, ...}]
# plain "toxocyst" is NOT present
```

and confirmed independently by calling the search endpoint with the wrong
slug:

```
GET /v1/auctions/search?type=riven&weapon_url_name=toxocyst
→ HTTP 400 {"error": {"weapon_url_name": ["app.auctions.errors.item_not_exist"]}}
```

vs. `weapon_url_name=dual_toxocyst` → `HTTP 200` with real listings. **This
corrects the domain assumption in CONTEXT.md's "Riven type" example and
issue #96's own framing**: Toxocyst and Dual Toxocyst are not "two Riven
types worth comparing" today — only Dual Toxocyst currently has a Riven type
on warframe.market at all. (Plain Toxocyst may simply never have had rivens
enabled for it, or may have been removed from the riven-eligible pool at
some point — this research didn't chase why, only confirmed the current
state.) Whatever weapon-list source feeds the app's Riven-type picker should
be `/v2/riven/weapons`, not the general item catalog, to avoid offering
weapons that `weapon_url_name` will reject.

## 4. What a listing carries, and buyout vs. bidding

Full real, unredacted `GET
/v1/auctions/search?type=riven&weapon_url_name=dual_toxocyst` response, one
buyout-only ("direct sell") listing and one open-bidding listing:

```json
{
  "starting_price": 888888, "buyout_price": 888888, "minimal_reputation": 0,
  "visible": true, "platform": "pc", "crossplay": true, "closed": false,
  "top_bid": null, "created": "2025-05-24T13:31:18.000+00:00",
  "updated": "2025-08-27T16:26:15.000+00:00", "is_direct_sell": true,
  "id": "6831ca26ddb959000825f9f2",
  "owner": {
    "reputation": 5999, "ingame_name": "Forever_Prime", "status": "offline",
    "last_seen": "2026-08-14T15:34:27.605+00:00", "region": "en", ...
  },
  "item": {
    "weapon_url_name": "dual_toxocyst",
    "attributes": [
      {"value": 161.7, "positive": true,  "url_name": "multishot"},
      {"value": 194.3, "positive": true,  "url_name": "critical_damage"},
      {"value": 105.8, "positive": true,  "url_name": "critical_chance"},
      {"value": -124.7, "positive": false, "url_name": "puncture_damage"}
    ],
    "polarity": "vazarin", "mastery_level": 14, "re_rolls": 24,
    "mod_rank": 8, "type": "riven", "name": "sati-critatis"
  },
  "private": false
}
```

```json
{
  "starting_price": 20000, "buyout_price": null, "minimal_reputation": 0,
  "visible": true, "platform": "pc", "crossplay": true, "closed": false,
  "top_bid": 200000, "created": "2025-07-03T14:59:40.000+00:00",
  "updated": "2026-04-29T02:48:29.000+00:00", "is_direct_sell": false,
  "id": "68669adc1a935b0006a3494e",
  "owner": {
    "reputation": 33, "ingame_name": "Smoothie26", "status": "offline",
    "last_seen": "2026-08-14T13:00:14.296+00:00", "region": "en", ...
  },
  "item": {
    "weapon_url_name": "dual_toxocyst",
    "attributes": [
      {"value": 139.1, "positive": true,  "url_name": "multishot"},
      {"value": 112.6, "positive": true,  "url_name": "critical_damage"},
      {"value": 207.0, "positive": true,  "url_name": "critical_chance"},
      {"value": -130.9, "positive": false, "url_name": "puncture_damage"}
    ],
    "polarity": "madurai", "mastery_level": 13, "re_rolls": 70,
    "mod_rank": 8, "type": "riven", "name": "crita-acrican"
  },
  "private": false
}
```

**The distinguishing field is `is_direct_sell`, not merely which price
fields are set**: `true` = a fixed "buy now" listing, and its sale price is
`buyout_price` (`starting_price` is also set to the same value by
convention, not meaningfully separate). `false` = a real open bidding
auction; `starting_price` is the floor bid, `top_bid` is the current highest
bid if any exist yet (`null` if no one has bid), and `buyout_price` may
*still* be non-null on a bidding auction as an optional instant-buy
escape hatch, or `null` for an uncapped auction. So there are three
practical shapes to handle, not two: buyout-only (`is_direct_sell: true`),
pure-auction-no-buyout (`is_direct_sell: false`, `buyout_price: null`), and
auction-with-buyout-option (`is_direct_sell: false`, `buyout_price` set).

Every field CONTEXT.md/issue #96 asked about is present: rolled stats
(`item.attributes[]`), seller online status (`owner.status`, same
`online`/`ingame`/`offline` vocabulary `MarketClient` already filters on —
just nested one level differently, `owner.status` here vs. `user.status` on
the v2 orders endpoint), mastery requirement (`item.mastery_level`),
polarity (`item.polarity`), mod rank (`item.mod_rank`), and re-rolls
(`item.re_rolls`). `closed` and `visible` were `false`/`true` respectively
on every one of the 500 returned listings — the search endpoint only ever
returns open, unclosed, visible auctions (relevant to §6).

## 5. Listing depth: good for popular weapons, hard-capped at 500, no working pagination — and a live outlier example

Testing four weapons at different popularity levels (all live, same day):

| weapon | listings returned |
|---|---|
| `dual_toxocyst` | 500 (capped) |
| `miter` | 500 (capped) |
| `brakk` | 318 (true count) |
| `kulstar` | 189 (true count) |

500 recurring exactly across two different, unrelated weapons strongly
indicates a server-side page-size cap, not a coincidence — confirmed by
`brakk`/`kulstar` returning their real, lower, non-round counts. Tried
`page=2`, `offset=500`, and `limit=1000` as pagination guesses against
`dual_toxocyst`: all three silently returned the identical first-500 result
again (`page=2` did not advance), so **there is no discovered way to see
past the 500th listing** for weapons that hit the cap. This doesn't block a
depth-weighted Floor/Ceiling in practice — the interesting listings for that
calculation are the cheap tail (floor) and the realistic-high tail
(ceiling), and 500 is deep enough to see both clearly — but it does mean the
app can't claim to have "all" listings for a hot weapon, only "the (unknown,
API-ordered) first 500."

The live `dual_toxocyst` buyout-price distribution is a direct, concrete
illustration of the exact outlier problem CONTEXT.md's Floor/Ceiling
definitions call out. Sorted, the 385 listings with a `buyout_price` set
cluster tightly at the low end and then thin into a long, sparse,
round-number tail:

```
2000 (×19) … 3000 (×34) … 5000 (×24) … 10000 (×22) …
20000 (×7), 30000 (×2), 40000 (×3), 50000 (×2), 60000, 66666,
150000, 666666 (×3), 888888 (×2)
```

Naively taking `max(buyout_price)` as "the ceiling" would report **888,888
platinum** — a price no real buyer would pay for a Dual Toxocyst riven.
Repeating-digit/round-meme prices (`666666`, `888888`, `9999`, `8888`,
`23456`) recurring at the top of *every* weapon's price list is a visible
community pattern of throwaway/joke/"not actually for sale" listings, not
genuine asks. This is exactly the "one riven sitting at 9000 platinum" problem
named in issue #96 — except live data shows it's worse than a single
outlier, it's a *recurring pattern* of them, which argues for percentile
trimming (e.g. 5th/95th percentile of active-seller listings) rather than
simple min/max, and probably for filtering to `owner.status in
{online, ingame}` first the same way `MarketClient::summarize` already does
for prime parts (in this same pull, only 49 of 500 `dual_toxocyst` owners —
39 `ingame` + 10 `online` — were reachable right now; 451 were `offline`).
The same pattern showed up on the bidding side too: a `top_bid` of `200000`
against a `starting_price` of `20000` and no buyout cap (shown in §4) is a
single-listing spike that would similarly distort a naive max-top-bid
ceiling.

## 6. Historical / completed-sale data: confirmed absent for rivens, and structurally unlikely to exist

**Regular items genuinely do have server-side sale history.** Live-tested
against `mirage_prime_set` (a prime part `MarketClient` already prices):

```
GET https://api.warframe.market/v1/items/mirage_prime_set/statistics
→ 200 OK
{"payload": {"statistics_closed": {"48hours": [...47 hourly buckets...],
                                    "90days":  [...89 daily buckets...]},
             "statistics_live": {...}}}
```

Each `90days` bucket is real closed-order history, not a live snapshot:

```json
{"datetime": "2026-05-18T00:00:00.000+00:00", "volume": 86,
 "min_price": 87, "max_price": 90, "open_price": 90, "closed_price": 90,
 "avg_price": 88.5, "wa_price": 89.14, "median": 89, ...}
```

— `volume` is a real trade count, and these fields (min/max/avg/median/
weighted-avg/open/close, per day, 90 days deep) are exactly the shape a
Floor/Ceiling calculation would want. This endpoint is live *despite* v1
orders (`/v1/items/{slug}/orders`) returning 403 — another instance of v1
being dead per-endpoint, not API-wide.

**No equivalent exists for rivens.** Every path guessed and tested returned
`404`:

```
/v1/rivens/dual_toxocyst/statistics
/v1/riven/weapons/dual_toxocyst/statistics
/v1/auctions/statistics?weapon_url_name=dual_toxocyst
/v2/riven/weapons/dual_toxocyst/statistics
/v1/items/dual_toxocyst/statistics   (dual_toxocyst isn't in the items catalog anyway)
```

Also tried smuggling a "give me closed ones too" signal into the working
search endpoint itself — `closed=true`, `closed=1`, `sold=true` — all were
silently ignored (still 500 results, still `closed: false` on every one);
there is no undocumented flip to reveal closed/completed auctions through
`/v1/auctions/search`. Since the search endpoint never surfaces a closed
auction, there's also no way to discover a closed auction's `id` to try a
single-record lookup against.

**This isn't just an unimplemented gap — it's structurally unlikely to ever
exist in the same shape.** The item-statistics endpoint works because a
prime part is a *fungible SKU*: "Mirage Prime Set" is interchangeable
between sellers, so "N sold at price P this hour" is a meaningful
aggregate. A riven is the opposite by design — every rolled copy is unique
(different stat rolls, rank, re-roll count, polarity), and rivens are
confirmed absent from `/v2/items` entirely (§3.2) — there's no single
fungible "SKU" per Riven type for a `statistics_closed`-shaped endpoint to
aggregate against the way it does for items. That doesn't prove DE/
warframe.market could never build one (they could bucket by weapon +
ignore roll quality, the way community sites approximate it), but no such
endpoint exists today under any path this research could find.

**Corroborating (not primary) evidence this is a genuine gap, not something
this research simply missed:**
[`leonardodalinky/pywmapi`](https://github.com/leonardodalinky/pywmapi) is
an actively-maintained third-party Python wrapper whose README explicitly
tracks implemented vs. not-yet-implemented vs. impossible endpoints (✅/🔲/🆖
markers) across the *whole* documented+undocumented API surface, including a
dedicated `rivens` module (`GET /v2/riven/weapons`, `GET
/v2/riven/attributes` — both confirmed live above) and an `experimental/auctions`
module. Its own auctions section marks "get a list of riven auctions by
given search params" as still 🔲 **not implemented**, and its full feature
list — spanning auth, profile, items, statistics, orders, liches, rivens,
auctions — contains **no riven sale-history function anywhere**. A wrapper
library actively tracking undocumented functionality not listing one is
circumstantial, but consistent with what direct testing above already
established more concretely.

**Bottom line for this repo's Floor/Ceiling calculation**: there is no
historical or completed-sale signal available for rivens from
warframe.market, full stop. The calculation has to be built entirely from
the current live-auction snapshot: depth (up to 500 listings per weapon,
confirmed plentiful for popular weapons — §5), seller reachability
(`owner.status`, same vocabulary as prime parts — §4), and outlier trimming
against the snapshot itself (e.g. percentile bounds, not raw min/max — §5's
`dual_toxocyst` data shows why raw min/max fails visibly). If a demand/trend
signal is wanted later, it would have to be built the same way this repo's
[mobile-inventory-api-coverage.md](mobile-inventory-api-coverage.md) found
WFHelper builds credit/resource history — by polling this live endpoint
repeatedly over time and persisting the app's own derived series — not by
reading a field the API exposes today.
