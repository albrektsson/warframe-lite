# warframe-lite

A standalone, **Linux-native** companion for [Warframe](https://www.warframe.com/) —
a light alternative to [AlecaFrame](https://alecaframe.com/) that runs **without
Overwolf**. Built for KDE Plasma (Wayland) with the game under Steam Proton, in Rust.

It shows a click-through overlay on top of the game with:

- **Live Void Fissures** — sorted normal → Steel Path → Storm, with ETAs that
  tick down each second. (General world state — the Void Trader and open-world
  cycles — is out of scope; see `docs/adr/0007-live-world-state-out-of-scope.md`.)
- **An automatic relic reward picker** — when a Void Fissure reward screen
  appears, the overlay swaps to the 2–4 rewards ranked by live warframe.market
  plat price, with the best pick highlighted and a mastery emblem in front of
  primes you have already **mastered**. No keypress needed.
- **An owned-relic mastery guide** — open the in-game **Void Relics** screen and
  the overlay automatically scans your relics as you scroll, then shows which of
  the ones you own can still drop a prime you **haven't mastered**, ranked by
  relic price. Also `wf-lite relics <codes…>` from the CLI.
- **A mastery planner** — `wf-lite mastery-plan` flips the view around: for every
  prime you haven't mastered, which of your owned relics (and how many) can
  still drop it, so you know which fissure tier is worth farming next. The
  scanned owned-relic set is saved to disk, so this works any time — no need to
  have the Relics screen open.

It only *observes* the game — no Overwolf, no memory reading, no account
credentials.

## Install & run

warframe-lite is a single self-contained binary — no runtime other than glibc and a
`dlopen`'d `libwayland-client.so.0` (present on every Wayland desktop). The relic
OCR features (the automatic reward picker, the owned-relic scanner, and the
`ocr`/`ocr-file`/`relic-file`/`relic-scan` commands) are an **opt-in build-time
feature** (`--features ocr`, see [Build](#build)) — a binary built with it links
`libtesseract`/`libleptonica` **in-process** (ADR-0008), so it needs those shared
libraries present at runtime, not a `tesseract` CLI on `PATH`. A binary built
*without* the feature runs fine without either library; those commands just print
a message pointing at a rebuild instead.

**1. Get the binary.** Download `wf-lite` from the
[latest release](https://github.com/albrektsson/warframe-lite/releases/latest)
and put it on your `PATH`:

```
install -Dm755 wf-lite ~/.local/bin/wf-lite
```

Release binaries are built against **glibc 2.35** (Ubuntu 22.04), so they run on
any current distro (Fedora 39+, Arch, Debian 12, Ubuntu 22.04+). On an older
glibc, build from source (see [Build](#build)).

**2. Install tesseract's runtime libraries** (only if your binary was built with
the `ocr` feature — skip this if it wasn't; the reward picker and OCR commands
just tell you so instead of failing confusingly):

| Distro | Command |
|---|---|
| Fedora | `sudo dnf install tesseract-libs tesseract-langpack-eng leptonica` |
| Arch | `sudo pacman -S tesseract tesseract-data-eng leptonica` |
| Debian / Ubuntu | `sudo apt install tesseract-ocr` (pulls in `libtesseract`/`libleptonica`) |
| Bazzite / atomic | `brew install tesseract` |

These packages happen to also ship the `tesseract` CLI binary, but wf-lite never
invokes it — it links `libtesseract.so`/`libleptonica.so` directly in-process
(ADR-0008), so only the shared libraries actually matter.

**3. Run it — the tray is the easy way.** Launch **`wf-tray`** (from a menu
shortcut or terminal). It sits in the KDE system tray, waits for Warframe to
start, and **auto-starts the overlay when the game window appears** (and stops it
when the game closes). The tray menu shows/hides the overlay, opens **Settings**,
detects your account id, and quits. Install the desktop shortcut so it appears in
your launcher:

```
install -Dm755 wf-tray ~/.local/bin/wf-tray
install -Dm644 packaging/warframe-lite.desktop ~/.local/share/applications/warframe-lite.desktop
install -Dm644 packaging/warframe-lite.svg ~/.local/share/icons/hicolor/scalable/apps/warframe-lite.svg
```

Prefer no tray? Start the overlay directly instead — either in a terminal
(`wf-lite overlay`) or from Warframe's **Steam launch options**:

```
wf-lite overlay & %command%
```

`wf-lite overlay` polls up to 30s for the game window, then anchors the panel to
its top-right corner (correct in fullscreen *and* borderless-windowed).

**4. (Optional) Enable mastery badges.** Detect your account id from the game log
(scraped and verified against the public profile, so it can't pick a squadmate):

```
wf-lite detect-account
```

The id only appears in the log after some activity (a relic crack in a squad, a
Duviri race); if detection can't find it, set it manually — find it at
`warframe.com/api/user-data`:

```
wf-lite set-account <id>
```

### Fedora (COPR / RPM)

A Fedora `.spec` builds from source with the standard Rust macros — see
[`packaging/warframe-lite.spec`](packaging/warframe-lite.spec). It currently
declares the `ocr` feature's `BuildRequires` (`tesseract-devel`,
`leptonica-devel`, `clang`) and `Requires` (`tesseract-libs`, `leptonica`)
unconditionally; whether the officially-published build actually passes
`--features ocr` is still an open question (tracked on the wayfinder map, #68)
independent of the `ocr` feature's existence. It can also back a
[COPR](https://copr.fedorainfracloud.org/) repo for `dnf install warframe-lite`.

## Commands

```
wf-lite                    # (no command) show this list of commands
wf-lite status             # live Void Fissures
wf-lite <market_slug>      # price an item, e.g. `wf-lite mirage_prime_set`
wf-lite relics <codes…>    # owned-relic guide: unmastered rewards + prices
wf-lite mastery-plan       # unmastered primes + which owned relics drop them
wf-lite tray               # tray companion: waits for the game, runs the overlay
wf-lite overlay            # show the live overlay (live fissures + relic picker)
wf-lite settings           # open the graphical settings window (needs wf-settings)
wf-lite toggle             # show/hide a running overlay (also: show / hide)
wf-lite copy               # copy the current best-pick reward (name + plat) to the clipboard
wf-lite capture [out.png]  # capture the Warframe window to a PNG
wf-lite relic [names…]     # evaluate reward names → matched item + plat
wf-lite relic-scan         # (feature-gated: ocr) capture the reward screen, OCR the names, rank them
wf-lite detect-account     # auto-detect your account id from EE.log (verified)
wf-lite set-account <id>   # save your account id for mastery lookup
wf-lite mastery [id]       # report your mastered-item count
wf-lite logstats           # parse whole EE.log history, report coverage/events
wf-lite logwatch           # follow EE.log live, print recognized events
wf-lite mem-scan           # read live Foundry state — see "mem-scan" below
```

### OCR / relic-grid scanning (opt-in feature)

The relic OCR features — `ocr`, `ocr-file`, `relic-file`, `relic-scan`,
`relic-grid-file`, `inventory-grid-file`, and the live overlay's automatic
reward-picker and owned-relic/Prime-Part scanners — need the `ocr` cargo
feature at build time:

```
cargo build --release --features ocr
```

Without it, those commands print a short message pointing at this rebuild
instead of failing to compile or erroring confusingly, and the overlay
(`wf-lite overlay`/`wf-lite tray`) still runs normally — fissures, the
control socket, and manual reward evaluation (`wf-lite relic`) all work — just
without automatic reward/relic detection. Building *with* `--features ocr`
needs `tesseract-devel`/`libtesseract-dev`-style headers, `leptonica-devel`/
`libleptonica-dev`-style headers, and `clang` (for the FFI bindgen step) —
see [Build](#build).

### mem-scan (Phase 4)

`mem-scan` reads Foundry state (in-progress and owned Prime blueprints)
straight out of the running game's own memory, then echoes the session token
it finds there once to DE's own `inventory.php` endpoint — see
[ADR-0001](docs/adr/0001-observe-only-never-touch-game-process.md) and
[ADR-0013](docs/adr/0013-token-relay-session-nonce-is-not-a-credential.md)
for why this is a read, not a held credential. Nothing it reads is ever
cached or written to disk.

It's part of `wf-lite`'s default build ([ADR-0016](docs/adr/0016-mem-scan-is-default-compiled-not-opt-in.md)),
so a bare `cargo build --release` already includes it; the `mem-scan` cargo
feature still exists so tests/CI can build without it via
`--no-default-features`:

```
cargo build --release                                     # includes mem-scan (default)
cargo build --release --no-default-features --features mem-scan  # explicit, equivalent
cargo build --release --no-default-features                # excludes mem-scan
```

Running `wf-lite mem-scan` at all **is** the explicit in-the-moment consent
this feature assumes — it doesn't prompt separately. It needs Warframe
already running and logged in, and the binary needs permission to read
another process's memory. This map decided against wiring that into
packaging, so it's a manual, one-time step after each build/install:

```
sudo setcap cap_sys_ptrace=+ep /path/to/wf-lite
```

(Alternatively, lower `/proc/sys/kernel/yama/ptrace_scope` to `1` or below —
`setcap` is the narrower, per-binary grant and the preferred option.) Without
either, `mem-scan` fails immediately with a clear permission error naming
both fixes, rather than a bare I/O error.

## Configuration

Config lives at `~/.config/warframe-lite/config.toml` (created on demand); the
`EE.log` path is auto-detected from the Steam Proton prefix but can be overridden
there. Network results (item catalogue, prices, mastered set) are cached under
`~/.cache/warframe-lite/`.

### Overlay placement

Warframe uses every screen corner for HUD and menu elements, so the overlay's
position and visibility are configurable under `[overlay]`:

```toml
[overlay]
anchor = "top-right"   # top-left | top-right | bottom-left | bottom-right
                       # | top | bottom | left | right | center
margin_x = 24          # horizontal inset from the anchored edge(s), px
margin_y = 24          # vertical inset
fissures = true        # false = reward-only: invisible until a relic reward screen
opacity = 1.0          # 1.0 = as-drawn, lower = more transparent (e.g. 0.7)
```

**Hide it on a hotkey.** The overlay is click-through and can't grab a global
key itself, so bind a **KDE custom shortcut** (System Settings → Shortcuts →
Add Custom → Command) to `wf-lite toggle` (or `wf-lite hide` / `wf-lite show`).
The running overlay listens on a control socket and shows/hides instantly.

**Copy a reward to the clipboard.** Bind another KDE custom shortcut to
`wf-lite copy` to copy the currently-displayed best-pick reward's name and
plat price (e.g. `Mirage Prime Systems 45p`) to the clipboard, ready to paste
into Warframe's trade chat. Needs `wl-clipboard` ≥ 2.3.0 for
`ext-data-control-v1` support on KWin ≥ 6.5 — older packaged versions (e.g.
Fedora/Debian/Ubuntu's stock 2.2.1) may hit `wl-copy`'s own documented
popup-surface hang fallback instead of copying instantly. Override the binary
with `WF_WL_COPY` if yours is named or pathed differently.

### Settings window

`wf-lite settings` opens a small graphical window (`wf-settings`) to edit
placement, opacity, and the fissure-panel toggle, detect your account id, and help
bind the KDE hotkey — all writing the same `config.toml`. It's a **separate
binary** so the overlay stays dependency-light; download `wf-settings` from the
release alongside `wf-lite`, or build it with `cargo build --release -p
wf-settings`. Restart `wf-lite overlay` to apply placement changes.

## Build

```
cargo build --release
cargo test
```

This builds all three binaries into `target/release/`: `wf-lite` (the overlay/CLI)
plus the `wf-tray` and `wf-settings` companions. `wf-lite tray` / `wf-lite settings`
expect the companion binaries next to `wf-lite`, so keep them together when you
install (e.g. all into `~/.local/bin`). The plain build above needs none of
`tesseract-devel`/`leptonica-devel`/`clang` — the relic OCR features
(reward picker, relic-grid/Inventory-Sell scanning, `ocr`/`ocr-file`/
`relic-file`/`relic-scan`/`relic-grid-file`/`inventory-grid-file`) are an
opt-in cargo feature (mirroring `mem-scan`'s pattern):

```
cargo build --release --features ocr
```

which does need those three build-time packages (`libtesseract`/
`libleptonica` are FFI-linked in-process, ADR-0008) plus the runtime
libraries from [Install & run](#install--run) at run time.

## License

MIT — see [`LICENSE`](LICENSE).

The bundled *Warframe* UI assets — the mastery laurel
(`crates/wf-overlay/assets/mastered.png`, shown on mastered rewards) and the
"unowned" eye icon (`assets/relic-unowned-eye.png`, used only to detect which
relics you don't own) — remain the property of **Digital Extremes Ltd.** They are
included solely to identify game state in this fan companion; *Warframe* is a
trademark of Digital Extremes. This project is unofficial and not affiliated with
or endorsed by Digital Extremes.

## Design & roadmap

See [`docs/PLAN.md`](docs/PLAN.md) for the feasibility analysis, architecture, and
implementation status; [`CONTEXT.md`](CONTEXT.md) for the project vision and
domain vocabulary; and [`AGENT.md`](AGENT.md) for contributing conventions.
