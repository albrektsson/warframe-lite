# `wf-browse` is read-only for OCR-derived state: relic ownership only ever changes via OCR scan

_Scope note (see ADR-0004): this rule covers OCR-derived state like
`owned-relics.json` specifically. It does not extend to hand-curated,
player-declared data such as the equipment wishlist, which has no scan-derived
truth to diverge from._

`wf-browse` never writes to `owned-relics.json` — there is no way, from the
GUI, to increment, decrement, or otherwise correct an Owned relic count, even
one that's visibly wrong from a bad OCR read. The only thing that can change
an owned-relic count is the scanner itself, by rescanning the in-game Void
Relics screen.

This extends ADR-0001's observe-only principle to a new surface: the scanner
is already the single source of truth for ownership, precisely because it's
the only channel the game's memory/API boundary allows. Letting the browser
also mutate that same file would create a second, competing definition of
"owned" — the GUI's view could silently diverge from what the scanner
actually observed, and any future feature reading `owned-relics.json` (or a
later rescan) would have to reconcile the two. It would also erode the trust
the observe-only design is meant to protect: a manually-edited count is no
longer a fact about the game, it's a guess wearing the same shape as one.

If OCR miscounts turn out to matter in practice, the fix belongs in the
scanner — better regions/thresholds (see issue #7, per-resolution
calibration) — not a manual override in a different tool.
