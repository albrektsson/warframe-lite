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

warframe-lite is a single self-contained binary — no runtime other than glibc, a
`dlopen`'d `libwayland-client.so.0` (present on every Wayland desktop), and the
`tesseract` CLI for the relic OCR feature.

**1. Get the binary.** Download `wf-lite` from the
[latest release](https://github.com/albrektsson/warframe-lite/releases/latest)
and put it on your `PATH`:

```
install -Dm755 wf-lite ~/.local/bin/wf-lite
```

Release binaries are built against **glibc 2.35** (Ubuntu 22.04), so they run on
any current distro (Fedora 39+, Arch, Debian 12, Ubuntu 22.04+). On an older
glibc, build from source (see [Build](#build)).

**2. Install tesseract** (only needed for the relic reward picker):

| Distro | Command |
|---|---|
| Fedora | `sudo dnf install tesseract tesseract-langpack-eng` |
| Arch | `sudo pacman -S tesseract tesseract-data-eng` |
| Debian / Ubuntu | `sudo apt install tesseract-ocr` |
| Bazzite / atomic | `brew install tesseract` |

Any reachable `tesseract` works; override the path with the `WF_TESSERACT` env var.

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

A Fedora `.spec` builds from source with the standard Rust macros and pulls in
`tesseract` as a dependency — see
[`packaging/warframe-lite.spec`](packaging/warframe-lite.spec). It can also back a
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
wf-lite capture [out.png]  # capture the Warframe window to a PNG
wf-lite relic [names…]     # evaluate reward names → matched item + plat
wf-lite relic-scan         # capture the reward screen, OCR the names, rank them
wf-lite detect-account     # auto-detect your account id from EE.log (verified)
wf-lite set-account <id>   # save your account id for mastery lookup
wf-lite mastery [id]       # report your mastered-item count
wf-lite logstats           # parse whole EE.log history, report coverage/events
wf-lite logwatch           # follow EE.log live, print recognized events
```

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
install (e.g. all into `~/.local/bin`).

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
