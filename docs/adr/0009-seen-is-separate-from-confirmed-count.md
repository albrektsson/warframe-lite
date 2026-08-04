# Owned-relic "Seen" is a separate, weaker signal from its Confirmed count

ADR-0005 trusts a relic's owned count only once two frames agree on the same
value, specifically to defeat noisy digit misreads on the `xNN` badge.
Comparing our persisted owned-relic set against the game's own "Collected
N/772" showed the gap is mostly not wrong counts — it's relics that never
clear that bar at all, even identity-wise, because a natural continuous
scroll rarely revisits the same card twice within one scan cycle (see
ADR-0008). The badge-noise problem and the relic-identity problem are
different questions with different failure modes: a relic's name+refinement,
matched at ≥80% similarity against the known catalogue
(`RelicIndex::best_match`) with no "unowned" eye icon present, is reasonably
trustworthy from a single clean read — the exact stack size is not.

We add **Seen**: a relic is marked owned from one clean identity read,
independent of its **Confirmed count**. The count itself keeps requiring two
agreeing frames as before, so ADR-0005's trust guarantee for numeric counts is
unchanged. A Seen relic with no confirmed count yet counts toward the owned
total (the figure compared against the game's "Collected N/772"), but its
exact count is reported as unknown/pending rather than guessed, until it
separately clears the count's own agreement bar.

## Consequences

- `owned-relics.json`'s schema changes from `code → refinement →
  {count, last_seen}` to `code → refinement → {seen, count: Option<{value,
  last_seen}>}`. Existing files need the same "back up and start clean"
  treatment ADR-0005 already established for incompatible formats.
- `wf-relic::owned::intact_counts` and the mastery/farm/sell planners need an
  actual number to rank by, and keep using only entries with a confirmed
  count — a Seen-but-uncounted relic doesn't feed those yet.
- The relic-scan-loop's owned-relic total (compared against the game's own
  count) now includes Seen entries, not just Confirmed-count ones.

## Scope

`wf-relic::owned` (schema, `apply_confirmed_count` and friends) and the
relic-grid scan loop in `src/main.rs` that feeds it. Does not touch
`wf-ocr`'s trust core (`Tally`, `parse_badge`), which ADR-0008 changes for
unrelated (performance) reasons.
