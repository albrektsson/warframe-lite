# Browse mastery/relics in a new `wf-browse` binary, not `wf-settings`

The Mastery/Owned-relic/Sell browsing GUI is its own companion binary,
`wf-browse`, launched the same way `wf-settings` and `wf-tray` are
(`wf-lite browse`, plus a `wf-tray` menu entry) — not a set of tabs bolted
onto `wf-settings`, and not a panel embedded in `wf-overlay`.

`wf-settings` exists to edit `config.toml`: it's opened rarely, touches only
local config, and its own doc comment is explicit that its heavier GUI deps
(`eframe`) are kept out of the leaner binaries on purpose. Mastery/relic
browsing is a different kind of tool — plausibly kept open for a while during
play, and pulling from three different data sources (the relic catalogue, the
DE mastery profile, and market prices) instead of one local file. Folding it
into `wf-settings` would mix two unrelated lifecycles and dependency sets
behind one window; a new binary keeps both purpose-built and lean, matching
the precedent `wf-settings`/`wf-tray` already set.

Embedding it in `wf-overlay` was ruled out even faster: the overlay is a
`wlr-layer-shell` click-through surface, not built for interactive,
scrollable widgets, and ADR-0001's observe-only spirit keeps it minimal by
design.
