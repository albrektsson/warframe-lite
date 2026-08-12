# ocr is default-compiled; the relic reward picker is the app's main point

Issue [#71](https://github.com/albrektsson/warframe-lite/issues/71) made
`wf-ocr` (and its hard dependent `wf-gridscan`) an opt-in `wf-lite` cargo
feature, mirroring `mem-scan`'s old opt-in pattern: a bare `cargo build`/
`cargo build --release` built every other subsystem but never linked
libtesseract/libleptonica, requiring `--features ocr` explicitly and needing
none of `libtesseract-dev`/`libleptonica-dev`/`clang` for a plain build.

That decision is reversed here, per direct user instruction: the automatic
relic reward picker — detecting the post-mission reward screen and ranking
the 2-4 choices by live plat price — is the app's main point (see
`CONTEXT.md`); everything else (fissure timers, mastery guide, mem-scan) is
secondary. Shipping a default build that silently can't do that (the
`ocr_disabled.rs` stand-in only prints a one-line startup log message, easy
to miss) defeats the app's purpose for anyone who builds it the obvious way.
`ocr` now joins `mem-scan` in `wf-lite`'s `default` feature set, and
`crates/wf-ocr`/`crates/wf-gridscan` rejoin the workspace's
`default-members`, matching [ADR-0016](0016-mem-scan-is-default-compiled-not-opt-in.md)'s
reversal of `mem-scan`'s equivalent opt-in.

This is recorded as its own ADR, narrowing #71's decision rather than
editing it in place, following the convention ADR-0016 set relative to
map #55.

## Why this doesn't touch ADR-0001

ADR-0001's observe-only rule is about *what* the OCR path is allowed to do
at runtime (read-only screen capture, no game-process interaction) — this
ADR is purely about which crates a plain `cargo build` links. OCR was never
gated on a runtime consent prompt the way `mem-scan` is (ADR-0013); it's a
passive screen-capture feature with no equivalent privileged-capability
grant, so there's no consent model to preserve or break here.

## Why reverse it, unlike wf-mem's build-dep tradeoff

ADR-0016 explicitly notes `wf-mem` carries no extra system build
dependencies, unlike `wf-ocr`'s native `tesseract`/`leptonica` linkage —
that asymmetry was part of why `mem-scan` went default first while `ocr`
stayed opt-in under #71. That tradeoff (a bare `cargo build` needing
`libtesseract-dev`/`libleptonica-dev`/`clang`) is real and unchanged by this
ADR — it's just outweighed: those packages are a one-line install on every
targeted distro (see `docs/ocr.md`'s per-distro table), while a build that
can't detect the reward screen is not meaningfully useful to most users who
`git clone && cargo build --release` without reading the README closely
enough to notice the opt-in flag. The `ocr` feature flag itself is kept
(not deleted) so `--no-default-features`/`--no-default-features --features
mem-scan` still lets tests and CI exercise the OCR-free configuration.

## Packaging

`packaging/warframe-lite.spec` and the release CI workflow were removed
entirely by #75 (no packaged builds pre-1.0), so there's no packaging
channel left whose `BuildRequires`/dependency list needs updating alongside
this change. CI's own `test`/`build` jobs already install
`libtesseract-dev`/`libleptonica-dev`/`clang` unconditionally (see
`.github/workflows/ci.yml`), so this ADR needs no CI changes — the release
build step now simply links what it was already prepared to link.
