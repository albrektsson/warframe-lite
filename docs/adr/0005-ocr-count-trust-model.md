# OCR-derived owned-relic counts are confirmed by agreement, never by max

The Void Relics screen is OCR'd frame-by-frame as the player scrolls, and every
frame's count-badge read is noisy. We resolve each `(relic code, refinement)`
count by **voting**: keep a per-session histogram of reads and trust the mode
only once it has been seen on at least two frames (`N=2`). A confirmed value
**replaces** the persisted one, so a count can correct downward. Counts drop to
zero only on positive evidence — a card scanned showing the "unowned" eye icon
— and an entry that is simply not re-seen is kept and flagged stale by its
per-entry `last_seen`, never auto-deleted.

## Context

The original design merged reads with `max` (both within a frame and across the
session), on the theory that OCR mostly *drops* digits, so the largest read is
the truest. That is one-directional: `max` cancels undercounts but amplifies any
single *over*count forever (a lone `x145` misread beats fifteen correct `x15`
reads until the session resets), and scrolling can only ratchet a count up,
never correct it. Compounding this, the badge parser concatenated every digit it
saw with no bound and no `x`-prefix check, so it manufactured the very
overcounts `max` then locked in.

## Consequences

- A count only reaches the screen after a second agreeing read; a single dwell
  on a relic confirms it, but a relic glanced past once shows its prior (aging)
  value rather than an unvetted number.
- The badge parser must reject anything that is not a clean `x?NN` under a
  plausibility cap. A **genuinely blank** count region means the player owns
  exactly one (singles show no badge); an ink-present-but-unreadable badge casts
  no vote rather than silently defaulting to one.
- Zeroing on the eye icon means eye-marked cards can no longer short-circuit
  before OCR — their names must be read so the right relic is zeroed, and the
  zeroing is agreement-gated so one false-positive eye match cannot wipe a real
  count.
- The persisted schema becomes `code → { refinement → { count, last_seen } }`
  with a wall-clock `last_seen` per entry. The pre-existing flat `u32` file is
  backed up and ignored on upgrade (every prior count is suspect anyway).

## Scope

The screen-agnostic core — multi-frame vote/agreement, strict badge parse,
`blank = 1`, `{count, last_seen}` entries, and stale-entry handling — is shared
(in `wf-ocr`) so the forthcoming equipment scan reuses it. Region calibration,
the eye-icon ownership signal, and refinement-suffix parsing stay relic-specific.
