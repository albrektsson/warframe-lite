# Memory-reading ban risk and prior art

Research for [issue #10](https://github.com/albrektsson/warframe-lite/issues/10),
the optional Phase 4 research spike into reading full inventory (foundry, riven
details, credits/plat, stats history) via `process_vm_readv` against
`Warframe.x64.exe` under Steam Proton. `docs/adr/0001-observe-only-never-touch-game-process.md`
already settles the *technical* axis — read-only, no writes, no injection,
no debugger attach in the sense of stopping the target — permanently and
without exception. What nobody had checked yet is the separate,
orthogonal axis: whether attempting this, even done exactly as the ADR
requires, carries **account-ban risk** distinct from "is it technically
possible." This note settles that gap and surveys prior art before the team
decides whether to greenlight a throwaway POC.

## Question

Does attempting a read-only memory-reading POC against Warframe carry
meaningful account-ban risk, and what prior art exists to inform the
approach?

## Answer, in short

**No evidence of ban risk was found for read-only memory-reading tools, and
strong prior art already exists — including one open-source tool that reads
`Warframe.x64.exe`'s memory on Linux today, the same way issue #10 proposes.**
Warframe's Steam/global PC build runs no named anti-cheat product — no EAC,
BattlEye, Vanguard, or Denuvo; PCGamingWiki lists "Anti-Cheat Expert" as
**exclusive to the China server build**, not the Steam version this project
targets ([PCGamingWiki: Warframe](https://www.pcgamingwiki.com/wiki/Warframe)).
Digital Extremes' own EULA confirms an unnamed in-house "Cheat Detection
Software" exists and can flag "unauthorized programs or processes," but every
documented signal about what it actually watches for (community-sourced,
not confirmed by DE) points to server-side statistical/anomaly checks and a
running-process list, not memory-read detection specifically — and on Linux,
a `process_vm_readv`/`/proc/[pid]/mem` read leaves no trace observable *by
the target process itself* (no ptrace stop, no `TracerPid` change, no
signal). Multiple read-only companion tools — AlecaFrame, WFHelper, and the
tool WFHelper itself delegates to (`Sainan/warframe-api-helper`, which reads
Warframe's process memory **on Linux today** via `/proc/[pid]/mem`) — have
operated for years without a documented, DE-confirmed ban wave, and DE staff
have specifically and separately blessed both AlecaFrame's read-only Overwolf
data access and WFInfo's screenshot-OCR approach on the official forums
(per secondary citations below — see the caveat under "Caveats and gaps").

The one real gap: **no official DE statement addresses memory-reading
specifically** (only Overwolf-mediated and screenshot-based tools have
explicit DE forum blessing) — that absence is itself worth weighing, not
papered over. Treat the finding as "no evidence of risk, with one
unaddressed axis," not "confirmed safe."

## 1. Anti-cheat presence on the Warframe Steam/PC build

**Confirmed: no EAC, BattlEye, Vanguard, or Denuvo on the build this project
targets.** PCGamingWiki's Warframe page lists, under "Middleware" →
"Anti-cheat": **"Anti-Cheat Expert" — "Exclusive to China server version"**
(fetched directly, 2026-08-10:
[pcgamingwiki.com/wiki/Warframe](https://www.pcgamingwiki.com/wiki/Warframe)).
Anti-Cheat Expert (ACE) is a Chinese-market anti-cheat product bundled only
with the Tencent-published China build — not the Steam/global build
`warframe-lite` runs against. The same page confirms the Steam/global build
runs under **Steam Play (Linux)** with a standard Proton compat-data prefix
(`<SteamLibrary-folder>/steamapps/compatdata/230410/pfx/`), i.e. this is an
ordinary Proton title with no anti-cheat-driven Linux blocklisting.

Digital Extremes' own EULA (fetched directly, 2026-08-10:
[warframe.com/en/eula-us](https://www.warframe.com/en/eula-us), Section 8,
"Ownership of the Services") confirms DE does run **some** in-house system,
referred to only generically:

> "The Services and/or the Cheat Detection Software may collect and transmit
> details about your Game Account, gameplay, and unauthorized programs or
> processes in connection with Cheating, subject to our Privacy Policy and
> applicable law."

No product name, mechanism, or detection method is given — "Cheat Detection
Software" is DE's own umbrella term, not a specific product. Community
discussion on the Warframe forums (title: "The Anti-Cheating System — A
Guide," [forums.warframe.com/topic/44016](https://forums.warframe.com/topic/44016-the-anti-cheating-system-a-guide-support-and-information/))
and a second thread specifically about what triggers it
([forums.warframe.com/topic/1403089](https://forums.warframe.com/topic/1403089-what-daily-use-software-can-trigger-the-anticheat-as-something-manipulating-the-memory/))
converge on: it runs primarily server-side, scanning end-of-mission metrics
for statistical anomalies, and separately checks a list of currently-running
program names (e.g. it's known to flag Cheat Engine, some macro tools, and
some VPNs) rather than doing endpoint memory-access instrumentation.

**Caveat:** both forum threads returned HTTP 403 (Cloudflare bot protection)
to every automated fetch attempt used here (WebFetch and a browser-UA
`curl`), so the above is a synthesis from search-engine-indexed excerpts of
those threads, not a verbatim primary-source read. Neither thread is
confirmed to carry an `[DE]`-tagged staff post — they read as
community-authored guides. A human should open both links directly before
treating "process-list check, not memory-read detection" as settled.

## 2. Detection mechanism: `process_vm_readv`/`/proc/[pid]/mem` on Linux

Per `process_vm_readv(2)`
([man7.org](https://man7.org/linux/man-pages/man2/process_vm_readv.2.html)):
permission to read another process's memory is governed by a
`PTRACE_MODE_ATTACH_REALCREDS` check — the same check `ptrace(PTRACE_ATTACH)`
uses. Critically, this is **only a permission check**, not an actual
`ptrace` attach: it doesn't stop the target, doesn't set `TracerPid` in
`/proc/[pid]/status`, and doesn't deliver any signal. The target process has
no OS-level mechanism to observe that its memory was read this way — there's
no equivalent of a debugger-attach event to notice. Reading via
`/proc/[pid]/mem` (a plain `read()` on that file, the method the prior-art
tool below actually uses) goes through the identical `PTRACE_MODE_ATTACH`
check and is equally invisible to the target.

Whether the check *succeeds* depends on the Yama LSM's `ptrace_scope`
sysctl. Per Fedora's own changelog docs
([fedoraproject.org: Restrict ptrace by default](https://fedoraproject.org/wiki/Changes/Restrict_ptrace_by_default))
and community write-ups
([linux-audit.com](https://linux-audit.com/protect-ptrace-processes-kernel-yama-ptrace_scope/)):
the upstream kernel default is `ptrace_scope=1` ("restricted": an unrelated,
same-UID process needs `CAP_SYS_PTRACE` or the target's explicit
`PR_SET_PTRACER` opt-in). Ubuntu, Arch, and openSUSE ship that default;
**Fedora ships `ptrace_scope=0`** ("classic": any same-UID process may
attach) via the `elfutils-default-yama-scope` package, which core Fedora
userspace depends on and which is installed on every Fedora system — this
matches the dev machine this research ran on (Fedora, per the environment
info). Either way, this governs whether the read *succeeds*, not whether the
target can *see* it happen — that answer (invisible) holds regardless of
`ptrace_scope`. Issue #10 already has this right: "same uid, needs
`ptrace_scope <= 1` or `CAP_SYS_PTRACE`," and separately recommends scoping
`CAP_SYS_PTRACE` to the binary via a file capability rather than lowering
`ptrace_scope` system-wide — that's the right call for staying inconspicuous
(a lowered system-wide sysctl is a bigger footprint than a capability on one
binary).

One more asymmetry worth naming: AlecaFrame runs as a native Windows process
*inside the same Windows environment* as `Warframe.x64.exe`, so if Warframe
(or Wine's Windows-API emulation of it) ever enumerated running processes via
something like `CreateToolhelp32Snapshot`, AlecaFrame would show up in that
listing. A native Linux reader process sits **outside** the Proton/Wine
prefix entirely — it isn't a Windows process at all, so it would not appear
in a Windows-style process enumeration the way AlecaFrame does today. This
is speculative (no evidence Warframe does such enumeration either way, per
§1's caveat) but is a structural difference worth flagging: the Linux
approach is, if anything, less visible on this specific axis than the
already-tolerated Windows prior art.

## 3. DE's EULA/ToS stance and official signals

The EULA itself draws no distinction between read-only helper tools and
cheats. Section 7 ("Player Conduct") prohibits, among other things:

> "use, or provide ancillary offerings to anyone, that are not offered
> within the Services by us...such as hosting, 'leveling' services,
> mirroring our servers, matchmaking, emulation, communication redirects,
> mods, hacks, cheats, bots (or any other automated control), trainers and
> automation programs"

and Section 8 gives DE unilateral discretion:

> "In the event that we in our sole discretion conclude that you are
> Cheating, you agree that we may exercise any or all of our rights under
> this EULA, including termination of this EULA and your access to our
> Services."

Read literally and in isolation, the EULA text alone gives no comfort — no
carve-out for "read-only" or "companion app" exists in the document. What
changes the picture is DE's *applied* behavior, documented (at one remove)
via two prior-art projects' own FAQs, each of which cites a specific DE
forum post:

- AlecaFrame's official docs state: **"The potentially risky part, getting
  your account/inventory data, is handled by Overwolf, which is explicitly
  said to be fine in DE's Third Party Policy"** — citing
  [forums.warframe.com/topic/1383123-third-party-software-usage/#comment-12964630](https://forums.warframe.com/topic/1383123-third-party-software-usage/#comment-12964630)
  (fetched via `docs.alecaframe.com/faq`, 2026-08-10:
  [docs.alecaframe.com/faq](https://docs.alecaframe.com/faq)).
- A separate, independent forum thread ("Is WFInfo still allowed by DE?",
  [forums.warframe.com/topic/1072535](https://forums.warframe.com/topic/1072535-is-wfinfo-still-allowed-by-de-and-if-anyone-is-using-it-is-it-up-to-date/))
  is reported (via search-engine synthesis, not a verbatim fetch — see
  caveat below) to contain a statement from **`[DE]Aidan`**, a then-DE staff
  forum presence, confirming WFInfo's screenshot-based approach does not
  result in bans.

**Caveat — this is the weakest-sourced part of this note.** Both
`forums.warframe.com` threads returned HTTP 403 to every fetch method tried
(WebFetch directly, WebFetch via `web.archive.org`, and `curl` with a
browser user-agent — all blocked, apparently by Cloudflare bot protection on
the forum software itself). Everything about `[DE]Aidan`'s WFInfo statement
and the "Third Party Software Usage" thread's Overwolf endorsement is a
search-engine-indexed excerpt or a downstream citation (AlecaFrame's own
docs quoting the thread), not a page this research verified by reading it
directly. **A human with normal browser access should open both links and
confirm the quotes before treating DE's stance as settled** — the citations
are specific enough (thread titles, a direct comment-anchor URL, a named
staff handle) to be worth the two minutes it'd take to check, but this note
cannot certify them as verbatim.

Separately, Overwolf's own support page
([support.overwolf.com: Overwolf won't get you banned](https://support.overwolf.com/en/support/solutions/articles/9000182312-overwolf-won-t-get-you-banned))
states Overwolf "work[s] directly with game publishers to ensure every app
in our official Appstore follows their Terms of Service" — but names Riot
and Ubisoft as examples, not Digital Extremes specifically. That page is
general Overwolf policy, not DE-specific evidence on its own; the
DE-specific claim rests on the forum-thread citation above.

**No DE statement of any kind — official or forum — was found addressing
direct process-memory reading specifically** (as opposed to Overwolf's
mediated access or WFInfo's screenshots). That's a real, unfilled gap in the
record, not something this research can resolve by more searching; it's
simply undocumented. Given that WFHelper's own inventory-fetch mechanism
(§4) already does exactly this today and remains operational, the practical
signal is "apparently tolerated," but that is an inference from absence of
enforcement, not a DE statement.

## 4. Prior art: technical approaches

Four tools were surveyed. In descending order of relevance to a Linux
`process_vm_readv` POC:

### `Sainan/warframe-api-helper` — reads Warframe's memory on Linux today

The strongest and most directly relevant prior art. Open source
([github.com/Sainan/warframe-api-helper](https://github.com/Sainan/warframe-api-helper),
C++, read directly from source 2026-08-10). Its entire approach, read from
`main.cpp`:

1. Finds the running `Warframe.x64.exe` process. Its Linux fallback —
   `Process::get("Warframe.x64.ex")`, one character short — exists
   specifically to match `/proc/[pid]/comm`'s 15-visible-character
   truncation of process names, which only matters on Linux/Proton. This is
   concrete evidence the tool is exercised against Warframe running under
   Wine/Proton on Linux, not just native Windows.
2. Enumerates the process's memory regions and pattern-scans them for the
   literal byte sequence `?accountId=` — the query-string fragment of an
   API call the game itself already makes.
3. Reads out the `accountId` and session `nonce` that immediately follow
   that marker in memory — a small, fixed-size token, not the full
   inventory structure.
4. Uses that token to call `mobile.warframe.com/api/inventory.php` directly
   — DE's own official mobile-companion-app API — and saves the JSON
   response locally.

This is read-only, reads only a short-lived auth token rather than
reverse-engineered inventory pointer chains, and gets the actual inventory
data from DE's own server rather than from parsed memory structures — which
sidesteps almost all of the "breaks on every game update" risk issue #10
flags, at the cost of only working for whatever fields DE's mobile API
exposes (not full foundry/riven-roll/stats-history detail).

The underlying cross-platform memory-access library, `calamity-inc/Soup`
([github.com/calamity-inc/Soup](https://github.com/calamity-inc/Soup),
`soup/ProcessHandle.cpp`, read directly from source 2026-08-10), confirms
the Linux implementation: it enumerates regions by parsing
`/proc/<pid>/maps` and reads memory by opening and seeking into
`/proc/<pid>/mem` directly — i.e., exactly the mechanism ADR-0001 and issue
#10 already scope this project to (`process_vm_readv`/`/proc/[pid]/mem`,
same-UID, no write path).

### WFHelper — open source, has a Linux beta, does not read memory itself

[github.com/WFHelper/WFHelper](https://github.com/WFHelper/WFHelper)
(README read directly 2026-08-10) is close prior art architecturally — a
desktop companion app with inventory, relics, foundry, mastery, and riven
tooling, explicitly including "an experimental Linux build" that finds
Warframe's `EE.log` inside the Proton prefix on its own, including Flatpak
and Snap Steam installs. However, WFHelper itself does **not** perform
memory reads for inventory: its README states "the game client offers no
local inventory API," and its first-run wizard instead offers three sources
— running the above `warframe-api-helper` tool, importing a JSON export, or
importing AlecaFrame's decrypted local cache (`lastData.dat`). WFHelper is
useful prior art for the surrounding app shape (Proton-prefix discovery,
Linux packaging as an AppImage, MIT-licensed, no telemetry) but the actual
memory read is delegated to `warframe-api-helper` above, not implemented in
WFHelper itself.

### AlecaFrame — Windows-only, Overwolf-mediated, architecture not public

[docs.alecaframe.com](https://docs.alecaframe.com/) confirms AlecaFrame is
read-only ("does not modify any game files/data or perform any in-game
actions, it just displays inventory data in a meaningful way") but the exact
memory-access mechanism is delegated to Overwolf's own native game-events
provider and isn't documented in AlecaFrame's own docs or its
`AlecaFrame-Docs` GitHub repo
([github.com/alecamaracm/AlecaFrame-Docs](https://github.com/alecamaracm/AlecaFrame-Docs) —
README checked directly, contains only doc-site build instructions, no
architecture detail). Search-engine synthesis (not independently verified
against a primary source) describes Overwolf's provider as opening
`Warframe.x64.exe` with `PROCESS_VM_READ`, walking memory via
`VirtualQueryEx`/`ReadProcessMemory`, and pattern-matching for a JSON blob
anchored on a `LastInventorySync` marker — plausible and consistent with
WFHelper's "no local inventory API" framing, but this detail should be
treated as unconfirmed until read from a primary source. AlecaFrame is
Windows/Overwolf-only; nothing here transfers to a Linux implementation
beyond the general shape (scan memory for a recognizable marker, extract
nearby structured data).

### Sentinel-for-Warframe — doesn't read memory at all

[github.com/calamity-inc/Sentinel-for-Warframe](https://github.com/calamity-inc/Sentinel-for-Warframe)
is a fourth tool by the same author/org as `warframe-api-helper` and `Soup`.
Per its README (fetched 2026-08-10), it does not read Warframe's memory
itself — it reads AlecaFrame's cached data file, so it requires AlecaFrame
to be installed. Included here only to note it's not independent
memory-reading prior art, despite showing up in searches for the topic.

### What doesn't transfer, and what does

Exact offsets and pointer chains are Windows/native-process-specific and
won't transfer to a Linux/Proton memory layout — no tool surveyed claims
otherwise, and none of the docs read here describe an offset-stability or
update-recovery strategy in any detail (a gap issue #10 already flags as the
real hard part of the full-inventory version of this feature). What *does*
transfer directly: the `warframe-api-helper`/`Soup` token-relay pattern
(scan for a small stable marker near a short-lived auth token, then call
DE's own API instead of parsing the full inventory structure) is a
concretely lower-effort, lower-maintenance target than parsing raw inventory
structures out of memory, and it's proven to work against Warframe on Linux
today, via source code this research read directly. Issue #10's own
recommended POC target — read one stable value (live credits) and confirm
it survives a loading-screen pointer invalidation — sits between these two
approaches in complexity, but the token-relay pattern is worth considering
as an alternative or a first milestone, since it reaches DE's own inventory
API without needing to reverse-engineer inventory structures at all.

## 5. Documented ban reports

**No confirmed, DE-attributed, or otherwise credibly sourced report of an
account ban specifically and verifiably caused by AlecaFrame, WFHelper,
WFInfo, `warframe-api-helper`, or Overframe was found**, despite these tools
being in wide, multi-year community use. What surfaced instead:

- A Steam Community discussion thread
  ([steamcommunity.com/app/230410/discussions/0/4630358389444061191](https://steamcommunity.com/app/230410/discussions/0/4630358389444061191),
  fetched 2026-08-10) contains one commenter's second-hand claim: "I've
  heard people getting banned for using AlecaFrame," which the same
  commenter frames as a "grey area" per an unnamed "developer" (no
  indication this was a DE developer rather than AlecaFrame's own). This is
  anecdotal hearsay, not a documented case, a named account, or a
  DE-confirmed cause.
- A separate, similarly unverified mention (via search-engine synthesis of
  other Steam Community threads on the same forum) claims "some YouTubers
  got banned from AlecaFrame" roughly eight months prior to that post, with
  support reportedly reversing the bans on investigation — again no primary
  source, name, or DE confirmation located.
- No Digital Extremes blog post, support article, patch note, or forum
  announcement describing an enforcement or ban wave targeting any of these
  tools was found in any search performed for this note.

Net: the absence of documented bans across years of tool use, combined with
the specific (if unverified — see §3's caveat) DE forum statements
tolerating both AlecaFrame's and WFInfo's approaches, is the strongest
positive signal this research turned up. It is an absence-of-evidence
argument, not a guarantee — appropriate weight for a go/no-go decision, not
proof of safety.

## Caveats and gaps, summarized

- Two Warframe forum threads central to the DE-stance question
  (`/topic/44016`, `/topic/1403089`, `/topic/1383123`, `/topic/1072535`)
  could not be fetched directly by any method available to this research
  (Cloudflare 403 on WebFetch, `web.archive.org`, and browser-UA `curl`
  alike) — all forum-derived claims here rest on search-engine-indexed
  excerpts or a downstream citation, not a verbatim primary-source read. A
  human should open these four links directly before finalizing a go/no-go
  decision.
- AlecaFrame's exact Windows memory-read mechanism (Overwolf's provider) is
  not documented in any AlecaFrame-controlled primary source found; the
  `ReadProcessMemory`/`LastInventorySync` detail is search-synthesized, not
  independently verified.
- No source — official or community — was found describing how DE's Cheat
  Detection Software would specifically react to a same-UID, read-only
  `process_vm_readv` client on Linux, because as far as this research could
  determine nobody has publicly documented trying it and been either
  banned or explicitly cleared. The `warframe-api-helper` precedent (§4) is
  the closest available signal, and it remains operational and open source
  as of this research.
