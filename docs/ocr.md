# OCR / relic-grid scanning

The relic OCR features are part of `wf-lite`'s **default build** (the `ocr`
cargo feature — see
[ADR-0017](adr/0017-ocr-is-default-compiled-not-opt-in.md); it reverses
#71's old opt-in pattern, since the automatic reward picker is the app's
main point). They cover:

- The overlay's automatic reward picker (the 2–4 rewards ranked by plat
  price, mastery emblems) and owned-relic/Prime-Part grid scanning (the Void
  Relics and Inventory/Sell screens).
- The `ocr`, `ocr-file`, `relic-file`, `relic-scan`, `relic-grid-file`, and
  `inventory-grid-file` CLI commands.

Building with `--no-default-features` (or `--no-default-features --features
mem-scan`) drops the `ocr` feature instead — those commands then print a
short message pointing at a rebuild instead of failing to compile or
erroring confusingly, and the overlay (`wf-lite overlay`/`wf-lite tray`)
still runs normally — fissures, the control socket, and manual reward
evaluation (`wf-lite relic`) all work, just without automatic reward/relic
detection.

## Building

```
cargo build --release
```

Needs, at build time (unless you drop OCR via `--no-default-features
--features mem-scan`):

- `tesseract-devel`/`libtesseract-dev`-style headers
- `leptonica-devel`/`libleptonica-dev`-style headers
- `clang` (for the FFI bindgen step)

`wf-ocr` links `libtesseract`/`libleptonica` **in-process** via `leptess`
behind a small engine pool (see
[ADR-0008](adr/0008-link-libtesseract-via-a-pool-not-the-cli.md)) — it never
shells out to a `tesseract` CLI. The built binary needs those two shared
libraries present at *runtime*, not the CLI on `PATH`.

## Runtime libraries

Only needed if your binary was built with the `ocr` feature (the default) —
skip this if you built with `--no-default-features --features mem-scan`;
the reward picker and OCR commands just tell you so instead of failing
confusingly.

| Distro | Command |
|---|---|
| Fedora | `sudo dnf install tesseract-libs tesseract-langpack-eng leptonica` |
| Arch | `sudo pacman -S tesseract tesseract-data-eng leptonica` |
| Debian / Ubuntu | `sudo apt install tesseract-ocr` (pulls in `libtesseract`/`libleptonica`) |
| Bazzite / atomic | `brew install tesseract` |

These packages happen to also ship the `tesseract` CLI binary, but wf-lite
never invokes it — only the shared libraries actually matter (ADR-0008).

No packaged builds (Fedora/COPR, GitHub Releases, or otherwise) are
published pre-1.0 — build from source (see the root
[README](../README.md#build)). Packaging is deferred until after 1.0.
