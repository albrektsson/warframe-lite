# `wf-browse` may clear (never set) owned-relic entries — a second, narrow exception to ADR-0003

A refined relic's card disappears from the in-game Void Relics screen
entirely once its count reaches zero — unlike an Intact card, which persists
and shows the "unowned" eye icon (see ADR-0009's Seen tier). The scanner has
no negative signal to act on when that happens: it only ever zeroes an entry
on an explicit eye-icon read (ADR-0005). A refined relic the player fully
consumes, sells, or reforges away therefore leaves a permanently stale
`(code, refinement)` entry in `owned-relics.json` — one no future scan can
ever correct, because the card that would prove it's gone no longer exists to
scan. Trying to infer "not seen this session" as "confirmed zero" was
considered and rejected: a single scroll never reliably covers the full
772-relic list (the whole premise of ADR-0008/0009), so that heuristic would
manufacture false zeros for relics simply not yet scrolled past, mirroring
the false-Seen-positive problem the tie-break fix in `RelicIndex::best_match`
already had to solve from the other direction.

ADR-0003 forbids `wf-browse` from writing `owned-relics.json` specifically to
prevent it from becoming a second, competing definition of "owned" — a
manually-set count is "a guess wearing the same shape as a fact." Clearing an
entry is a different operation: it asserts nothing. Deleting a
`(code, refinement)` key returns it to its default, never-scanned state; the
next real scan re-derives whatever Seen/Confirmed-count state actually holds
from OCR, same as for any relic never previously recorded. `wf-browse` cannot
use this to set or correct a count to a value it believes is right — only to
forget one, at the player's explicit request, when they know a scan can never
clear it on its own.

## What we do

- `wf-browse` gains two user-initiated (button-triggered, never automatic or
  silent) actions on `owned-relics.json`: clear one `(code, refinement)`
  entry, and a full reset (equivalent to the "back up and start clean"
  treatment the scanner already applies to an incompatible file, ADR-0005) —
  both pure deletions.
- Both actions write directly (no backup-then-prompt ceremony beyond what a
  full reset already does) — a single clear is trivially recoverable by
  rescanning; a full reset already matches the scanner's own precedent for
  discarding the file outright.

## Consequences

- `wf-browse` is no longer literally read-only for `owned-relics.json`, but
  its write path is constrained to deletion only — it can never introduce a
  count or Seen flag the scanner didn't itself observe. ADR-0003's core
  guarantee (every *positive* fact in the file came from OCR) is unchanged.
- Distinct from ADR-0004's wishlist exception: that one grants normal
  read/write ownership of a browser-owned file with no scan-derived truth to
  diverge from. This one grants a narrower capability (delete-only) on a file
  `wf-browse` still does not otherwise own.
