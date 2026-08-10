# Coverage of `mobile.warframe.com/api/inventory.php`

Research for [issue #51](https://github.com/albrektsson/warframe-lite/issues/51),
child of the wayfinder map [issue #49](https://github.com/albrektsson/warframe-lite/issues/49),
which is charting a go/no-go decision on Phase 4 full inventory support
(issue #10) around the token-relay technique: scan Warframe's process memory
for a `?accountId=` marker, extract the session nonce, and call DE's own
mobile-companion-app endpoint directly rather than reverse-engineering raw
inventory pointer chains. Issue #10's original ambition was *full* inventory
— foundry, riven details, credits/plat, stats history — and nobody had yet
checked what the endpoint itself actually returns.

## Question

Does DE's mobile inventory API (`mobile.warframe.com/api/inventory.php`)
cover the breadth of data issue #10 wanted (foundry, riven details,
credits/plat, stats history), or only a thin slice — and what does that mean
for the go/no-go decision?

## Answer, in short

**It is not a thin slice — it is DE's own full internal account-inventory
dump, the same shape the desktop client itself syncs, and it covers three of
issue #10's four asks directly.** Read from source (not guessed): **foundry**
is covered (`PendingRecipes[]` gives the exact in-progress build with a
`CompletionDate`, `Recipes[]` gives owned/uncrafted blueprints); **riven
details** are covered in full fidelity (`Upgrades[].UpgradeFingerprint`
carries the exact buff/curse tags, roll values, polarity, mastery
requirement, and reroll count DE itself uses to render a riven — not an
approximation); **credits/platinum** are covered as a live balance snapshot
(`RegularCredits`, `PremiumCredits` fields read directly off the payload).
**Stats/mission history is the one genuine gap**: the endpoint returns a
single point-in-time snapshot with no time-series or transaction log: the
"stats history" WFHelper shows is not something the API returns, it's
WFHelper polling this same endpoint every ~10 minutes and diffing the
balances itself, persisted to its own local `stats-history.json`. The
payload does, however, carry several *other* fields adjacent to "history"
that issue #10 didn't originally ask for — sortie/Archon Hunt completion,
weekly Ayatan Treasure Hunt completion, and Netracell weekly pulse counts —
so "history" specifically is thin, but "account progress state" is broader
than a plain inventory list. Net for the go/no-go call: **go, with the
caveat that any "stats over time" feature needs the app to build and persist
its own polling history exactly as WFHelper does — DE's endpoint alone
cannot supply it.**

One additional wrinkle surfaced that matters directly for the live POC
(issue #52): WFHelper's own maintainers found `mobile.warframe.com`'s
inventory response can come back an **empty HTTP 200**, and fall back to
querying `api.warframe.com` with the identical authz string, which works —
see §5.

## 1. `Sainan/warframe-api-helper` — does it save the raw response, and where?

Read directly from source, 2026-08-10:
[github.com/Sainan/warframe-api-helper, `main.cpp`](https://raw.githubusercontent.com/Sainan/warframe-api-helper/senpai/main.cpp)
(default branch is `senpai`, not `main`). The relevant tail of `main()`:

```cpp
// Note: Could also use api.warframe.com
HttpRequest hr("mobile.warframe.com", "/api/inventory.php" + authz);
auto res = hr.execute();
...
auto inventory = std::move(res->body);
auto jr = json::decode(inventory);
...
string::toFile("inventory.json", jr->encodePretty());
aes::pkcs7Pad(inventory);
aes::cbcEncrypt(reinterpret_cast<uint8_t*>(inventory.data()), inventory.size(), key, 16, iv);
string::toFile("lastData.dat", inventory);
```

Yes — it saves the raw JSON response twice: pretty-printed to
`inventory.json`, and AES-128-CBC-encrypted to `lastData.dat` using a
hardcoded key (`4C 45 4F 2D 41 4C 45 43 09 45 4F 2D 41 4C 45 43`, i.e. the
ASCII bytes of `"LEO-ALEC\tEO-ALEC"`). That key/IV pair is not incidental —
it exactly matches the format AlecaFrame's own local cache file uses (see
§4), which is why `calamity-inc/Sentinel-for-Warframe` can read either
tool's output interchangeably (§4). The repo ships no README and no example
JSON fixture; the schema had to be reconstructed from a consumer (WFHelper)
instead — see §2.

The comment `// Note: Could also use api.warframe.com` is the author noting
both hosts accept the same authz — this becomes concretely important in §5.

## 2. WFHelper — the fullest schema reconstruction available

WFHelper doesn't shell out to `warframe-api-helper` and only read its output
loosely — it **reimplements the same fetch itself** in
[`services/apiHelperRunner.ts`](https://raw.githubusercontent.com/WFHelper/WFHelper/main/services/apiHelperRunner.ts)
(read directly, 2026-08-10), spawning the upstream helper only to extract
the authz string from its stdout via regex
(`/\?accountId=[a-f0-9]+&nonce=\d+/i`), then making its own HTTPS GET and
saving the raw bytes:

```ts
/** GET inventory.php with the helper-extracted authz. Tries api.warframe.com first. */
async function fetchInventoryWithAuthz(authz: string, destPath: string): Promise<void> {
  const hosts = ["api.warframe.com", "mobile.warframe.com"];
  ...
  for (const host of hosts) {
    const url = `https://${host}/api/inventory.php${authz}`;
    const res = await httpsGetBuffer(url, headers, MAX_INVENTORY_RESPONSE_BYTES);
    if (res.statusCode === 200 && res.body.length > 0) {
      fs.writeFileSync(destPath, res.body);
      ...
```

Everything downstream in WFHelper parses this exact raw JSON (or the
identically-shaped `lastData.dat` from AlecaFrame/`warframe-api-helper`, via
`config/shared/inventoryPayload.ts`'s envelope-unwrapping helper, which
handles both a bare object and one nested under `InventoryJson`/`payload`/
`data` keys). The type this settles into,
[`src/types/inventory.ts`'s `RawInventoryData`](https://raw.githubusercontent.com/WFHelper/WFHelper/main/src/types/inventory.ts)
(read directly, 2026-08-10), documents the field names:

```ts
export interface RawInventoryData {
  InventoryJson?: RawInventoryData | string;
  Suits?: RawInventoryEntry[];       // Warframes
  LongGuns?: RawInventoryEntry[];    // Primaries
  Pistols?: RawInventoryEntry[];     // Secondaries
  Melee?: RawInventoryEntry[];
  Sentinels?: RawInventoryEntry[];
  SentinelWeapons?: RawInventoryEntry[];
  SpaceSuits?: RawInventoryEntry[];  // Archwings
  SpaceGuns?: RawInventoryEntry[];
  SpaceMelee?: RawInventoryEntry[];
  OperatorAmps?: RawInventoryEntry[];
  MechSuits?: RawInventoryEntry[];   // Necramechs
  PendingRecipes?: RawInventoryEntry[]; // Foundry: in-progress builds
  Recipes?: RawInventoryEntry[];        // Foundry: owned blueprints
  MiscItems?: RawInventoryEntry[];      // Endo, Ducats, Aya, Vitus, Forma, resources...
  LevelKeys?: RawInventoryEntry[];      // Relics
  RawUpgrades?: RawInventoryEntry[];    // Veiled/stackable rivens
  Upgrades?: RawInventoryEntry[];       // Unveiled rivens + other mods, w/ UpgradeFingerprint
  Arcanes?: RawInventoryEntry[];
  [key: string]: unknown;
}
```

Each entry carries `ItemType`, `ItemCount`, and — critically for mastery —
`XP`. This is the same field-name convention DE uses internally across its
inventory-sync APIs (it is not WFHelper's own invention); the `[key:
string]: unknown` catch-all is there because the payload carries many more
top-level keys than WFHelper chooses to model (confirmed further in §4).

## 3. Coverage of issue #10's four asks

### Foundry — covered

[`src/lib/inventory/foundryResources.ts`](https://raw.githubusercontent.com/WFHelper/WFHelper/main/src/lib/inventory/foundryResources.ts)
(read directly) builds the in-progress build list straight from
`PendingRecipes`, including the exact completion timestamp:

```ts
for (const recipe of data.PendingRecipes || []) {
  ...
  endDate: parseCompletionDate(recipe.CompletionDate),
```

and [`config/shared/foundryPending.ts`](https://raw.githubusercontent.com/WFHelper/WFHelper/main/config/shared/foundryPending.ts)
cross-references `PendingRecipes` against `Recipes` to net out
already-committed blueprint copies (a blueprint stays in `Recipes` until its
build is claimed). The only thing *not* in the payload is static recipe
metadata (ingredient list, build price/time) — WFHelper joins that in from a
separate item database, which is expected: that's game-content data, not
player state, and warframe-lite already has an equivalent source (WFCD
`warframe-items`, per [ADR-0011](../adr/0011-warframe-items-for-prime-part-build-quantities.md)).

### Riven details — covered, in full fidelity

[`services/rivenFingerprint.ts`](https://raw.githubusercontent.com/WFHelper/WFHelper/main/services/rivenFingerprint.ts)
(read directly) decodes rivens straight out of `Upgrades[].UpgradeFingerprint`
— its own top comment says exactly that: `Decode riven stats from inventory
UpgradeFingerprint`. The fingerprint (a JSON string embedded in the field)
carries `compat` (weapon unique name), `pol` (polarity), `lvl`/`lvlReq`
(current rank / mastery requirement), `rerolls`, and a `buffs`/`curses`
array of `{Tag, Value}` pairs — the exact same encoded roll values the game
client itself uses to render a riven's stat lines, not a lossy summary:

```ts
return {
  itemId: entry.ItemId?.$oid || "",
  weaponName,
  weaponUniqueName: fp.compat,
  rivenName,
  masteryReq: typeof fp.lvlReq === "number" ? fp.lvlReq : 0,
  currentRank: lvl,
  maxRank: 8,
  rerolls: typeof fp.rerolls === "number" ? fp.rerolls : 0,
  ...
```

Veiled rivens (not yet identified in-game) are separately covered:
unveiled/single rivens live in `Upgrades[]` with a fingerprint that fails
the `compat` check (`isVeiledFingerprint`), and stackable veiled rivens
(multiple copies of the same unidentified riven) live in `RawUpgrades[]`
with no fingerprint at all — both are read and surfaced by WFHelper as
"veiled" entries.

### Credits / platinum — covered, as a snapshot

[`services/statsTracker.ts`](https://raw.githubusercontent.com/WFHelper/WFHelper/main/services/statsTracker.ts)
(read directly) reads both balances straight off the same payload:

```ts
const plat    = _num(data.PremiumCredits);
const credits = _num(data.RegularCredits);
```

This confirms both are present as top-level scalar fields on every
`inventory.php` response — a live point-in-time balance, not a ledger of
transactions.

### Stats / history — not covered natively; the app has to build it

This is the one place the endpoint falls short of "history" in the sense
issue #10 meant it. `statsTracker.ts`'s entire purpose is to **synthesize**
history WFHelper's own way: on each poll (WFHelper's default interval is 10
minutes, per `apiHelperRunner.ts`'s `DEFAULT_POLL_INTERVAL_MS`) it reads the
current `RegularCredits`/`PremiumCredits`/Endo/Ducats/Aya/Vitus balances,
diffs them against an in-memory baseline, and upserts a per-day delta entry
into its own locally-persisted `stats-history.json`
(`app.getPath("userData") + "/stats-history.json"`, written via
`writeFileAtomicSync`). There is no field anywhere in `RawInventoryData` or
in what WFHelper models that represents a time series, a transaction log, or
mission-by-mission history — `inventory.php` is a snapshot endpoint, full
stop. Any "stats over time" feature in warframe-lite would need the same
poll-and-diff approach WFHelper uses, not a field read off one response.

## 4. `calamity-inc/Sentinel-for-Warframe` — extra fields beyond issue #10's ask

[`Inventory.cpp`/`Inventory.hpp`](https://raw.githubusercontent.com/calamity-inc/Sentinel-for-Warframe/senpai/Inventory.cpp)
(read directly, 2026-08-10; repo default branch is `senpai`) is a second,
independent consumer of this exact payload shape — it reads AlecaFrame's
`%localappdata%\AlecaFrame\lastData.dat` (AES-decrypted with the identical
key/IV `warframe-api-helper` uses, confirming both tools' output is
byte-for-byte the same JSON schema regardless of how the authz was obtained)
and unwraps the same `InventoryJson` envelope WFHelper's
`unwrapInventoryPayload` also handles. Sentinel-for-Warframe surfaces fields
WFHelper doesn't bother modeling, which is useful evidence the payload is
broader than either single consumer's model:

```cpp
bool Inventory::hasCompletedLatestSortie(const std::string& oid)      // reads "LastSortieReward"
bool Inventory::hasCompletedLatestArchonHunt(const std::string& oid)  // reads "LastLiteSortieReward"
time_t Inventory::getLastAyatanTreasureHuntCompletion()                // reads "PeriodicMissionCompletions"
int Inventory::getUsedNetracellSearchPulses()                          // reads "EntratiVaultCountLastPeriod"
time_t Inventory::getNetracellResetTime()                              // reads "EntratiVaultCountResetDate"
```

It also reads a top-level `XPInfo` array (`{ItemType, XP}` pairs) as an
alternate/aggregate source of per-item mastery affinity, separate from the
per-entry `XP` field WFHelper reads directly off `Suits`/`LongGuns`/etc.
entries — both appear to carry the same information from two angles.

None of these five fields were on issue #10's original wishlist (they're
weekly-reset progress markers, not "stats history" in the graphed sense),
but they demonstrate the payload is closer to DE's full internal
account-state dump than a purpose-built "companion app inventory slice"
would be — nothing here reads as deliberately pared down for the mobile
client.

`calamity-inc/Soup` was also checked directly (`ProcessHandle.cpp`) but adds
nothing schema-relevant beyond what
[the prior ban-risk research note](memory-reading-ban-risk-and-prior-art.md)
already covered — it's the cross-platform `/proc/[pid]/mem` access library
underlying `warframe-api-helper`, not an inventory schema source.

## 5. A wrinkle for the live POC: `mobile.warframe.com` vs `api.warframe.com`

`apiHelperRunner.ts` contains a comment explaining *why* WFHelper's
reimplementation tries `api.warframe.com` first and falls back to
`mobile.warframe.com`, not the reverse:

```ts
// Don't gate on exit code: helper's own HTTP request to mobile.warframe.com
// returns empty 200s, but the authz it prints is still valid against
// api.warframe.com, so we fetch ourselves.
```

This is a direct, source-level claim (not a guess) that
`mobile.warframe.com/api/inventory.php` can return an **empty HTTP 200
body** for a valid authz in at least some observed conditions, while the
identical authz against `api.warframe.com/api/inventory.php` succeeds. The
root cause isn't stated (rate limiting, host-specific auth quirks, or
something else) and no other source corroborates or explains it further.
This directly affects issue #52 (the live POC): if the from-scratch Rust
implementation targets `mobile.warframe.com` only and gets an empty
response, that isn't necessarily proof the token-relay approach failed —
`api.warframe.com` with the same authz is worth trying as a fallback before
concluding the POC didn't work.

## 6. Cross-reference against warframe-lite's domain model

Per `CONTEXT.md` and issue #10's original list:

| Issue #10 ask | Covered by `inventory.php`? | Evidence |
|---|---|---|
| Foundry | **Yes** | `PendingRecipes[]` (in-progress, with `CompletionDate`), `Recipes[]` (owned blueprints) — §3 |
| Riven details | **Yes, in full fidelity** | `Upgrades[].UpgradeFingerprint` — exact buffs/curses/values/polarity/mastery req/rerolls, same encoding the game client uses — §3 |
| Credits / platinum | **Yes, as a live snapshot** | `RegularCredits`, `PremiumCredits` top-level fields — §3 |
| Stats / mission history | **No — snapshot only** | No time-series or transaction-log field found anywhere in either consumer's model; WFHelper builds it itself by polling+diffing — §3 |

Two things not on issue #10's list but relevant to warframe-lite's existing
domain model:

- **Mastery**: warframe-lite's `CONTEXT.md` currently sources Mastery from
  DE's public `getProfileViewingData` API (no auth required). WFHelper's
  `services/masteryHelper.ts` shows `inventory.php` *also* carries mastery
  data — both a `MasteryRank`/`PlayerLevel`/`LevelInfo` account-level field
  and per-item `XP` on every equipment entry — so this endpoint would be
  redundant with, not a replacement for, the existing mastery source; no
  new capability there.
- **Owned relics**: warframe-lite currently sources this from OCR-scanning
  the in-game Void Relics screen (ADR-0009), explicitly because no API
  source exists. `inventory.php`'s `LevelKeys[]` array is exactly the
  authoritative relic-ownership data that scan is standing in for — worth
  flagging as a potential future consideration if Phase 4 goes ahead, though
  it is out of scope for this ticket's question.

## Caveats and gaps

- No source read here includes a full, unredacted example JSON response —
  every field name above was reconstructed from what two independent open-
  source consumers (WFHelper, Sentinel-for-Warframe) chose to read out of
  the payload, not from a raw fixture or DE documentation. There is very
  likely payload structure neither consumer bothers to model (both are
  purpose-built companion apps, not schema documentation projects) — this
  research cannot rule out additional fields, nor can it fully verify field
  *types* (e.g. whether `CompletionDate` is a Unix timestamp, a Mongo
  `$date` object, or something else) beyond what each consumer's parsing
  code implies.
- No public reverse-engineering writeup, teardown, or archived documentation
  of DE's mobile companion app API was found beyond the two source repos
  above — searches for "warframe mobile api inventory.php schema" and
  similar turned up only the same WFHelper/AlecaFrame/`warframe-api-helper`
  ecosystem already covered, no independent third-party documentation.
- The `mobile.warframe.com` vs `api.warframe.com` empty-response behavior
  (§5) is asserted by one source comment with no reproduction steps or root
  cause — worth the live POC (#52) explicitly testing both hosts and noting
  which one actually works, rather than assuming the ticket's named host is
  sufficient.
- Given the above, **the live POC (#52) should still capture and save the
  raw response itself** (as both `warframe-api-helper` and WFHelper already
  do, to `inventory.json`) rather than relying on this research as the final
  word on schema — this note narrows the open question from "what does this
  endpoint return at all" to "confirm the specific field names/types above
  against a real response," which is a much smaller and well-scoped ask for
  that ticket.
