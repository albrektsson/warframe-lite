# Fetch per-category warframe-items files for Prime Part build quantities

Showing how many of each Prime Part a build requires (e.g. Afuris Prime needs
2 Barrel, 2 Receiver, 1 Link) needs quantity data that `warframe-drop-data`'s
`relics.json` doesn't carry — only WFCD's separate `warframe-items` dataset
has per-component `itemCount`. We fetch it as a handful of per-category files
(`Warframes.json`, `Primary.json`, `Secondary.json`, `Melee.json`,
`Sentinels.json`, `Archwing.json`, etc. — whichever categories contain Prime
items) rather than the combined `All.json`, since that single file is
~54.5MB, roughly 150x larger than any file this app already caches
(`relics.json` is 311KB); the per-category files are a few MB each, in line
with the rest of the app's cache footprint. When a quantity lookup misses
(fetch failure, category mismatch, item not found), the Relics & Plan tab
omits the quantity rather than defaulting to 1 — showing an unverified 1 as
if it were certain would clash with the app's existing trust model (e.g. Seen
vs Confirmed relic counts, ADR-0009) of never presenting unverified data as
fact.
