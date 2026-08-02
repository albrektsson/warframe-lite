# warframe-lite

A Linux-native, Overwolf-free companion app for Warframe: a `wlr-layer-shell`
overlay that shows live game timers and automatically picks the best relic
reward, built entirely from data the game exposes on its own — never by
reading or writing its process memory.

## Language

### Fissures & rewards

**Fissure**:
A timed Void mission that lets a player crack Relics for rewards. Comes in
three tiers, sorted normal → Steel Path → Storm.
_Avoid_: Void mission, relic mission.

**Relic**:
An item consumed during a Fissure to reveal a reward; each relic has a fixed
pool of possible rewards at different rarities.

**Relic crack**:
The moment a Relic is consumed at extraction, which opens the reward screen.
Detected from the `DVRCAftermath` marker in EE.log.

**Reward screen**:
The ~15-second, player-controlled UI (summonable via Tab) showing the 2–4
rewards a crack produced, from which the player picks one.
_Avoid_: Reward picker (that's the app's feature, not this screen).

**Built prime**:
The fully-built Warframe or weapon that a reward *part* belongs to. Mastery is
tracked per built prime, not per individual part — every part maps to one
before a mastery lookup.

**Mastery**:
A permanent status an item earns once its lifetime affinity crosses its
rank-30 cap; never resets on Forma. Read from DE's public profile API
(`getProfileViewingData`), which requires no authentication.
_Avoid_: Ranked, maxed (those describe the affinity climb, not the permanent
mastery state).

**Unmastered**:
A built prime whose lifetime affinity has not yet crossed the mastery cap —
the target of relic-cracking and fissure planning.

**Owned relic**:
A Relic the player currently holds, known only from OCR-scanning the in-game
Void Relics screen (see ADR-0001) — never read from process memory or a login
API. Ownership is a count per relic code (e.g. "Axi H3"), not per copy.
_Avoid_: Relic inventory (reads as authoritative/API-sourced; "owned relic"
keeps it clear the count is scan-derived and can lag or miss entries).

**Mastery plan**:
The ranked view of owned relics grouped by the Unmastered prime they can still
drop, sourcing relics ordered by owned count — the basis for deciding which
Fissures to run next.

**Farm pick**:
For an owned relic, its single highest-value **already-mastered** prime
reward — the basis for cracking the relic and selling that specific part
rather than selling the relic itself.

**Radiant share**:
A squad of four players who all bring copies of the same relic into a
Fissure, guaranteeing four independent reward rolls and maximizing the
chance that at least one lands the relic's most valuable drop. A Farm pick's
natural use: organize a radiant share around the relic that names it.

**Wishlisted part**:
A reward part the player has hand-marked as wanted, independent of its
mastery status. Unlike Owned relic or Mastery, this is player-declared intent
with no scan or API source behind it — the player is the source of truth (see
ADR-0004). Surfaces as its own marker on the reward screen, separate from the
mastery emblem.
_Avoid_: Favorite, tracked item.

### World state

**World state**:
The set of live, game-wide timers and statuses — Fissures, the Void Trader,
and the Cetus/Vallis/Cambion cycles — polled from `warframestat.us`.
_Avoid_: Live data, game status.

**Void Trader**:
Baro Ki'Teer, a vendor who visits on a recurring schedule; tracked as part of
world state.
_Avoid_: Baro (fine in conversation, but prefer "Void Trader" in code/UI so it
reads consistently with the other world-state entries).

**Cycle**:
A recurring day/night or weather rotation on an open-world zone (Cetus,
Orb Vallis, Cambion Drift); tracked as part of world state.
