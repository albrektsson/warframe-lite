# Are mission-start/mission-end EE.log markers already known publicly?

Research for [issue #87](https://github.com/albrektsson/warframe-lite/issues/87), part of the
Wayfinder map [issue #86](https://github.com/albrektsson/warframe-lite/issues/86), blocking
[issue #89](https://github.com/albrektsson/warframe-lite/issues/89) (exact-marker confirmation),
which is also blocked by the sibling empirical-capture ticket
[issue #88](https://github.com/albrektsson/warframe-lite/issues/88) — that capture step is
explicitly out of scope here.

## Question

Do any public/community sources — other `EE.log`-parsing tools, the Warframe wiki, forum
threads, or a disassembly of Warframe's shipped Lua — already document or credibly imply the
`EE.log` line(s) that mark (1) leaving a hub (Orbiter/Relay/Dojo) and loading into mission
gameplay, and (2) mission end (extraction, abort, or return to a hub)?

## Answer, in short

**Partial candidates, at three different confidence levels — no source (public or in this
research) empirically confirms any of them against a live-captured, current-build `EE.log`, so
none should be wired into `crates/wf-log` without the capture issue #88 first (matching this
repo's own prior experience with `InventorySellOpen` in issue #37).** In descending order of
confidence:

1. **Wiki-documented, but flagged speculative and not proven against this app's mission-detection
   need**: the official Warframe Wiki's `EE.log` article shows a real captured line, `Sys [Info]:
   Client loaded {"name":"<Node>"} with MissionInfo:`, firing when a mission's parameters load —
   a strong mission-**start**-adjacent signal, but the wiki page itself carries `{{Speculation}}`
   and `{{Stub}}` maintenance tags (§1).
2. **Shipped and matched-against in two independent real-world parsers, but of uncertain scope**:
   `WFCD/warframe-deathlog`'s `Mission Complete Bonus: <credits>` regex (its own README calls it
   "not entirely accurate") for mission end with reward, and WFinfo's `MatchingService::EndSession`
   substring, used as a "session over" signal for an unrelated purpose (§2, §3).
3. **Un-confirmed, this-repo's-own-methodology disassembly inference** (the same approach that
   found `InventoryTest.lua: PopulateGrid` in issue #37): candidate Lua script/function names —
   `MissionComplete.lua: TriggerReturnToLobby`, `ExtractionTimer.lua:
   ExtractionTriggerFirstTouched`, and, most notably, `ThemedSquadOverlay.lua:
   OnReturnToOrbiter` / `CancelMission` / `NotifyForceLoadSoloMission` / `InitiateDojoVisit` —
   found in the **same Lua file this repo already has a live-confirmed marker from**
   (`OnLoginComplete`), which raises confidence somewhat, but the disassembly dump they come from
   is six years stale (§4).

No source — WFinfo, `warframe-deathlog`, the wiki, or general web/forum search — has a marker
that is both **documented as mission-start/mission-end specifically** and **empirically
confirmed on a current build**. This matches the ticket's own stated expectation going in.

## 1. The Warframe Wiki: a real "mission info loaded" line, but flagged speculative

[wiki.warframe.com/w/EE.log](https://wiki.warframe.com/w/EE.log) (fetched 2026-08-13) opens with
`{{UpdateMe}}{{Stub}}{{Speculation}}` maintenance templates — the community wiki's own editors
flag this page as unverified/incomplete. With that caveat, its `===Mission Details===` section
shows a real captured excerpt:

```
282.067 Sys [Info]: Loading game rules: LotusGameRules
282.067 Sys [Info]: Setting gGameRules
282.067 Sys [Info]: Client loaded {"name":"SolNode42_Alert"} with MissionInfo:
{
    "missionType" : "MT_DEFENSE",
    ...
    "levelOverride" : "/Lotus/Levels/Proc/Grineer/GrineerGalleonDefense",
    ...
}
```

The single-line trigger (`Client loaded {"name":"SolNode42_Alert"} with MissionInfo:`) is well
formed for this crate's `parse_line`/`classify` shape — the multi-line JSON that follows has no
`<secs> <Subsystem> [<Level>]:` prefix, so it already falls outside `parse_line`'s accepted shape
(continuation lines, same as this crate's own doc comment on `parse_line` already anticipates)
and doesn't need to be parsed for the trigger to work. This is a `Sys`-subsystem line, not the
`Script`-lua-function convention the rest of this crate's markers follow — it looks like an
engine-level trace of the mission JSON payload being applied, not a traced Lua function call.

Separately, the same page documents (under "Important Game Events", also without
`{{Speculation}}` caveat re-applied per-item, but under the same page-level flag):

```
112.501 Sys [Info]: ===[ Game successfully connected to: /Lotus/Levels/Proc/PlayerShip/DOA.lp ]===
```

captioned "Game connected to [[Orbiter]]" — i.e. a generic "level connected" system line whose
payload is the level's asset path. The wiki only shows this for the Orbiter case. It is a
reasonable **inference**, not a confirmed fact, that the same `Game successfully connected to:
<path>` pattern fires for every level load, including mission tilesets and Relay/Dojo — which
would make it a single, symmetric signal for both directions (mission path vs. hub path)  — but
no source shown here demonstrates the pattern recurring for a mission or a Relay/Dojo
specifically. The [Fandom mirror of the same article](https://warframe.fandom.com/wiki/EE.log)
carries the same content (not an independent corroboration — same community lineage — but at
least a second maintained copy).

The wiki's `===Host Migration===` section (matching this crate's `HostMigration` doc comments)
and the dedicated [Host Migration wiki page](https://wiki.warframe.com/w/Host_Migration) (fetched
2026-08-13) both explicitly list mission abort as *one of several* host-migration triggers
("When host player aborts the mission through the menu") — confirming `HostMigration` alone is
not a usable mission-end signal (it's necessary-but-far-from-sufficient, and only fires when a
non-host player is present to notice the migration), consistent with this crate not currently
treating it as one.

## 2. `WFCD/warframe-deathlog`: a shipped, credited "mission end with reward" regex

[`WFCD/warframe-deathlog`](https://github.com/WFCD/warframe-deathlog) is a maintained
Warframe-Community-Developers ("Inspired by and designed after Semlar's [Death Log
Parser](https://semlar.com/deathlog)") Node.js `EE.log` tailer. Its
[`src/regex.js`](https://raw.githubusercontent.com/WFCD/warframe-deathlog/master/src/regex.js)
ships:

```js
eomRegex: /^([0-9.]+).*Mission Complete Bonus: ([\d]+)/,
closedRegex: /^([0-9.]+).*Main Shutdown Complete\.$/,
```

wired up in
[`src/Parser.js`](https://raw.githubusercontent.com/WFCD/warframe-deathlog/master/src/Parser.js)
and [`src/events/EndOfMissionEvent.js`](https://raw.githubusercontent.com/WFCD/warframe-deathlog/master/src/events/EndOfMissionEvent.js).
The project's own [README](https://raw.githubusercontent.com/WFCD/warframe-deathlog/master/README.md)
lists its three supported events as "Start", "Death", and **"Successful mission end (with
credits, not entirely accurate)"** — i.e. the maintainer's own admission that
`Mission Complete Bonus: <credits>` only catches *some* mission-end cases (almost certainly
successful/rewarded completions; an abort is unlikely to emit a credit-bonus line at all, since
there's no bonus to report). `closedRegex` ("Main Shutdown Complete.") is a **game-closing**
marker, not a mission-end/return-to-hub marker — noted here only to rule it out.

`warframe-deathlog`'s "Start" event, despite the name, is **not** a per-mission start marker —
[`src/events/StartEvent.js`](https://raw.githubusercontent.com/WFCD/warframe-deathlog/master/src/events/StartEvent.js)
derives it from `Sys [Info]: Logged in <name>` (session login) and a `Sys [Diag]: Current time:`
line, both fired once per game-process launch, not once per mission. This project has no
per-mission-start signal at all.

## 3. WFinfo (`WFCD/WFinfo`): one incidental "session end" substring, used for something else

WFinfo no longer tails the `EE.log` file directly — current
[`WFInfo/LogCapture.cs`](https://raw.githubusercontent.com/WFCD/WFinfo/master/WFInfo/LogCapture.cs)
reads the game's Windows debug-output buffer (`DBWIN_BUFFER`) via a memory-mapped file, not the
log file on disk, though the messages it captures are the same text the game also writes to
`EE.log`. Its consumer,
[`WFInfo/Data.cs`](https://raw.githubusercontent.com/WFCD/WFinfo/master/WFInfo/Data.cs)`:LogChanged`
(around line 1566), keys off relic-reward substrings almost exclusively — `"Pause countdown
done"`, `"Got rewards"` — matching this crate's own `RELIC_REWARD_MARKERS`. One line is
different, though:

```csharp
if (!(line.Contains("MatchingService::EndSession") || line.Contains("Relic timer closed"))
    || !(_settings.AutoList || _settings.AutoCSV || _settings.AutoCount)) return;
```

`MatchingService::EndSession` is checked as an *alternative* trigger, alongside the relic-timer
close, to fire WFinfo's auto-listing of reward screens once the relevant multiplayer session is
over. It is a real substring WFCD's maintainers put in production code — not speculative — but
neither WFinfo's source, its `Log Examples.txt` fixture (checked directly; does not contain this
string at all — see below), the Warframe Wiki, nor a targeted web search turned up any
documentation of what specifically causes `MatchingService::EndSession` to fire, or whether it
fires once per mission (return to hub) vs. only in narrower circumstances (e.g. specifically
squad-session teardown after a relic fissure run, which is the only context WFinfo's own code
uses it in). It is plausible by name — matchmaking "sessions" in Warframe map to mission/hub
instances — but this is inference, not a documented fact.

[`WFCD/WFinfo`'s `Log Examples.txt`](https://raw.githubusercontent.com/WFCD/WFinfo/master/Log%20Examples.txt)
(checked directly, 136 lines) contains only relic-reward-screen log excerpts (`Relic rewards
initialized`, `Got rewards`, `_PlayersChanged. N member(s) left`, etc.) — no mission-start,
extraction, or hub-return lines at all, confirming this repo's earlier note that WFinfo has "no
prior art" for screens/transitions outside its core relic-reward use case, extending that finding
from the Inventory/Sell screen (issue #31/#37) to mission transitions as well.

Two other Linux/community reimplementations of WFinfo were checked for anything additional:
[`knoellle/wfinfo-ng`](https://github.com/knoellle/wfinfo-ng) and
[`soramanew/wfinfo-linux`](https://github.com/soramanew/wfinfo-linux) — both are relic-reward-OCR
tools only, with no mission-transition log parsing to speak of (not deep-dived further, since
their entire purpose, like upstream WFinfo, is scoped to the reward screen).
[`GodlySchnoz/Warframe-EE.log-reader`](https://github.com/GodlySchnoz/Warframe-EE.log-reader) (a
Python `EE.log` combat/death analyzer, source checked directly) also has no mission-transition
parsing — only player-name detection (`Sys [Info]: Logged in`, same as `warframe-deathlog`),
start/end timestamps, and `Game [Info]: ... was killed/downed by ...` combat lines.

## 4. Disassembly inference (this repo's own precedent method), with one notably strong lead

Per this ticket's brief, the same disassembly-based approach that produced the
`InventoryTest.lua: PopulateGrid` prediction for issue #37 has a public precedent:
[`rogerxiii/warframe-lua-disassembled`](https://github.com/rogerxiii/warframe-lua-disassembled)
("All lua files disassembled of the game Warframe, always up-to-date"). **Caveat up front: the
repo's own claim of being "always up-to-date" doesn't hold — its last push was 2020-07-18, so
this is a six-year-old snapshot as of 2026-08-13.** Script contents, function names, and even
whole files may have been renamed, refactored, or removed since. Function/table-key names in
this era's dump are also partially hashed (shown as `0xXXXXXXXX` hex literals) where DE's
string-hashing obfuscation applies — many, but not all, global function names remain in plain
text.

Searching the tree (`git ls-tree -r`, 3,323 paths) for mission/extraction/loading-related
filenames surfaced:

- **`Lotus/Scripts/MissionComplete.lua`** — disassembly shows two top-level global functions
  defined directly on the file's root closure: `TriggerReturnToLobby` and `ReturnToLobbyNoEom`.
  Per this crate's established convention (`<ScriptName>.lua: <FunctionName>` trace lines for
  named Lua function calls), the predicted markers are `MissionComplete.lua:
  TriggerReturnToLobby` and `MissionComplete.lua: ReturnToLobbyNoEom` — both directly on-topic for
  "mission end / return to hub," with `ReturnToLobbyNoEom` ("No End-Of-Mission") reading as a
  strong candidate specifically for the **abort** path (skipping the normal end-of-mission
  screen), distinct from a normal completion.
- **`Lotus/Scripts/ExtractionTimer.lua`** — top-level globals `ExtractionTriggerFirstTouched`,
  `ExtractionTriggerFirstUntouched`, `ExtractionTriggerFull`, `ExtractionTriggerEmptied`. These
  read as the extraction-zone trigger-volume callbacks (player enters/leaves the extraction
  marker) — a precursor signal to mission end, not equivalent to the hub-return moment itself,
  but plausibly useful for "extraction is now available/active."
- **`Lotus/Interface/MissionIntro.lua`** — top-level globals `Initialize`, `Shutdown`, `Update`,
  `onViewportSizeChanged`. `Initialize`/`Shutdown`/`Update` are common HUD-component lifecycle
  names shared by many unrelated interface scripts in this same dump (e.g.
  `MissionProgressDisplay.lua` defines the identical trio) — the function names alone are
  generic, but the full `MissionIntro.lua: Initialize` / `MissionIntro.lua: Shutdown` substrings
  would still be filename-specific if the convention holds, and a "mission intro" HUD element is
  a very plausible thing to initialize right at mission start and tear down shortly after.
- **`Lotus/Interface/ThemedSquadOverlay.lua` — the strongest lead here.** This is the *exact
  same file* this crate already cites as a confirmed marker source (the doc comment atop
  `crates/wf-log/src/lib.rs` gives `Script [Info]: ThemedSquadOverlay.lua: OnLoginComplete -
  squad overlay` as the crate's own canonical example of the `<ScriptName>.lua:
  <FunctionName>` convention, and `OnLoginComplete` is exactly one of this file's top-level
  globals in the 2020 disassembly, at `SETGLOBAL ... OnLoginComplete`). That the function this
  crate already relies on is still present, under the same name, in a six-year-old dump is
  circumstantial evidence that this particular file's naming has been comparatively stable —
  which raises (without confirming) confidence in its *other* globals from the same dump,
  several of which are squarely on-topic:
  - `OnReturnToOrbiter` — about as on-the-nose a name as could be hoped for; predicted marker
    `ThemedSquadOverlay.lua: OnReturnToOrbiter`.
  - `CancelMission` — predicted `ThemedSquadOverlay.lua: CancelMission`, a mission-abort
    candidate.
  - `NotifyForceLoadSoloMission` and `LoadAutonomousMultiplayerMission` — mission-load
    candidates (solo/autonomous-squad launch paths specifically; may not cover every launch
    path, e.g. joining a public squad already in a mission).
  - `InitiateDojoVisit`, `OpenDojoLevel`, `ConfirmEnterDojoLeaveSquad` — Dojo-hub-entry
    candidates.
  - `OnSquadMissionSelected` — fires at mission *selection*, before launch, not the launch/load
    moment itself — noted to rule it out as a start marker.

None of these are confirmed against any real captured log line in this research — they are
**structurally the same class of prediction** as `InventoryTest.lua: PopulateGrid()` was before
issue #37's empirical capture confirmed it: grounded in a real (if stale) disassembly and this
crate's own established naming convention, but unverified. `ThemedSquadOverlay.lua:
OnReturnToOrbiter` in particular is the single most promising candidate to prioritize when issue
#88's capture happens, given the same-file precedent above.

## Caveats and gaps

- No source found — wiki, WFinfo, `warframe-deathlog`, or the disassembly dump — is contemporary
  and confirmed simultaneously. The wiki content is flagged speculative by its own editors; the
  shipped-code markers (`Mission Complete Bonus:`, `MatchingService::EndSession`) are real
  strings from real production tools but of documented-uncertain or undocumented scope; the
  disassembly is six years stale.
- GitHub code search (`gh api search/code`) was used to search within `WFCD/WFinfo` and
  `WFCD/warframe-deathlog` specifically; a repo-unscoped code search across all of GitHub for
  `EE.log`-parsing mission-transition logic was not exhaustively possible with the tools
  available here (GitHub's code search API requires either a scoped query or authentication
  context this session didn't fully exploit beyond the repos above) — it's possible another,
  less-discoverable public `EE.log` parser has something this research missed.
- No Discord-indexed content was reachable (Discord servers, including Warframe's own, aren't
  crawled by general web search) — the ticket's mention of "discord-adjacent forum posts indexed
  by search" turned up nothing because there wasn't anything web-search-indexed to find; this is
  a gap in *reach*, not a claim that Discord has nothing.
- None of the candidates in §4 were cross-checked against a second, independent disassembly or
  decompilation source — only `rogerxiii/warframe-lua-disassembled` was found and used.

## Verdict

**Partial candidates only — nothing here rises to "usable public marker" the way
`ProjectionRewardChoice.lua`'s reward-screen lines or (after its own empirical confirmation)
`InventoryTest.lua: PopulateGrid` do.** The best-supported leads, in priority order for the
empirical capture in issue #88 to test first, are: `ThemedSquadOverlay.lua: OnReturnToOrbiter`
(same file as this crate's live-confirmed `OnLoginComplete`, semantically exact) and
`ThemedSquadOverlay.lua: CancelMission`/`NotifyForceLoadSoloMission` for the mission
end/abort/start triad from that same file; `Sys [Info]: Client loaded {"name":"..."} with
MissionInfo:` for mission start (wiki-documented, real captured text, but flagged speculative);
`MissionComplete.lua: TriggerReturnToLobby`/`ReturnToLobbyNoEom` and `ExtractionTimer.lua`'s
trigger callbacks as a second-tier disassembly lead; and `Mission Complete Bonus: <credits>` /
`MatchingService::EndSession` as shipped-but-scope-uncertain fallbacks. None should be adopted
into `crates/wf-log` on this research alone — issue #88's real capture is the only way to turn
any of these from "plausible, named, and independently sourced" into "confirmed," exactly as it
was for `InventorySellOpen` in issue #37. This research narrows what that capture should be
checked against, rather than replacing the need for it.
