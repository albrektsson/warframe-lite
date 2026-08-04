# OCR engine: link `libtesseract` via a small per-thread pool, not shell out to the CLI

`wf-ocr` shells out to the `tesseract` CLI per recognition call, on the stated
assumption that OCR "runs at most once per reward screen, so process-spawn
overhead is irrelevant." The relic-grid scanner (added after that assumption
was written) breaks it: it OCRs the visible card grid on repeat, dozens of
Tesseract subprocess spawns per scan cycle (ADR-0006). A live capture test
confirmed the cost of this: one clean, disciplined, single-direction scroll
through the full 772-relic list, timed under 90 seconds, gained only 4 newly
**Confirmed count** relics. Consecutive scan cycles rarely see the same card
twice at a player's natural scroll speed (pauses of ≤1s between wheel ticks),
so the cross-frame agreement vote (ADR-0005) almost never converges — no
amount of phase-alignment or OCR-accuracy tuning fixes that if the underlying
per-cycle latency is the bottleneck. Subprocess-spawn and model-reload cost is
the dominant, avoidable part of that latency.

We switch `wf-ocr` to `leptess` (FFI bindings to libtesseract/leptonica,
in-process, no subprocess) behind a small pool of independent engine
instances — sized at roughly half the available CPU cores, leaving headroom
for capture/overlay/tokio work during a scan burst — rather than one shared
engine. Tesseract's `TessBaseAPI` is not safe for concurrent calls on a single
instance (tesseract-ocr/tesseract#4281); `scan_relic_grid` already relies on
true parallel recognition across dozens of cards per cycle (today free via
process isolation), so a single mutex-guarded instance would serialize that
away. The pool preserves the parallelism at a bounded size instead of shelling
out per call.

We considered `rusty-tesseract` as the binding, since it was the other
candidate named in `docs/PLAN.md`'s original tech-stack notes — but it also
just shells out to the CLI (it depends on the `subprocess` crate), so it would
not have addressed the latency problem at all.

## Consequences

- Reverses `wf-ocr`'s original CLI-vs-library rationale; its module doc needs
  correcting to explain why the "runs at most once" assumption no longer holds
  for the relic scanner.
- Adds a build-time dependency on `libtesseract-dev`, `libleptonica-dev`, and
  `clang` (for `bindgen`) — a change to the release build environment (Fedora
  COPR/RPM `BuildRequires`) and CI, not to end users: anyone with a working
  `tesseract` CLI already has the runtime shared libraries as its transitive
  dependency.
- `Ocr::new()` / `Ocr::recognize()` keep their existing public shape and
  error-handling behaviour (a clear "OCR unavailable" error), so the
  reward-picker and relic-grid callers don't change; only `wf-ocr`'s internals
  and its concurrency contract (a bounded pool instead of unlimited threads)
  change.

## Scope

`wf-ocr` only. Callers keep calling the same `Ocr` API.
