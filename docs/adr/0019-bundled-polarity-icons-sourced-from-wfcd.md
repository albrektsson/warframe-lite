# Bundled polarity icons come from WFCD/genesis-assets, not the wiki

Displaying **Polarity** (ADR-0018) as an icon needs actual image files, which
this repo doesn't have. Two candidate sources exist: `wiki.warframe.com`
serves the five icons as individual SVGs (`Special:FilePath/Madurai_Pol.svg`
etc.) but the wiki's content license is CC BY-NC-SA; WFCD's
`genesis-assets` repo (the same community org behind the `warframe-items`
dataset this project already depends on for **Build quantity**, ADR-0011)
bundles the same five as PNGs under its own Apache 2.0 project license.

We use WFCD's PNGs. They match the precedent already set by this project's
two existing bundled DE-owned assets (`crates/wf-overlay/assets/mastered.png`,
`assets/relic-unowned-eye.png`): a permissively-licensed redistribution of
DE-owned artwork by a known community project, disclaimed in the README the
same way. The wiki's NonCommercial clause is a worse fit for that pattern
even though this project is non-commercial in practice.

## Scope

New `crates/wf-browse/assets/polarity/{madurai,vazarin,naramon,zenurik,unairu}.png`,
loaded via the same `include_bytes!` + `OnceLock` pattern as the existing
bundled assets; README's License section gains the new files under its
existing Digital-Extremes-ownership disclaimer.
