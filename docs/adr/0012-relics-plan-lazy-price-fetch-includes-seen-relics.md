# Relics & Plan tab's price fetching becomes lazy, retryable, and includes Seen relics

Nagantaka Prime, Trumna Prime, Scourge Prime, and many other primes were
showing up in the Relics & Plan tab with no Set price and no relic prices,
even though `items.json`, `relics-v2.json`, and `part-quantities-v3.json` all
have complete, current data for them. Two independent causes:

- `load_data`'s Set-price batch resolves every Unmastered prime's `{prime}
  Set` slug once, at app launch, through `cached_price`'s 2.5s
  fetch-or-fall-back-to-stale path (`wf-relic/src/lib.rs`). A slug with
  nothing cached yet and a fetch that times out or errors gets `None`
  permanently — there's no retry until the next full relaunch. In practice
  only ~13 of ~150 Unmastered primes' Set prices were landing in the cache
  from a single eager pass.
- Relic-level prices in the same tab are gated on `owned_counts()`, which by
  design excludes Seen relics (see ADR-0009: Seen "never itself gates or is
  gated by the count"). But the Relics & Plan list itself already includes
  Seen relics — `mastery_plan` keeps any relic with *either* trust tier — so
  a Seen relic renders with no price next to it at all, forever, since
  nothing ever asks the market for its slug.

Both read as the same symptom (blank price) but need different fixes,
because they're not the same kind of gate.

## What we do

- **Set/relic price fetching moves from eager-at-launch to lazy-on-first-view.**
  `relics_tab` renders its full ~150-prime list flat, every frame — it has no
  expand/collapse like the Relics EV tab does — so "lazy" here means "fetch
  when the tab is opened this session," not "fetch when a row is expanded."
  This reuses the existing `fetch_prices`/`PriceCache` machinery (same
  `PRICE_FETCH_CONCURRENCY = 8` bound, same on-disk cache) already proven by
  the Relics EV tab's lazy Intact/Radiant pricing — just triggered by tab
  view instead of row expand.
- **A price fetch still missing after its first attempt retries automatically,
  on a per-item cooldown (~30–60s), for as long as the tab stays open.**
  Required specifically because the tab re-renders every frame with no
  virtualization: a naive "retry if still missing" check with no cooldown
  would refire 60×/second for anything that fails. No new global concurrency
  cap is added — each fetch call site keeps its own independent 8-wide bound,
  which is judged sufficient for a single-user desktop app against a public
  API.
- **Relic-level price *eligibility* for this tab now includes Seen relics,
  not just Confirmed-count ones — but this is scoped narrowly to price
  fetching, not to `owned_counts()` itself.** `owned_counts()` keeps meaning
  exactly what ADR-0009 says it means, because it also feeds `sell_picks` and
  `farm_picks`, which report "you have N copies to sell/farm" — folding Seen
  into that would manufacture a sellable/farmable count for a relic never
  actually confirmed. The Relics & Plan tab already displays Seen relics on
  their own terms (labeled `"seen"` rather than `xN`, per
  `RelicEvidence::SeenOnly` at the render site); this change only makes sure
  a displayed Seen relic also gets asked for a price, through a separate
  eligibility path that does not touch `owned_counts()`'s callers.
- **Blank price cells become explicit states** — a loading indicator while a
  fetch is in flight or on cooldown, distinct from an explicit "no listing
  found" once a fetch has genuinely come back empty — instead of empty space
  that reads as broken.

## Consequences

- `relics_tab` (or its data layer) gains per-item fetch state (not-yet-fetched
  / loading-or-cooldown / resolved) it didn't need before, since price
  resolution is no longer a single up-front batch computed in `load_data`.
- The Relics & Plan tab now makes more outbound warframe.market requests over
  a session than before (Seen relics that were never priced now are, and a
  session with several tab views can retry previously-failed items) — judged
  acceptable given the existing 8-wide per-call-site bound and no evidence of
  rate-limiting from the current, already-larger Sell/Farm eager batches.
- `owned_counts()`'s contract (Confirmed-count only, per ADR-0009) is
  unchanged; nothing about `sell_picks` or `farm_picks` ranking changes.
- App-launch time should improve slightly, since the Set-price batch no
  longer runs unconditionally as part of `load_data` for users who never
  open the Relics & Plan tab.

## Scope

`wf-browse/src/main.rs` (`relics_tab`, `load_data`'s Set-price batch, the
Relics EV tab's `spawn_relic_price_fetch`/`fetch_prices` machinery this
reuses) and `wf-relic` (`owned.rs`'s `owned_counts()`/`owned_evidence`, and
wherever the new price-eligibility set is derived). Does not touch
`wf-relic::owned`'s schema or trust tiers themselves (ADR-0009), and does not
change `sell_picks`/`farm_picks` semantics.
