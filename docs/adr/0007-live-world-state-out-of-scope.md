# Live world state is out of scope; only Fissures remain as a live feed

warframe-lite is scoped to **relics, equipment, and mastery** and the data
those touch (relic reward picking, owned-relic scanning, the mastery/fissure
plan, market prices, drop tables, mastery lookups). General live **world
state** — the Void Trader (Baro Ki'Teer) and the open-world Cetus/Vallis/
Cambion day-night cycles — is **out of scope** and will be removed. The single
`warframestat.us` feed that stays is the live **Fissure** list, because it is a
relic feature: the Mastery plan cross-references currently-active fissures to
flag which relic tiers are runnable right now.

## Context

The original plan (see `docs/PLAN.md`) framed the app as two co-equal
must-haves: a **relic reward picker** and **timers & world state**. Phase 1
shipped a world-state overlay showing fissures, Baro, and the three open-world
cycles, all polled from `warframestat.us`.

In practice the relic/equipment/mastery half is where the app's distinct value
lives — the OCR reward picker, owned-relic scanning, and the mastery plan are
things no trivial HTTP-against-a-public-API tool already does well. Baro and the
open-world cycles, by contrast, are commodity world-state readouts that any of
many existing web/app trackers show; carrying them here adds overlay surface,
config, CLI, and a data path to maintain for no relic/mastery benefit.

Fissures sit on the boundary. As a *data source* they come from the same
`warframestat.us` world-state endpoint as Baro and the cycles. But as a
*domain concept* a Fissure is where relics are cracked, and the Mastery plan
already uses the live active-fissure set to mark which owned-relic tiers can be
run right now. That makes fissures a relic feature that happens to be sourced
from the world-state API, not general world state.

## Decision

- Remove the Void Trader and open-world cycles from the domain, the overlay,
  the CLI, and config. Drop the code paths that fetch and render them.
- Keep the live **Fissure** feed from `warframestat.us`, trimmed to what the
  relic/mastery features need (active fissures and their tiers).
- Trim the `worldstate` module to a fissures-only surface rather than deleting
  it outright, so the Mastery plan's "tier active now" marker keeps working.
- Update `CONTEXT.md` and `docs/PLAN.md` to reflect the narrowed scope.

## Consequences

- The overlay and `wf-lite status` no longer show Baro or the open-world
  cycles; users wanting those use a general world-state tracker.
- The `worldstate` fetch/model shrinks to fissures; the Void Trader and cycle
  types, their rendering, and their tests are removed.
- The app's identity is simpler and singular: a relic/equipment/mastery
  companion, not a general Warframe HUD. New feature requests for non-relic
  world state are declined by default under this ADR.
- This is a scope decision, not a reversal of the observe-only rule
  ([ADR-0001](0001-observe-only-never-touch-game-process.md)); the retained
  fissure feed is still a one-directional read of a public API.
