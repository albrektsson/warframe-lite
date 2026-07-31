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
