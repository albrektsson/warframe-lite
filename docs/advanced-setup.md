# Advanced setup

Everything past "build it and run `wf-lite`" — running without the tray, a
desktop launcher, mastery badges, mem-scan, hotkey binding, and the full
configuration reference.

## Desktop launcher

Install the desktop shortcut so `wf-lite` (tray mode) appears in your
application launcher:

```
install -Dm644 packaging/warframe-lite.desktop ~/.local/share/applications/warframe-lite.desktop
install -Dm644 packaging/warframe-lite.svg ~/.local/share/icons/hicolor/scalable/apps/warframe-lite.svg
```

## Running without the tray

Prefer no tray icon? Start the overlay directly instead — either in a
terminal:

```
wf-lite overlay
```

or from Warframe's Steam launch options:

```
wf-lite overlay & %command%
```

`wf-lite overlay` polls up to 30s for the game window, then anchors the
panel to its top-right corner (correct in fullscreen *and*
borderless-windowed).

## Mastery badges

The relic reward picker highlights primes you've already mastered with a
mastery emblem, once it knows your account id. Detect it from the game log
(scraped and verified against the public profile, so it can't pick a
squadmate):

```
wf-lite detect-account
```

The id only appears in the log after some activity (a relic crack in a
squad, a Duviri race); if detection can't find it, set it manually — find it
at `warframe.com/api/user-data`:

```
wf-lite set-account <id>
```

## mem-scan

`wf-lite mem-scan` reads Foundry/relic/equipment state straight out of the
running game's own memory (read-only) instead of via OCR. It needs a
one-time grant so the binary can read another process's memory:

```
sudo setcap cap_sys_ptrace=+ep /path/to/wf-lite
```

See [docs/mem-scan.md](mem-scan.md) for the alternative `ptrace_scope` grant
and the full consent model.

## Hotkey binding

The overlay is click-through and can't grab a global key itself, so bind
hotkeys through KDE instead (System Settings → Shortcuts → Add Custom →
Command):

- **Hide/show the overlay** — bind a custom shortcut to `wf-lite toggle` (or
  `wf-lite hide` / `wf-lite show`). The running overlay listens on a control
  socket and shows/hides instantly.
- **Copy the current reward** — bind a custom shortcut to `wf-lite copy` to
  copy the currently-displayed best-pick reward's name and plat price (e.g.
  `Mirage Prime Systems 45p`) to the clipboard, ready to paste into
  Warframe's trade chat. Needs `wl-clipboard` ≥ 2.3.0 for
  `ext-data-control-v1` support on KWin ≥ 6.5 — older packaged versions (e.g.
  Fedora/Debian/Ubuntu's stock 2.2.1) may hit `wl-copy`'s own documented
  popup-surface hang fallback instead of copying instantly. Override the
  binary with `WF_WL_COPY` if yours is named or pathed differently.

## Configuration reference

Config lives at `~/.config/warframe-lite/config.toml` (created on demand);
the `EE.log` path is auto-detected from the Steam Proton prefix but can be
overridden there. Network results (item catalogue, prices, mastered set) are
cached under `~/.cache/warframe-lite/`.

`wf-lite settings` (an alias for `wf-lite browse`) opens the browse window on
its **Settings** tab, to edit placement, opacity, and the fissure-panel
toggle, detect your account id, and help bind the KDE hotkey — all writing
the same `config.toml`. Restart `wf-lite overlay` to apply placement changes.

Warframe uses every screen corner for HUD and menu elements, so the
overlay's position and visibility are configurable under `[overlay]`:

```toml
[overlay]
anchor = "top-right"   # top-left | top-right | bottom-left | bottom-right
                       # | top | bottom | left | right | center
margin_x = 24          # horizontal inset from the anchored edge(s), px
margin_y = 24          # vertical inset
fissures = true        # false = reward-only: invisible until a relic reward screen
opacity = 1.0          # 1.0 = as-drawn, lower = more transparent (e.g. 0.7)
```
