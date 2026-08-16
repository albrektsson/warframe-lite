# Riven verdict fetch becomes lazy-on-view, with a dedicated rate limit for auctions-search

`fetch_riven_verdicts` runs eagerly inside `load_data` at app launch, firing
`PRICE_FETCH_CONCURRENCY = 8` concurrent `/v1/auctions/search` calls (one per
distinct owned-riven weapon, already deduped) with no throttle beyond that
concurrency cap, and `load_data` doesn't return until the whole batch
resolves. warframe.market documents a general 3 req/s budget, but a much
tighter ~10-20 req/min budget specifically for auctions-search — so an
account with rivens spread across many weapons can burst past that budget
and/or stall app launch for minutes on a cold cache. This is the same
failure shape ADR-0012 already fixed once for relic/Set prices: an eager,
unthrottled, launch-blocking batch against a public API.

We apply the same fix here: riven verdict fetching moves to lazy-on-view
(fetched only once the Owned Rivens tab is opened, with per-item
loading/cooldown states, same as ADR-0012), and gets its own token-bucket
rate limiter (~15 req/min) scoped to auctions-search specifically, since
even a single tab-open across many distinct weapons could otherwise burst
past that endpoint's tighter budget on its own.

Two adjacent options were rejected. WebSocket subscriptions (which the
warframe.market docs recommend over polling generally) don't address the
actual problem — a launch-time burst, not a lack of real-time push — and are
a materially larger change; left for a future issue if ever pursued. A
general proactive rate limiter across the *other* warframe.market endpoints
(items, orders, weapon catalogue, relic/set prices) was also rejected:
ADR-0012 already judged the existing 8-wide concurrency pattern sufficient
for the general 3 req/s budget at this app's actual call volume, and nothing
here changes that.

## Scope

`wf-browse::fetch_riven_verdicts`/`load_data`, `wf-relic`'s riven price
cache/fetch machinery, a new rate limiter scoped to
`RivenMarketClient::auctions_for` only. Does not touch the Relics & Plan
tab's existing lazy-fetch machinery (ADR-0012) or the other warframe.market
endpoints' concurrency handling.
