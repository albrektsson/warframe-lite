# Riven/weapon community tier-ranking sources: does any expose a real API?

Research for [issue #97](https://github.com/albrektsson/warframe-lite/issues/97),
child of the wayfinder map [issue #94](https://github.com/albrektsson/warframe-lite/issues/94),
which is spec'ing a `wf-browse` riven tab: every Unveiled riven with decoded
stats, a warframe.market Floor/Ceiling price for its Riven type, an optional
community weapon-tier signal, and a computed Verdict ("likely dissolve" vs.
"likely keep"). The motivating example: overframe.gg rating a weapon like the
Javlok "C tier", which combined with a low market price would reinforce a
dissolve recommendation. This repo's existing data sources (`market.rs`,
`items.rs`, the WFCD packages, warframestat.us) all integrate through real
public APIs, never by scraping rendered HTML — a deliberate, load-bearing
constraint, not an oversight. This note checks whether any tier-ranking
source the community treats as authoritative clears that same bar.

## Question

Survey overframe.gg and any other riven/weapon tier-ranking source the
Warframe community treats as authoritative. For each: does it expose a real,
programmatically-fetchable API, or only rendered HTML? What do open-source
companion tools (WFHelper and others) actually fetch from, read from source?
For anything that clears the bar: report data shape (tier scale, per-weapon
vs. per-riven-type granularity, update-frequency signal) and any rate
limit/auth/ToS restriction. If nothing clears the bar, say so plainly and
note what that means for the Verdict feature.

## Answer, in short

**No source clears the API-only bar. overframe.gg — the specific site named
in the motivating example — has no public API**, documented or otherwise:
its own `robots.txt` explicitly disallows `/api/` for crawlers, direct
requests to `overframe.gg/api/` return HTTP 404, `api.overframe.gg` and
`docs.overframe.gg` don't resolve, no GitHub org/repo/OpenAPI spec exists,
and the site is Cloudflare bot-gated even for its public HTML (§1). A claim
circulating in search results that "the Overframe API, launched in late
2024" is used by third-party tools traces to a single unofficial fan/SEO
site with zero citations, whose named "third-party tool" turns out to be
another article on that same site — not corroborating evidence of a real
API (§1.3). **No open-source companion tool reads tier data from anywhere**:
WFHelper's riven code compares against warframe.market listings, not a
weapon-tier source, and no reference to overframe.gg or any tier API exists
anywhere in its source (§2). Every other candidate surveyed — Semlar's riven
calculators, RivenRadar, the Warframe wiki, TierMaker, and the SEO
tier-list-of-the-month sites — either isn't a tier-ranking source at all, is
a derived price signal warframe-lite could already compute from data it has,
or has no API and no claim to community authority (§3). **For the Verdict
feature, this means the community-tier cross-reference has no viable data
source today.** Per issue #94's "not yet specified" list, the Verdict should
degrade to a price-only signal (Floor/Ceiling from `market.rs`, no tier
cross-reference) unless the project's no-scraping policy is deliberately
revisited later — that tradeoff is noted here, not recommended.

## 1. overframe.gg

### 1.1 `robots.txt` — the strongest primary-source signal

Fetched directly, 2026-08-15 (`curl https://overframe.gg/robots.txt`):

```
User-agent: *
Content-Signal: search=yes,ai-train=no,use=reference
Allow: /

User-agent: Amazonbot
Disallow: /
User-agent: Applebot-Extended
Disallow: /
User-agent: Bytespider
Disallow: /
User-agent: CCBot
Disallow: /
User-agent: ClaudeBot
Disallow: /
User-agent: CloudflareBrowserRenderingCrawler
Disallow: /
User-agent: Google-Extended
Disallow: /
User-agent: GPTBot
Disallow: /
User-agent: meta-externalagent
Disallow: /

User-agent: *
Allow: /
Disallow: /api/
Disallow: /Lotus/
Disallow: /app/
Disallow: /overwolf/
Sitemap: https://overframe.gg/sitemap.xml

User-agent: grapeshot
Disallow: /
```

Two things matter here, read straight off the file:

- **`Disallow: /api/` applies to `User-agent: *`** — every crawler, not a
  named bot. This is the site itself declaring its own `/api/` path
  off-limits for automated/programmatic access, which is exactly the
  category a warframe-lite HTTP client would fall into.
- The `Content-Signal: search=yes,ai-train=no,use=reference` line, plus the
  blanket `Disallow: /` for every named AI/LLM crawler (including
  `ClaudeBot` by name) and the citation of "ARTICLE 4 OF THE EUROPEAN UNION
  DIRECTIVE 2019/790" (the EU's text-and-data-mining opt-out mechanism), is
  the site operator formally reserving rights against automated content
  reuse. Not itself a "no API" statement, but it reinforces that this site
  is actively asserting restrictions on programmatic consumption, not
  inviting it.

### 1.2 Direct probes — no public API surface exists

Checked directly, 2026-08-15:

| URL | Result |
|---|---|
| `overframe.gg/api/` | HTTP 404 |
| `overframe.gg/api` | HTTP 404 (redirects to `/api/`) |
| `overframe.gg/api/docs` | HTTP 404 |
| `api.overframe.gg` | connection failure — host doesn't resolve/respond |
| `docs.overframe.gg` | connection failure — host doesn't resolve/respond |
| `overframe.gg/` (plain `curl`) | HTTP 403 (Cloudflare bot-protection blocks non-browser requests) |
| `overframe.gg/` (via WebFetch's browser-like fetch) | loads; footer has "Terms of Service"/"Privacy Policy" links, **no link to API or developer docs anywhere on the page** |
| `overframe.gg/tier-list` | loads; a real S/A/B/C/D(+description) tier list exists, covering Warframes, primary/secondary/melee weapons, archwing, companions, and Helminth abilities — but **not rivens specifically**, and the fetched HTML carried no inline `__NEXT_DATA__` or JSON payload, consistent with the tier data being hydrated from a client-side call against the same `/api/` path `robots.txt` excludes |

No GitHub organization, repository, API client library, or OpenAPI/Swagger
spec for overframe.gg was found via GitHub search or web search. A search
engine's own summary of a browser-devtools-based writeup notes that
`static.overframe.gg/_next/static/chunks/db` contains bundled JS that
"parses a JSON string acting as some sort of database" — i.e. even
third-party observers describe extracting overframe.gg's data as inspecting
minified JS bundles and embedded HTML, not calling a documented endpoint.
That is scraping (of a JS bundle instead of rendered HTML, but the same
category of "not a stable, sanctioned integration point"), not an API.

### 1.3 The "Overframe API launched late 2024" claim — checked and rejected

Search results repeatedly surfaced one verbatim sentence: *"The Overframe
API, launched in late 2024, allows third-party tools like Warmarket and
Discord bots to pull real-time build data."* Tracing it to its source
(`warframegame.com/overframe/`, fetched directly 2026-08-15): the site's own
footer states *"This is an unofficial fan site"* / *"This site is not
affiliated with or endorsed by Digital Extremes"*, the article carries no
byline beyond a generic "Warframe Game India Team" credit, and **contains no
citation, link, or reference for the API claim at all** — it's a bare
assertion. Worse, the "third-party tool" it names as evidence, "Warmarket,"
resolves to `/warframe-new-update/` — **another article on that same
site**, not an independent product. No `docs.overframe.gg`, no GitHub
client, no Discord bot repo referencing an Overframe API, and no primary
Overframe-published documentation corroborates this claim anywhere. This
reads as unsourced SEO content (possibly LLM-generated, given how uniformly
the exact sentence propagates across unrelated low-quality domains that
turned up in search, including one dead/hijacked government subdomain) and
should not be treated as evidence overframe.gg has a real, intended-for-
third-parties API. Combined with §1.1–§1.2's direct findings (declared
off-limits, unreachable subdomains, no docs, 404 on the obvious path), the
conclusion is: **no such API exists in any form this research could verify.**

## 2. Open-source companion tools — what do they actually fetch?

### 2.1 WFHelper (`github.com/WFHelper/WFHelper`)

Read directly, 2026-08-15. The README's riven-relevant lines: *"Rivens -
your rivens with market comparison and a riven finder"* and *"Riven scanner
- reads rolls and compares old vs. new stats while rerolling."* Both are
**warframe.market price comparisons**, not weapon-tier lookups. No mention
of overframe.gg, "tier," or "grade" appears anywhere in the README. This is
consistent with what `docs/research/mobile-inventory-api-coverage.md`
(already in this repo) found reading WFHelper's actual riven-decoding source
(`services/rivenFingerprint.ts`): rivens are decoded from DE's own
`inventory.php` `UpgradeFingerprint` field, and pricing runs through
warframe.market — there is no third fetch to any weapon-tier source
anywhere in that pipeline. No evidence WFHelper pulls a community tier
ranking from overframe.gg or anywhere else.

### 2.2 Other companion tools checked

`Sainan/warframe-api-helper`, `calamity-inc/Sentinel-for-Warframe`, and
AlecaFrame were already read in full for the prior inventory-API research
note and none of them touch riven valuation or tier data at all — they stop
at the raw inventory payload. No new tool surfaced in this search that reads
tier-list data from any source.

## 3. Other candidate sources surveyed

| Candidate | What it is | API? | Verdict |
|---|---|---|---|
| [Semlar's Riven Calculator](https://semlar.com/rivencalc), [Comparator](https://semlar.com/comp), [Price Guide](https://semlar.com/rivenprices) | Client-side stat-range math and a community price guide, also mirrored at `browse.wf/rivencalc` | No server API — pure browser-side computation on user-entered values, no game-memory reading, no account login | Not a weapon-tier source at all (no S/A/B/C/D ranking, riven-attribute math only) — not applicable to this feature |
| [RivenRadar](https://rivenradar.com) | Computes a riven "grade" by pulling live warframe.market auction listings and bucketing comparable rolls | Consumes warframe.market's own API (the same one `market.rs` already calls), no independent tier-ranking API of its own | Not a distinct source — its "grade" is a derived price signal warframe-lite could compute itself from data it already has, not a third-party community-tier verdict |
| Warframe Wiki (`wiki.warframe.com`, Fandom-hosted MediaWiki) | The community's reference wiki, including a Weapon Disposition list | **Yes, a real API** — the standard MediaWiki Action API at `api.php` | Clears the "real API" bar technically, but returns raw wiki markup/page data, not a maintained tier verdict. The one relevant table (Riven Weapon Disposition) is DE's own objective disposition value, not a subjective community "S/A/B/C/D" ranking — different signal than what the Javlok example asks for. Not evaluated further here since it doesn't answer the tier question; flagged only because it does clear the API bar on a technicality |
| TierMaker "Riven Attribute Tier List" | User-submitted, crowd-voted tier template | No API — TierMaker is a generic template-tiermaker platform with no Warframe-specific infrastructure | Not community-authoritative in the curated sense (anyone can submit a vote), no API — scraping-only if used at all, and not worth using even if scraping were allowed |
| SEO tier-list articles (AxeeTech, BoundByFlame, Boostmatch, Sportskeeda, Attack of the Fanboy, warframegame.com) | Periodically-republished editorial "tier list" guides | No API — static HTML content-mill articles | Not treated as authoritative by the community itself; several appear to recycle the same handful of underlying claims (see §1.3). Excluded on both API and authority grounds |

No other candidate surfaced in this search (community wikis beyond the one
above, other riven pricing sites, Discord-bot ecosystems) exposes a
maintained, community-authoritative weapon-tier ranking through a real API.

## What this means for the Verdict feature

Per issue #94, the Verdict's community-tier signal was explicitly listed as
"not yet specified... depends on whether any tier source clears the
API-only bar." It doesn't. Concretely, for the `wf-browse` riven tab spec:

- The Verdict signal should be built from **Floor/Ceiling price alone**
  (`market.rs`-style warframe.market data, per issue #94's existing scope),
  with no community-tier cross-reference term.
- This is not a stopgap pending a better search — every plausible candidate
  a Warframe player would call "the tier list" (overframe.gg by name, plus
  everything adjacent) was checked directly against source, and none offers
  a documented, intended-for-third-parties API. The remaining path to a
  tier cross-reference would be scraping overframe.gg's rendered HTML or its
  internal `/api/` (which its own `robots.txt` disallows and Cloudflare
  actively gates) — which this project's existing policy rules out, and
  this note isn't recommending revisiting that policy, just naming the
  tradeoff: **without it, the Verdict is price-only; with it, the app would
  be scraping a site that has explicitly reserved rights against exactly
  that kind of automated reuse.**

## Caveats and gaps

- overframe.gg's HTML was reachable only through the sandboxed WebFetch
  tool, not direct `curl` (Cloudflare returned HTTP 403 to a plain request);
  this research cannot rule out that a differently-configured client (real
  browser headers, session cookies) might reach further pages than were
  checked here, though this doesn't change the `robots.txt`/`/api/` 404
  findings, which came from direct requests.
- No attempt was made to log into an overframe.gg account or the desktop
  `app.overframe.gg` client, in case either exposes a different,
  authenticated API surface than the anonymous public site does — out of
  scope for "real public API" as issue #97 framed the question, but worth
  naming as an unexplored corner.
- The MediaWiki API on `wiki.warframe.com` (§3) was confirmed to exist by
  general knowledge of the MediaWiki platform and corroborating search
  results, not by directly calling `api.php` and inspecting a response in
  this session — if a future ticket wants disposition data specifically
  (not the tier-ranking question this note answers), that endpoint deserves
  its own direct verification pass.
- This note did not exhaustively enumerate every SEO/blog tier-list site in
  existence — a representative, high-ranking sample was checked and all
  shared the same disqualifying properties (no API, no claim to curated
  community authority). A different specific site could in principle be
  raised later, but the pattern found here (editorial content-mill articles,
  frequently recycling unsourced claims) makes it unlikely a new one would
  change the conclusion.
