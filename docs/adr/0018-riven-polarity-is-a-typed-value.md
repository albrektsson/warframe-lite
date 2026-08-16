# Riven polarity is a typed value, unifying two upstream spellings

Polarity was stored as `Option<String>` end to end and rendered verbatim, so
the Owned Rivens grid showed DE's raw internal codes (`AP_ATTACK`,
`AP_TACTIC`, ...) instead of the names players actually know (Madurai,
Naramon, ...). A second upstream source — warframe.market's auction
listings — already spells the same five values as plain lowercase names
(`"madurai"`, `"vazarin"`, ...) but had no display path of its own.

We introduce a `Polarity` enum (five known variants plus an `Unknown(String)`
fallback) that both sources parse into at their boundary, so every consumer
downstream deals in one canonical type instead of two raw string spellings.
It lives in `wf-data`: the only crate reachable from both parse sites (the
mem-scan path via `wf-mem`, and the market-auction path in `wf-data` itself)
without introducing a new dependency edge, given `wf-data → wf-relic →
wf-mem → wf-browse`.

## Scope

`wf-data` (new `Polarity` type + parsers for both spellings), `wf-mem`
(gains a direct `wf-data` dependency alongside its existing `wf-relic` one),
`wf-relic::riven_decode`, `wf-browse`'s Owned Rivens grid render site
(`lib.rs:2934`/`2939`). Confirmed mapping: `AP_ATTACK` = Madurai,
`AP_DEFENSE` = Vazarin, `AP_TACTIC` = Naramon, `AP_POWER` = Zenurik,
`AP_WARD` = Unairu.
