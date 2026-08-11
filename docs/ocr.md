# OCR / relic-grid scanning

The relic OCR features are an **opt-in build-time cargo feature** (`ocr`) —
mirroring `mem-scan`'s old opt-in pattern (see
[ADR-0016](adr/0016-mem-scan-is-default-compiled-not-opt-in.md)). They gate:

- The overlay's automatic reward picker (the 2–4 rewards ranked by plat
  price, mastery emblems) and owned-relic/Prime-Part grid scanning (the Void
  Relics and Inventory/Sell screens).
- The `ocr`, `ocr-file`, `relic-file`, `relic-scan`, `relic-grid-file`, and
  `inventory-grid-file` CLI commands.

Without the feature, those commands print a short message pointing at a
rebuild instead of failing to compile or erroring confusingly, and the
overlay (`wf-lite overlay`/`wf-lite tray`) still runs normally — fissures,
the control socket, and manual reward evaluation (`wf-lite relic`) all work,
just without automatic reward/relic detection.

## Building with OCR

```
cargo build --release --features ocr
```

Needs, at build time:

- `tesseract-devel`/`libtesseract-dev`-style headers
- `leptonica-devel`/`libleptonica-dev`-style headers
- `clang` (for the FFI bindgen step)

`wf-ocr` links `libtesseract`/`libleptonica` **in-process** via `leptess`
behind a small engine pool (see
[ADR-0008](adr/0008-link-libtesseract-via-a-pool-not-the-cli.md)) — it never
shells out to a `tesseract` CLI. A binary built with `--features ocr` needs
those two shared libraries present at *runtime*, not the CLI on `PATH`.

## Runtime libraries

Only needed if your binary was built with the `ocr` feature — skip this if
it wasn't; the reward picker and OCR commands just tell you so instead of
failing confusingly.

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
