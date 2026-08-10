# Phase 4 inventory scope: relics/equipment/mastery slice only

DE's `inventory.php` response covers far more than this app's existing scope
(CONTEXT.md; [ADR-0007](0007-live-world-state-out-of-scope.md)) — credits and
platinum, sortie/Archon Hunt/Ayatan/Netracell weekly progress, full account
state. Phase 4 (go/no-go decided in
[issue #53](https://github.com/albrektsson/warframe-lite/issues/53)) commits
only to the fields that extend the app's existing relics/equipment/mastery
focus: Foundry (`PendingRecipes`/`Recipes`, in-progress Prime builds), riven
details (`Upgrades[]`), `LevelKeys[]` (a candidate future authoritative
replacement for the OCR-based Owned relic scan, [ADR-0009](0009-seen-is-separate-from-confirmed-count.md)),
and Prime/weapon ownership for mastery. Credits/plat, sortie/Archon
Hunt/Ayatan/Netracell progress, and stats/mission history (the endpoint has
no time-series, so this would need a separate poll-and-diff build) are
explicitly out of scope — unlocked only by what the endpoint happens to
return, not wanted features.
