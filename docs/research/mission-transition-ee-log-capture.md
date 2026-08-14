# Real EE.log capture: mission-start and mission-end transitions

Capture for [issue #88](https://github.com/albrektsson/warframe-lite/issues/88), part of the
Wayfinder map [issue #86](https://github.com/albrektsson/warframe-lite/issues/86). Unlike the
sibling research ticket [issue #87](https://github.com/albrektsson/warframe-lite/issues/87)
(no confirmed public marker), this ticket's job was to get a real, current-build `EE.log`
covering a live mission launch and mission end — the input issue #89 needs to confirm exact
markers.

## Source

An already-existing `EE.log` from a normal play session on this machine (build `2026.08.12.14.02
Retail Windows x64`, captured 2026-08-13) already covered both required transitions cleanly, so
no fresh staged capture was needed. The session: login → hub → launch a mission (a Warframe 1999
"Steel Path" assassination node, solo/host) → play → extract → back to hub → normal client
shutdown (`Main Shutdown Complete.`). Line numbers below refer to that file; the account name is
redacted to `<player>` throughout (the file itself is not attached/committed — excerpts only).

## The finding: one symmetric marker pair, not several candidates

Contrary to the research ticket's multiple partial candidates, this capture shows **one clean,
symmetric mechanism** that fires identically for every level transition — hub or mission —
distinguished only by the level's asset path:

1. **`Game [Info]: FrameworkCmd::OpenLevel - <level path>`** — fires the instant a level load is
   *commanded* (loading screen begins).
2. **`Sys [Info]: ===[ Game successfully connected to: <level path>/<hash>.lp ]===`** — fires when
   that level has *finished* loading and is playable (loading screen ends).

Both lines appeared **exactly twice each** in the whole ~1,273-second, 10,608-line session (once
per transition) — no false positives, no noise to filter.

### Mission start (Orbiter/hub → mission)

```
341.965 Script [Info]: RetroMap.lua: Confirm sector SolNode856_Hard
341.965 Net [Info]: Set squad mission: {"name":"SolNode856_Hard","difficulty":0}
341.965 Script [Info]: ThemedSquadOverlay.lua: OnSquadMissionSelected - force=false
...
351.997 Script [Info]: ThemedSquadOverlay.lua: Mission name: Assassinate: H-09 Tank (Höllvania) - THE STEEL PATH
351.997 Script [Info]: ThemedSquadOverlay.lua: Host loading {"name":"SolNode856_Hard","difficulty":0} with MissionInfo:
info={
    ...
    levelOverride=/Lotus/Levels/Proc/Vania/VaniaAssassinationSummer
    ...
}

351.998 Script [Info]: ThemedSquadOverlay.lua: Lobby::Host_StartMatch: launching level for SolNode856_Hard (/Lotus/Levels/Proc/Vania/VaniaAssassinationSummer)
351.998 Game [Info]: FrameworkCmd::OpenLevel - /Lotus/Levels/Proc/Vania/VaniaAssassinationSummer
352.002 Sys [Info]: ResourceLoader ... (/Lotus/Levels/Proc/Vania/VaniaAssassinationSummer) Found 104 items to load
   ... (~9s of loading-screen resource spam) ...
361.076 Sys [Info]: ===[ Game successfully connected to: /Lotus/Levels/Proc/Vania/VaniaAssassinationSummer/DgRwSS6oQG89EaVLSRAQICAoIGCA.lp ]===
```

`OnSquadMissionSelected` fires at mission *selection* (before the 10s launch countdown) — matches
issue #87's research, which already ruled it out as a start marker. The `MissionInfo:` line
(`Host loading {...} with MissionInfo:`) is the mission-selection-confirmed signal, but note the
verb differs by host/client role — this line reads **"Host loading"** here (solo/host), not the
wiki's speculative **"Client loaded"** — a squad member joining someone else's session would
presumably see the other wording. `FrameworkCmd::OpenLevel`, one line later, has no such
role-dependent wording and is otherwise identical in both directions (see below) — the stronger
candidate of the two.

### Mission end (mission → Orbiter/hub)

```
1068.545 Script [Info]: ExtractionTimer.lua: EOM: All players extracting
1068.545 Sys [Info]: Cinematic ... Start, 1 scene(s), first: /Lotus/Animations/Tenno/Motorcycle/Extraction_cin.fbx
1068.548 Script [Info]: EndOfMatch.lua: Initialize
1068.549 Script [Info]: EndOfMatch.lua: Mission Succeeded
   ... (EndOfMatch.lua reward/profile-save sequence, ~7s while results screen is shown) ...
1075.713 Script [Info]: EndOfMatch.lua: EndOfMatch.lua - Close
1075.719 Net [Info]: GameRulesImpl - changing state from SS_STARTED to SS_ENDING
1075.719 Net [Info]: MatchingService::EndSession
1075.719 Net [Info]: GameRulesImpl - changing state from SS_ENDING to SS_ENDED
1075.720 Game [Info]: FrameworkCmd::OpenLevel - /Lotus/Levels/Proc/TheNewWar/PartTwo/TNWDrifterCampMain
1075.725 Sys [Info]: ResourceLoader ... (/Lotus/Levels/Proc/TheNewWar/PartTwo/TNWDrifterCampMain) Found 55 items to load
   ... (~5.6s of loading-screen resource spam) ...
1081.352 Sys [Info]: ===[ Game successfully connected to: /Lotus/Levels/Proc/TheNewWar/PartTwo/TNWDrifterCampMain/BoC8ACA.lp ]===
```

`ExtractionTimer.lua: EOM: All players extracting` (issue #87's research candidate) fires at
extraction, but the results screen (`EndOfMatch.lua`) stays open for ~7 more seconds until the
player closes it (auto-close here) — the level doesn't actually change, and the player is still
mid-results-screen, not "back at the hub," until `FrameworkCmd::OpenLevel` fires afterward. Issue
#87's other candidate, `MatchingService::EndSession`, is real and does fire here, in the same
instant as `OpenLevel` — but note WFinfo's own usage of it (per issue #87's research) implies
narrower scoping (squad-session teardown); this capture only shows it firing here, not e.g. on a
solo non-squad mission (this run was host of a squad session, even solo).

### Baseline: the same hub path from a clean boot

The very first `Game successfully connected to` line in the file (session boot, before any
mission) already shows the identical hub path as the post-mission return:

```
15.202 Sys [Info]: ===[ Game successfully connected to: /Lotus/Levels/Proc/TheNewWar/PartTwo/TNWDrifterCampMain/BoC8ACA.lp ]===
```

confirming `TNWDrifterCampMain` is stable across a session, not something that only appears once.

## New wrinkle for issue #89 to weigh: level-path classification isn't a simple prefix check

Both the mission path (`/Lotus/Levels/Proc/Vania/VaniaAssassinationSummer`) **and** the hub path
captured here (`/Lotus/Levels/Proc/TheNewWar/PartTwo/TNWDrifterCampMain`) share the same
`/Lotus/Levels/Proc/...` prefix — the wiki's own example Orbiter path
(`/Lotus/Levels/Proc/PlayerShip/DOA.lp`, cited in issue #87's research) is a *third*, structurally
different hub path again. A "does the path start with X" heuristic won't separate hub from
mission; whatever ships will need either an allow-list of known hub path prefixes (Orbiter,
Relay, Dojo, and apparently per-quest-hub variants like this 1999-era Drifter Camp) or some other
signal, with "unrecognized path" presumably falling under the map's fail-open-show default. Also
worth note: this session's hub was **not** the classic Orbiter but a Warframe 1999 quest-specific
hub space — still correctly "non-mission" for this effort's purposes, but a data point that hub
paths vary by context, not just by player ship choice.

## What this capture does not cover

- **Abort mid-mission** (quitting via the pause menu rather than extracting) — not captured this
  session; optional per issue #88's own scope. If issue #89 needs it, worth a short follow-up
  capture, but the `FrameworkCmd::OpenLevel` mechanism found here is a plausible universal answer
  (an abort presumably also issues an `OpenLevel` back to the hub) that may not need a dedicated
  new marker to confirm.
- **A Relay or Dojo hub path** — not visited this session; would give a second/third real hub-path
  sample for the classification question above.
- **A squad session where this client is not host** (`Client loaded ... with MissionInfo:` wording
  instead of `Host loading ...`) — not captured; irrelevant to `FrameworkCmd::OpenLevel`/`Game
  successfully connected to`, which showed no host/client wording variance, but relevant if issue
  #89 ends up preferring the `MissionInfo:` line instead.

## Verdict

`FrameworkCmd::OpenLevel - <path>` (loading begins) and `Game successfully connected to: <path>`
(loading ends, playable) form a single, symmetric, low-noise marker pair that fires for every
level transition observed in this capture, mission or hub alike — a stronger and simpler
candidate than any single item in issue #87's research. The remaining open question for issue #89
is not *which line* fires, but **how to classify a given path as "hub" vs. "mission"** — not a
simple prefix rule, per the wrinkle above — plus optionally confirming the abort path and a
second hub sample (Relay/Dojo).
