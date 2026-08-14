//! warframe-lite — command-line entry point.
//!
//! Orchestrates the overlay, relic picker, mastery, and live-Fissure/market
//! lookups. Running the binary with **no command runs `tray`** (the tray
//! icon, which auto-starts the overlay when the game appears); `help`/`-h`/
//! `--help` still prints usage ([`print_help`]). Every subcommand is
//! dispatched from `main`. `status` shows live Void Fissures and a bare
//! `<market_slug>` prices that item.
//!
//! `tray`/`browse` run the [`wf_tray`]/[`wf_browse`] crates in-process —
//! they're linked into this binary as libraries, not spawned as
//! separately-installed sibling binaries (see #69). The one place a real
//! child process is still spawned for these subsystems is `wf_tray`'s own
//! overlay supervision, which re-execs this same binary
//! (`std::env::current_exe()`) with the `overlay` subcommand so the overlay
//! can be started/stopped/crash-isolated independently of the tray.
//!
//! `settings` is a harmless alias for `browse` (#72): the standalone
//! settings window was folded into `wf-browse`'s tab bar as a Settings tab,
//! so `wf-lite` no longer depends on the `wf-settings` crate at all — it
//! stays in the workspace as a standalone, independently
//! buildable/embeddable crate (`cargo run -p wf-settings`), just not wired
//! into this binary's command dispatch.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use wf_config::Config;
use wf_data::{http_client, market::MarketClient, worldstate};

// The real OCR/relic-grid-scan pipeline (`ocr_enabled.rs`) when the `ocr`
// cargo feature is compiled in, or a friendly "not compiled in" stand-in
// (`ocr_disabled.rs`) otherwise — see the root Cargo.toml's `ocr` feature
// and README.md. Both expose the same `pub(crate)` surface, so dispatch
// below and `run_overlay` never need their own `#[cfg]` branches.
#[cfg(feature = "ocr")]
#[path = "ocr_enabled.rs"]
mod ocr;
#[cfg(not(feature = "ocr"))]
#[path = "ocr_disabled.rs"]
mod ocr;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "wf_lite=info,wf_data=info,wf_config=info,wf_log=info,wf_overlay=info,wf_relic=info,wf_ocr=info,wf_tray=info,wf_browse=info".into()
            }),
        )
        .with_target(false)
        .init();

    // An explicit help flag prints usage instead of running anything, so it
    // stays reachable. No command (bare `wf-lite`) no longer does this — see
    // the `tray`/`None` arm below, which makes today's `tray` behavior (tray
    // icon, auto-starts the overlay when the game appears) the default entry
    // point instead (#69).
    match std::env::args().nth(1).as_deref() {
        Some("help") | Some("-h") | Some("--help") => {
            print_help();
            return Ok(());
        }
        _ => {}
    }

    // --- Config + EE.log detection ---------------------------------------
    let config_path = Config::default_path()?;
    let config = Config::load(&config_path)?;
    println!("config:   {}", config_path.display());
    let ee_log = match config.resolve_ee_log() {
        Ok(p) => {
            println!("EE.log:   {}", p.display());
            Some(p)
        }
        Err(e) => {
            println!("EE.log:   NOT FOUND ({e:#})");
            None
        }
    };

    // --- Log subcommands --------------------------------------------------
    match std::env::args().nth(1).as_deref() {
        Some("logstats") => return log_stats(ee_log),
        Some("logwatch") => return log_watch(ee_log),
        Some("capture") => return capture_window(std::env::args().nth(2)),
        Some("overlay-png") => return overlay_png(&config, std::env::args().nth(2)).await,
        Some("overlay") => return run_overlay(config).await,
        Some(cmd @ ("toggle" | "show" | "hide" | "copy")) => return overlay_control(cmd),
        Some("ocr") => return ocr::ocr_test(),
        Some("ocr-file") => return ocr::ocr_file(),
        Some("relic-file") => return ocr::relic_file(&config).await,
        Some("mastery") => return mastery_cmd(&config).await,
        Some("set-account") => return set_account_cmd(&config_path),
        Some("detect-account") => return detect_account_cmd(&config, &config_path).await,
        // `settings` now opens the same browse window as `browse` (#72):
        // Settings became a tab inside `wf-browse` rather than its own
        // window, so this is a harmless alias for any existing muscle-memory
        // or scripts — it doesn't auto-select the Settings tab, landing on
        // the new default Home tab instead, same as `browse`.
        Some("settings") | Some("browse") => return run_browse(),
        // No subcommand behaves like `tray`: launch the tray, which
        // auto-starts the overlay when the game window appears (#69).
        None | Some("tray") => return wf_tray::run().await,
        Some("relic") => return relic_eval(&config).await,
        Some("relics") => return relics_cmd(&config).await,
        Some("mastery-plan") => return mastery_plan_cmd(&config).await,
        Some("relic-guide-png") => return relic_guide_png(&config).await,
        Some("relic-grid-file") => return ocr::relic_grid_file().await,
        Some("inventory-grid-file") => return ocr::inventory_grid_file().await,
        Some("relic-scan") => return ocr::relic_scan(&config).await,
        Some("reward-png") => return reward_png(&config).await,
        #[cfg(feature = "mem-scan")]
        Some("mem-scan") => return mem_scan_cmd(&config).await,
        _ => {}
    }

    let client = http_client();

    // --- Live Fissures ----------------------------------------------------
    println!("\n== Fissures ({}) ==", config.platform);
    match worldstate::fetch(&client, &config.platform).await {
        Ok(ws) => print_fissures(&ws),
        Err(e) => println!("  fissure fetch failed: {e:#}"),
    }

    // --- Optional market lookup ------------------------------------------
    // Reached by `status` (fissures only) or a bare `<slug>` (fissures +
    // that item's price); every other command returned from the match above.
    if let Some(slug) = std::env::args().nth(1).filter(|a| a != "status") {
        println!("\n== Market: {slug} ==");
        let market = MarketClient::new(client.clone(), config.market_platform.clone());
        match market.price_summary(&slug).await {
            Ok(s) => println!(
                "  lowest sell: {}  |  highest buy: {}  |  active sellers: {}",
                fmt_plat(s.lowest_sell),
                fmt_plat(s.highest_buy),
                s.active_sellers
            ),
            Err(e) => println!("  market lookup failed: {e:#}"),
        }
    } else {
        println!("\n(tip: pass a warframe.market slug to price it, e.g. `wf-lite mirage_prime_set`)");
    }

    Ok(())
}

/// Print grouped usage. Shown for `help`/`-h`/`--help`. A bare `wf-lite` (no
/// command) no longer prints this — it behaves like `tray` instead (#69).
fn print_help() {
    print!(
        "\
wf-lite — Linux-native Warframe companion (overlay, relic picker, mastery)

USAGE:
    wf-lite <command> [args]        (no command runs `tray`)

RUN IT
    tray                  Tray icon: waits for the game, auto-runs the overlay
                           (also what a bare `wf-lite`, with no command, runs)
    overlay               Show the live overlay (live fissures + relic picker)
    browse                Open the mastery/relic browser (Home/Mastery/Relics/
                           Sell/Settings — `settings` is an alias for this)
    toggle | show | hide  Show/hide a running overlay
    copy                  Copy the current best-pick reward (name + plat) to the clipboard

RELICS & MASTERY
    relics <codes…>       Owned-relic guide: unmastered rewards + prices
    mastery-plan          Unmastered primes + which of your relics drop them
    mastery [id]          Report how many items you've mastered
    detect-account        Auto-detect your account id from EE.log
    set-account <id>      Save your account id for mastery lookup

FISSURES & PRICES
    status                Show live Void Fissures
    <market_slug>         Price an item, e.g. `wf-lite mirage_prime_set`

DIAGNOSTICS
    logstats              Parse EE.log history, report coverage/events
    logwatch              Follow EE.log live, print recognized events
    capture [out.png]     Capture the Warframe window to a PNG
    ocr [x y w h]         OCR the Warframe window (or a region)
    relic [names…]        Evaluate reward names → item, plat, mastery
    relic-scan            Capture + rank the on-screen relic rewards
    reward-png            Render the reward panel to a PNG (offscreen)
    relic-guide-png       Render the relic-guide panel to a PNG (offscreen)

Config: ~/.config/warframe-lite/config.toml   Docs: README.md
"
    );
    #[cfg(feature = "mem-scan")]
    print!(
        "\nPHASE 4 (default-compiled; running it is your live confirmation, see ADR-0013)\n    \
         mem-scan               Read Foundry + Riven state from the live game via memory-reading\n"
    );
}

fn print_fissures(ws: &worldstate::WorldState) {
    let active: Vec<_> = ws.fissures.iter().filter(|f| f.active()).collect();
    println!("  Fissures: {} active", active.len());
    for f in active.iter().take(8) {
        let sp = if f.is_hard { " [SP]" } else { "" };
        let storm = if f.is_storm { " [Storm]" } else { "" };
        println!(
            "    {:<6} {:<13} {:<22} {}{}{}",
            f.tier, f.mission_type, f.node, f.eta(), sp, storm
        );
    }
}

fn fmt_plat(p: Option<u32>) -> String {
    match p {
        Some(v) => format!("{v}p"),
        None => "—".to_string(),
    }
}

/// Parse the entire EE.log history and report parse coverage and event counts.
/// Validates the line parser against thousands of real lines.
fn log_stats(ee_log: Option<std::path::PathBuf>) -> Result<()> {
    let Some(path) = ee_log else {
        anyhow::bail!("no EE.log to analyze");
    };
    let bytes = std::fs::read(&path)?;
    let text = String::from_utf8_lossy(&bytes);

    let (mut total, mut parsed) = (0usize, 0usize);
    let mut by_subsystem: std::collections::BTreeMap<String, usize> = Default::default();
    let mut events: Vec<wf_log::Event> = Vec::new();

    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        total += 1;
        if let Some(parsed_line) = wf_log::parse_line(line) {
            parsed += 1;
            *by_subsystem.entry(parsed_line.subsystem.to_string()).or_default() += 1;
            if let Some(ev) = wf_log::classify(&parsed_line) {
                events.push(ev);
            }
        }
    }

    println!("\n== EE.log stats ==");
    println!(
        "  lines: {total}  parsed: {parsed} ({:.1}%)  unparsed (continuations): {}",
        100.0 * parsed as f64 / total.max(1) as f64,
        total - parsed
    );
    println!("  by subsystem:");
    for (sub, n) in &by_subsystem {
        println!("    {sub:<8} {n}");
    }
    println!("  recognized events: {}", events.len());
    for ev in &events {
        println!("    {ev:?}");
    }
    Ok(())
}

/// Capture the Warframe Xwayland window to a PNG and report its geometry.
fn capture_window(out: Option<String>) -> Result<()> {
    let out = out.unwrap_or_else(|| "warframe-capture.png".to_string());
    println!("\n== Capturing Warframe window ==");
    let cap = wf_capture::capture_warframe(None)?;
    println!(
        "  captured {}x{} at root ({}, {})",
        cap.image.width(),
        cap.image.height(),
        cap.root_x,
        cap.root_y
    );
    cap.image.save(&out).map_err(|e| anyhow::anyhow!("saving {out}: {e}"))?;
    println!("  saved to {out}");
    Ok(())
}

/// Read the live Warframe process's memory for the session authz, echo it
/// once to DE's inventory endpoint, and print the parsed Foundry state (see
/// `wf_mem`'s docs and ADR-0001/ADR-0013). Running this subcommand at all
/// **is** the map's required in-the-moment consent — no separate interactive
/// prompt. A failed marker scan, a failed API call, or a missing
/// `CAP_SYS_PTRACE`/`ptrace_scope` capability each surface as a single clear
/// error from `wf_mem` (see `process.rs`/`inventory.rs`) — no retry, printed
/// once by `main`'s top-level `Result` handling.
#[cfg(feature = "mem-scan")]
async fn mem_scan_cmd(config: &Config) -> Result<()> {
    let client = wf_data::http_client();
    let raw = wf_mem::scan_and_fetch(&client).await?;

    println!("\n== mem-scan: Foundry ==");
    let foundry = wf_mem::parse_foundry(&raw)?;
    print_foundry(&foundry);

    println!("\n== mem-scan: Rivens ==");
    let rivens = wf_mem::parse_rivens(&raw)?;
    print_rivens(&rivens);

    println!("\n== mem-scan: Relics (LevelKeys) ==");
    let level_keys = wf_mem::parse_level_keys(&raw)?;
    print_level_keys(&level_keys);

    println!("\n== mem-scan: Equipment ==");
    let equipment = wf_mem::parse_owned_equipment(&raw)?;
    print_owned_equipment(&equipment);

    println!("\n== mem-scan: Owned Relics (MiscItems VoidProjection) ==");
    let relics = wf_mem::parse_owned_relics(&raw)?;
    let relic_names = load_relic_names(&client).await;
    print_owned_relics(&relics, &relic_names);
    write_owned_relics(&relics, &relic_names);

    println!("\n== mem-scan: Owned Parts (MiscItems built components) ==");
    let parts = wf_mem::parse_owned_parts(&raw)?;
    print_owned_parts(&parts);
    let quantities = load_part_quantities(&client).await;
    write_owned_parts(&parts, &quantities);

    println!("\n== mem-scan: Owned but Unmastered ==");
    let mastery = load_mastery(config, &client).await;
    print_owned_but_unmastered(&owned_but_unmastered(&equipment, &mastery));

    Ok(())
}

/// Print parsed Foundry state in the app's existing output style (cf.
/// `print_fissures`/`relics_cmd`) — aligned columns, not a raw JSON dump.
#[cfg(feature = "mem-scan")]
fn print_foundry(state: &wf_mem::FoundryState) {
    if state.pending.is_empty() && state.recipes.is_empty() {
        println!("  Foundry is empty — no builds in progress, no blueprints on hand");
        return;
    }

    if !state.pending.is_empty() {
        println!("  in progress ({}):", state.pending.len());
        for b in &state.pending {
            println!(
                "    {:<32} x{:<3} {}",
                readable_item_name(&b.item_type),
                b.item_count,
                b.completion.map(foundry_eta).unwrap_or_else(|| "—".to_string())
            );
        }
    }

    if !state.recipes.is_empty() {
        println!("  blueprints on hand ({}):", state.recipes.len());
        for r in &state.recipes {
            println!("    {:<32} x{}", readable_item_name(&r.item_type), r.item_count);
        }
    }
}

/// Print parsed Riven state in the app's existing output style (cf.
/// `print_foundry`) — aligned columns, not a raw JSON dump. Plain fused mods
/// never reach here (`wf_mem::parse_rivens` drops those — see its module
/// doc); a still-veiled riven does, with no weapon resolved yet.  `Value`s
/// inside `buffs`/`curses` are DE's encoded roll ints, not displayable
/// percentages (see `wf_mem::riven`'s module doc for why this crate doesn't
/// decode them) — only each stat's `Tag` name is shown, since that's what's
/// actually comparable against the riven's in-game stat lines.
#[cfg(feature = "mem-scan")]
fn print_rivens(state: &wf_mem::RivenState) {
    if state.rivens.is_empty() {
        println!("  no rivens found");
        return;
    }

    println!("  rivens ({}):", state.rivens.len());
    for r in &state.rivens {
        let weapon = r
            .weapon_unique_name
            .as_deref()
            .map(readable_item_name)
            .unwrap_or_else(|| "veiled".to_string());
        let rank = r.rank.map(|v| format!("{v}/8")).unwrap_or_else(|| "—".to_string());
        let mastery = r.mastery_req.map(|v| format!("MR{v}")).unwrap_or_else(|| "—".to_string());
        let rerolls = r.rerolls.map(|v| v.to_string()).unwrap_or_else(|| "—".to_string());
        println!(
            "    {:<32} {:<28} rank {:<4} {:<5} rerolls {}",
            readable_item_name(&r.item_type),
            weapon,
            rank,
            mastery,
            rerolls
        );
        if !r.buffs.is_empty() || !r.curses.is_empty() {
            let tags = |stats: &[wf_mem::RivenStat]| -> String {
                if stats.is_empty() {
                    "—".to_string()
                } else {
                    stats.iter().map(|s| s.tag.as_str()).collect::<Vec<_>>().join(", ")
                }
            };
            println!("      buffs: {}   curses: {}", tags(&r.buffs), tags(&r.curses));
        }
    }
}

/// Print raw `LevelKeys[]` state in the app's existing output style (cf.
/// `print_foundry`) — aligned columns, not a raw JSON dump. This is a raw
/// exposure only (see `wf_mem::level_keys`'s module doc): no refinement
/// decoding, no dedup against the existing OCR-based Seen/Confirmed relic
/// scan (ADR-0009) — that pipeline is untouched and out of scope here.
#[cfg(feature = "mem-scan")]
fn print_level_keys(state: &wf_mem::LevelKeyState) {
    if state.level_keys.is_empty() {
        println!("  no LevelKeys entries found");
        return;
    }

    println!("  entries ({}):", state.level_keys.len());
    for k in &state.level_keys {
        println!("    {:<32} x{}", readable_item_name(&k.item_type), k.item_count);
    }
}

/// Print raw owned-equipment state in the app's existing output style (cf.
/// `print_level_keys`) — aligned columns, grouped by equipment category, not
/// a raw JSON dump. This is a raw ownership exposure only (see
/// `wf_mem::equipment`'s module doc): no cross-reference against `MasterySet`
/// here — see `owned_but_unmastered`/`print_owned_but_unmastered` for that
/// pairing (#65), rendered as its own section.
#[cfg(feature = "mem-scan")]
fn print_owned_equipment(state: &wf_mem::OwnedEquipment) {
    if state.items.is_empty() {
        println!("  no owned equipment found");
        return;
    }

    println!("  {} items owned:", state.items.len());
    for category in wf_mem::EquipmentCategory::ALL {
        let items: Vec<_> =
            state.items.iter().filter(|i| i.category == category).collect();
        if items.is_empty() {
            continue;
        }
        println!("  {} ({}):", category.label(), items.len());
        for item in items {
            println!("    {:<32} x{}", readable_item_name(&item.item_type), item.item_count);
        }
    }
}

/// Print owned-relic state in the app's existing output style (cf.
/// `print_level_keys`) — aligned columns, not a raw JSON dump. Each entry is
/// decoded to its player-facing tier/code/refinement (e.g. "Axi B3
/// (Intact)") via `relic_names`; an entry `relic_names` doesn't recognize
/// (fetch failure, or a genuinely new/unlisted relic) falls back to the raw
/// internal name rather than being dropped — an undecoded entry still shows
/// the player *has* it, just not resolved to a code yet. Still no dedup
/// against the existing OCR-based Seen/Confirmed relic scan (ADR-0009) — that
/// pipeline is untouched and out of scope here.
#[cfg(feature = "mem-scan")]
fn print_owned_relics(state: &wf_mem::OwnedRelicState, relic_names: &wf_relic::RelicNameIndex) {
    if state.relics.is_empty() {
        println!("  no owned-relic entries found");
        return;
    }

    let mut decoded: Vec<(wf_relic::RelicIdentity, &wf_mem::OwnedRelic)> = Vec::new();
    let mut undecoded: Vec<&wf_mem::OwnedRelic> = Vec::new();
    for r in &state.relics {
        match relic_names.lookup(&r.item_type) {
            Some(id) => decoded.push((id.clone(), r)),
            None => undecoded.push(r),
        }
    }
    decoded.sort_by(|(a, _), (b, _)| a.sort_key().cmp(&b.sort_key()));
    undecoded.sort_by(|a, b| a.item_type.cmp(&b.item_type));

    println!("  entries ({}):", state.relics.len());
    for (id, r) in &decoded {
        println!("    {:<24} ({:<11}) x{}", id.display(), id.refinement, r.item_count);
    }
    for r in undecoded {
        println!("    {:<24} {:<13}x{}", readable_item_name(&r.item_type), "(undecoded)", r.item_count);
    }
}

/// Print the outcome of writing owned-relic entries to `owned-relics.json`.
/// The actual decode+snapshot+apply+save logic now lives in
/// [`wf_mem::write_owned_relics`] (#72) — shared with `wf-browse`'s Home-tab
/// Scan Memory button, which calls the same function in-process and formats
/// its own status line from the same [`wf_mem::RelicsWriteReport`]. This is a
/// thin wrapper that preserves the CLI's exact console text (undecoded
/// entries were already visible via `print_owned_relics`'s `(undecoded)`
/// rows; `wf_mem::write_owned_relics` also logs a `tracing::warn!` for them,
/// same as before this refactor).
#[cfg(feature = "mem-scan")]
fn write_owned_relics(state: &wf_mem::OwnedRelicState, relic_names: &wf_relic::RelicNameIndex) {
    let report = wf_mem::write_owned_relics(state, relic_names);
    if report.saved {
        println!(
            "  wrote {} entries to {} ({} undecoded, skipped)",
            report.written,
            wf_relic::OWNED_RELICS_FILE,
            report.undecoded
        );
    }
}

/// Print raw owned-part state in the app's existing output style (cf.
/// `print_level_keys`) — aligned columns, not a raw JSON dump. This is a raw
/// exposure only (see `wf_mem::owned_parts`'s module doc, mirroring #64's
/// `print_owned_relics` before its own #67 decode step): no `(FrameOrWeapon,
/// Part)` split, no `Helmet` -> `Neuroptics` rename, no catalogue
/// cross-reference or `owned-prime-parts.json` wiring — that's #81.
#[cfg(feature = "mem-scan")]
fn print_owned_parts(state: &wf_mem::OwnedPartsState) {
    if state.parts.is_empty() {
        println!("  no owned-part entries found");
        return;
    }

    println!("  entries ({}):", state.parts.len());
    for p in &state.parts {
        println!("    {:<32} x{}", readable_item_name(&p.item_type), p.item_count);
    }
}

/// Owned equipment (#62's raw ownership set) whose internal path names a
/// Prime and isn't yet mastered per `mastery` (#61's cross-reference,
/// surfacing what `XPInfo`-based mastery can't see: a freshly-built,
/// still-rank-0 Prime). Non-Prime equipment never appears here — `MasterySet`
/// only ever tracks Primes (see its own module doc), so cross-referencing a
/// vanilla item would always read "unmastered" without meaning anything.
#[cfg(feature = "mem-scan")]
fn owned_but_unmastered<'a>(
    equipment: &'a wf_mem::OwnedEquipment,
    mastery: &wf_relic::MasterySet,
) -> Vec<&'a wf_mem::OwnedItem> {
    equipment
        .items
        .iter()
        .filter(|item| item.item_type.to_ascii_lowercase().contains("prime"))
        .filter(|item| !mastery.is_mastered_by_path(&item.item_type))
        .collect()
}

/// Print the owned-but-unmastered cross-reference in the app's existing
/// output style (cf. `print_level_keys`). With no `account_id` configured,
/// `mastery` is empty (`load_mastery`'s documented "indicators simply off"
/// convention), so every owned Prime prints here rather than none — expected,
/// not a bug.
#[cfg(feature = "mem-scan")]
fn print_owned_but_unmastered(items: &[&wf_mem::OwnedItem]) {
    if items.is_empty() {
        println!("  none — every owned Prime is already mastered");
        return;
    }
    println!("  {} owned Prime(s) not yet mastered:", items.len());
    for item in items {
        println!("    {:<32} x{}", readable_item_name(&item.item_type), item.item_count);
    }
}

/// DE's public API exposes no display name for these internal paths, so this
/// is the best a CLI can do without a full item catalogue lookup: take the
/// last `/`-separated segment and split it on camelCase boundaries, e.g.
/// `/Lotus/Types/Recipes/Weapons/LatoPrimeBlueprint` -> "Lato Prime Blueprint".
#[cfg(feature = "mem-scan")]
fn readable_item_name(item_type: &str) -> String {
    let leaf = item_type.rsplit('/').next().unwrap_or(item_type);
    let mut out = String::new();
    let mut prev: Option<char> = None;
    for c in leaf.chars() {
        if c.is_uppercase() && prev.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit()) {
            out.push(' ');
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

/// Time remaining until a Foundry build's `CompletionDate`, formatted like
/// `worldstate::Fissure::eta`.
#[cfg(feature = "mem-scan")]
fn foundry_eta(completion: time::OffsetDateTime) -> String {
    format_remaining(completion - time::OffsetDateTime::now_utc())
}

/// "now" once the duration has elapsed, else the two most significant units
/// (mirrors `worldstate::format_duration`'s convention).
#[cfg(feature = "mem-scan")]
fn format_remaining(d: time::Duration) -> String {
    let secs = d.whole_seconds();
    if secs <= 0 {
        return "now".to_string();
    }
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}

#[cfg(all(test, feature = "mem-scan"))]
mod mem_scan_tests {
    use super::*;

    #[test]
    fn readable_item_name_splits_the_leaf_path_segment_on_camel_case() {
        assert_eq!(
            readable_item_name("/Lotus/Types/Recipes/Weapons/LatoPrimeBlueprint"),
            "Lato Prime Blueprint"
        );
        assert_eq!(
            readable_item_name(
                "/Lotus/Types/Recipes/WeaponParts/AkstilettoPrimeReceiverBlueprint"
            ),
            "Akstiletto Prime Receiver Blueprint"
        );
    }

    #[test]
    fn format_remaining_reports_now_for_an_elapsed_or_zero_duration() {
        assert_eq!(format_remaining(time::Duration::seconds(0)), "now");
        assert_eq!(format_remaining(time::Duration::seconds(-10)), "now");
    }

    #[test]
    fn format_remaining_reports_minutes_under_an_hour() {
        assert_eq!(format_remaining(time::Duration::minutes(5)), "5m");
    }

    #[test]
    fn format_remaining_reports_hours_and_minutes_past_the_hour_mark() {
        assert_eq!(format_remaining(time::Duration::minutes(125)), "2h 5m");
    }

    #[test]
    fn format_remaining_reports_days_and_hours_past_the_day_mark() {
        assert_eq!(format_remaining(time::Duration::hours(50)), "2d 2h");
    }

    fn owned(item_type: &str) -> wf_mem::OwnedItem {
        wf_mem::OwnedItem {
            category: wf_mem::EquipmentCategory::Warframes,
            item_type: item_type.to_string(),
            item_count: 1,
        }
    }

    #[test]
    fn owned_but_unmastered_keeps_an_owned_prime_below_the_mastery_cap() {
        let equipment = wf_mem::OwnedEquipment {
            items: vec![owned("/Lotus/Powersuits/Excalibur/ExcaliburPrimeSuit")],
        };
        let mastery = wf_relic::MasterySet::default(); // nothing mastered

        let gaps = owned_but_unmastered(&equipment, &mastery);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].item_type, "/Lotus/Powersuits/Excalibur/ExcaliburPrimeSuit");
    }

    #[test]
    fn owned_but_unmastered_drops_an_already_mastered_prime() {
        let equipment = wf_mem::OwnedEquipment {
            items: vec![owned("/Lotus/Powersuits/Excalibur/ExcaliburPrimeSuit")],
        };
        let mastery = wf_relic::MasterySet::from_xp([(
            "/Lotus/Powersuits/Excalibur/ExcaliburPrimeSuit".to_string(),
            900_000,
        )]);

        assert!(owned_but_unmastered(&equipment, &mastery).is_empty());
    }

    #[test]
    fn owned_but_unmastered_ignores_non_prime_equipment() {
        let equipment =
            wf_mem::OwnedEquipment { items: vec![owned("/Lotus/Powersuits/Volt/VoltSuit")] };
        let mastery = wf_relic::MasterySet::default();

        assert!(owned_but_unmastered(&equipment, &mastery).is_empty());
    }
}

/// Evaluate a set of relic reward names: fuzzy-match each to the item
/// catalogue, look up live plat + ducats, and mark the best picks. With no args,
/// uses a demo set (with deliberate OCR noise) to exercise fuzzy matching.
async fn relic_eval(config: &Config) -> Result<()> {
    let names: Vec<String> = {
        let args: Vec<String> = std::env::args().skip(2).collect();
        if args.is_empty() {
            vec![
                "M1RAGE PRIME BLUEPRINT".to_string(), // OCR noise: 1→I
                "Forma Blueprint".to_string(),
                "Braton Prime Receiver".to_string(),
                "Akbronco Prime Link".to_string(),
            ]
        } else {
            args
        }
    };

    println!("\n== Relic reward evaluation ==");
    let client = http_client();
    let index = wf_relic::ItemIndex::load_cached(&client, CATALOGUE_TTL).await?;
    println!("  catalogue: {} items", index.len());
    let market = MarketClient::new(client.clone(), config.market_platform.clone());
    let cache = wf_relic::price_cache();
    let vaulted = load_vaulted(&client, &index).await;

    let evals = wf_relic::evaluate_cached(
        &names,
        &index,
        &market,
        &cache,
        wf_relic::PriceOpts::default(),
        &vaulted,
    )
    .await;
    let mastery = load_mastery(config, &client).await;
    print_reward_table(&evals, &mastery);
    Ok(())
}

/// Owned-relic mastery guide: for each relic, which rewards you haven't mastered,
/// plus the relic's market price. `wf-lite relics <code…>` treats the given relic
/// codes (e.g. `axi h3 meso n11`) as owned and prices them; with no args it lists
/// every relic that contains an unmastered reward (no prices).
async fn relics_cmd(config: &Config) -> Result<()> {
    let client = http_client();
    let index = wf_relic::RelicIndex::load_cached(&client, CATALOGUE_TTL).await?;
    let mastery = load_mastery(config, &client).await;
    println!(
        "\n== Relic guide ({} relics; {} mastered) ==",
        index.len(),
        mastery.len()
    );

    // Args are relic codes (owned). Adjacent tokens like "axi h3" join into one.
    let args: Vec<String> = std::env::args().skip(2).collect();
    let owned: Vec<&wf_relic::RelicInfo> = if args.is_empty() {
        index.all().iter().collect()
    } else {
        pair_relic_codes(&args)
            .iter()
            .filter_map(|c| index.best_match(c))
            .collect()
    };

    let mut picks: Vec<wf_relic::RelicPick> = Vec::new();
    let price = !args.is_empty(); // only price an explicit owned set (avoids 700+ calls)
    let market = MarketClient::new(client.clone(), config.market_platform.clone());
    let cache = wf_relic::price_cache();
    for r in owned {
        let unmastered = r.unmastered(&mastery);
        if unmastered.is_empty() {
            continue;
        }
        let plat = if price {
            wf_relic::cached_plat(&cache, &market, &r.slug(), wf_relic::PriceOpts::default()).await
        } else {
            None
        };
        picks.push(wf_relic::RelicPick {
            display: r.display.clone(),
            count: 1,
            unmastered,
            plat,
            parts_owned: None,
        });
    }
    cache.save();
    wf_relic::rank_relics(&mut picks);

    if picks.is_empty() {
        println!("  no relics with unmastered rewards found");
        return Ok(());
    }
    for p in picks.iter().take(if price { picks.len() } else { 40 }) {
        let plat = p.plat.map(|v| format!("{v}p")).unwrap_or_else(|| "—".into());
        println!(
            "  {:<10} {:>5}  {} unmastered: {}",
            p.display,
            plat,
            p.unmastered.len(),
            p.unmastered.join(", ")
        );
    }
    if !price {
        println!("\n  (pass relic codes to price them, e.g. `wf-lite relics meso n11 axi h3`)");
    }
    Ok(())
}

/// Map ranked relic picks to overlay rows (top preview reward + counts).
fn relic_rows(picks: &[wf_relic::RelicPick]) -> Vec<wf_overlay::RelicRow> {
    picks
        .iter()
        .map(|p| wf_overlay::RelicRow {
            name: p.display.clone(),
            count: p.count,
            unmastered: p.unmastered.len() as u32,
            top_reward: p.unmastered.first().cloned().unwrap_or_default(),
            plat: p.plat,
        })
        .collect()
}

/// Render the relic-guide overlay panel from a demo owned set, to a PNG.
async fn relic_guide_png(config: &Config) -> Result<()> {
    println!("\n== Rendering relic guide panel ==");
    let client = http_client();
    let index = wf_relic::RelicIndex::load_cached(&client, CATALOGUE_TTL).await?;
    let mastery = load_mastery(config, &client).await;
    let market = MarketClient::new(client.clone(), config.market_platform.clone());
    let cache = wf_relic::price_cache();

    let mut picks = Vec::new();
    for code in ["axi a1", "neo v9", "meso n11", "lith g4", "axi h3"] {
        if let Some(r) = index.best_match(code) {
            let unmastered = r.unmastered(&mastery);
            if unmastered.is_empty() {
                continue;
            }
            let plat =
                wf_relic::cached_plat(&cache, &market, &r.slug(), wf_relic::PriceOpts::default())
                    .await;
            picks.push(wf_relic::RelicPick {
                display: r.display.clone(),
                count: 1,
                unmastered,
                plat,
                parts_owned: None,
            });
        }
    }
    cache.save();
    wf_relic::rank_relics(&mut picks);

    let font = wf_overlay::load_font()?;
    let canvas = wf_overlay::render_relic_panel(&relic_rows(&picks), &font);
    let img = image::RgbaImage::from_raw(canvas.width, canvas.height, canvas.buf)
        .context("canvas -> image")?;
    let out = "relic-guide.png";
    img.save(out).map_err(|e| anyhow::anyhow!("saving {out}: {e}"))?;
    println!("  {}x{} panel saved to {out}", img.width(), img.height());
    Ok(())
}

/// Join relic-code tokens into `"tier code"` pairs, so `["axi","h3","meso","n11"]`
/// becomes `["axi h3", "meso n11"]`. A token that is itself a known era starts a
/// new pair.
fn pair_relic_codes(tokens: &[String]) -> Vec<String> {
    const ERAS: &[&str] = &["lith", "meso", "neo", "axi", "requiem"];
    let mut out = Vec::new();
    let mut cur = String::new();
    for t in tokens {
        if ERAS.contains(&t.to_lowercase().as_str()) {
            if !cur.is_empty() {
                out.push(cur.clone());
            }
            cur = t.clone();
        } else {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(t);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Fissure-planning helper: for each prime you haven't mastered, which of your
/// owned relics (from the last scan, persisted across sessions) can still drop
/// it, and how many — so you know which fissure tier to prioritise running.
/// `wf-lite mastery-plan`.
async fn mastery_plan_cmd(config: &Config) -> Result<()> {
    let Some(owned) =
        wf_cache::load_blob::<wf_relic::OwnedRelics>(wf_relic::OWNED_RELICS_FILE)
    else {
        anyhow::bail!(
            "no owned-relic data yet. Run `wf-lite overlay` (or the tray) and open the in-game \
             Void Relics screen once — it scans automatically as you scroll."
        );
    };

    // Per-entry scan ages are the authoritative freshness signal now; summarise
    // them as a range for the header (the file-level stamp is just when it was
    // last written).
    let age_note = match wf_relic::intact_age_range(&owned.value) {
        Some((newest, oldest)) if newest == oldest => wf_cache::format_age(oldest),
        Some((newest, oldest)) => {
            format!("{} – {}", wf_cache::format_age(newest), wf_cache::format_age(oldest))
        }
        None => wf_cache::format_age(owned.age()),
    };
    println!("\n== Mastery plan (owned relics scanned {age_note}) ==");
    let client = http_client();
    let index = wf_relic::RelicIndex::load_cached(&client, CATALOGUE_TTL).await?;
    let mastery = load_mastery(config, &client).await;
    let quantities = wf_relic::PartQuantities::load_cached(&client, CATALOGUE_TTL)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("part quantity load failed: {e:#}");
            wf_relic::PartQuantities::empty()
        });
    let evidence = wf_relic::owned_evidence(&owned.value);
    let owned_parts = wf_cache::load_blob::<wf_relic::OwnedPrimeParts>(wf_relic::OWNED_PRIME_PARTS_FILE)
        .map(|s| s.value)
        .unwrap_or_default();

    // Price every owned relic (not the whole catalogue) so the breakdown can
    // list cheapest first — the mastery_plan_cmd equivalent of relics_cmd's
    // per-relic pricing.
    let market = MarketClient::new(client.clone(), config.market_platform.clone());
    let cache = wf_relic::price_cache();
    let mut relic_prices: std::collections::HashMap<String, Option<u32>> = std::collections::HashMap::new();
    for relic in index.all() {
        if !evidence.contains_key(&relic.display) {
            continue;
        }
        let plat =
            wf_relic::cached_plat(&cache, &market, &relic.slug(), wf_relic::PriceOpts::default()).await;
        relic_prices.insert(relic.slug(), plat);
    }
    cache.save();

    let ctx = wf_relic::RelicContext { index: &index, mastery: &mastery, quantities: &quantities, owned_parts: &owned_parts };
    let plans = wf_relic::mastery_plan(&evidence, &relic_prices, &std::collections::HashMap::new(), &ctx);

    if plans.is_empty() {
        println!("  no unmastered primes found among your scanned relics");
        return Ok(());
    }

    // Cross-reference against currently active fissures so the breakdown can
    // flag which relics are actionable right now.
    let active_tiers: std::collections::HashSet<String> = worldstate::fetch(&client, &config.platform)
        .await
        .map(|ws| ws.active_fissure_tiers())
        .unwrap_or_default();

    println!("  {:<24} {:>6}", "unmastered prime", "owned");
    for p in &plans {
        println!("  {:<24} {:>6}", truncate_str(&p.prime, 24), p.total_owned);
        for g in &p.parts {
            let need = g.build_quantity.map(|q| format!(" (need x{q})")).unwrap_or_default();
            let breakdown = g
                .relics
                .iter()
                .map(|r| {
                    let live =
                        if active_tiers.contains(wf_relic::tier_of(&r.relic_display)) { "*" } else { "" };
                    let price = r.plat.map(|p| format!(" ({p}p)")).unwrap_or_default();
                    let qty = match r.evidence {
                        wf_relic::RelicEvidence::Confirmed(n) => format!("x{n}"),
                        wf_relic::RelicEvidence::SeenOnly => "seen".to_string(),
                    };
                    format!("{}{live} {qty}{price}", r.relic_display)
                })
                .collect::<Vec<_>>()
                .join(", ");
            println!("      {:<16}{need}  {breakdown}", g.part.part);
        }
    }
    println!("\n  * = a fissure of that relic's tier is active right now");
    Ok(())
}

/// Save a Warframe account id to the config for mastery lookup.
/// `wf-lite set-account <account-id>`.
fn set_account_cmd(config_path: &std::path::Path) -> Result<()> {
    let id = std::env::args()
        .nth(2)
        .context("usage: set-account <account-id> (from https://www.warframe.com/api/user-data)")?;
    let mut config = Config::load(config_path)?;
    config.account_id = Some(id.clone());
    config.save(config_path)?;
    println!("saved account_id={id} to {}", config_path.display());
    Ok(())
}

/// Run the browse window in-process (the `wf_browse` crate is linked into
/// `wf-lite`, not shelled out to as a separate installed binary — see #69).
/// Both `wf-lite`'s `browse` subcommand and its `settings` alias (#72: the
/// standalone settings window was folded into `wf-browse`'s tab bar) land
/// here.
///
/// Called directly (not via `spawn_blocking`, unlike `run_overlay`'s calloop
/// loop): `eframe`/`winit` panics if its event loop is created off the
/// process's actual main thread ("Initializing the event loop outside of the
/// main thread is a significant cross-platform compatibility hazard" —
/// confirmed live in this environment), and `spawn_blocking` runs its
/// closure on a tokio blocking-pool thread, not the main thread. `main`'s
/// `#[tokio::main]` `block_on` drives the async body on the real main thread
/// itself, so a direct, synchronous call here blocks that same thread until
/// the window closes — fine, since this is the last thing `main` does
/// before returning; nothing else needs that thread meanwhile.
fn run_browse() -> Result<()> {
    wf_browse::run().map_err(|e| anyhow::anyhow!("browse window failed: {e}"))
}

/// Auto-detect the local account id from `EE.log` and save it to config.
///
/// The id is scraped from the log (see `wf_log::scan_account`) and every
/// candidate is verified against the public profile API — only an id whose
/// `DisplayName` matches the logged-in name is accepted, so a squadmate's id can
/// never be saved by mistake.
async fn detect_account_cmd(config: &Config, config_path: &std::path::Path) -> Result<()> {
    let log_path = config.resolve_ee_log()?;
    println!("\n== Detect account id ==\n  scanning {}", log_path.display());
    let text = std::fs::read_to_string(&log_path)
        .with_context(|| format!("reading {}", log_path.display()))?;
    let scan = wf_log::scan_account(&text);

    let name = scan.local_name.clone().context(
        "no `Logged in <name>` line in EE.log — log in to Warframe once, then retry",
    )?;
    println!("  logged-in player: {name}");
    if scan.candidates.is_empty() {
        anyhow::bail!(
            "no account id found in this log. It only appears after certain activity \
             (cracking a relic in a squad, a Duviri race). Play a bit and retry, or set it \
             manually with `wf-lite set-account <id>`."
        );
    }

    let client = http_client();
    for id in &scan.candidates {
        match wf_relic::mastery::fetch_display_name(&client, id).await {
            Ok(Some(profile_name)) if profile_name.eq_ignore_ascii_case(&name) => {
                let mut cfg = Config::load(config_path)?;
                cfg.account_id = Some(id.clone());
                cfg.save(config_path)?;
                println!("  verified {id} → {profile_name}");
                println!("  saved account_id to {}", config_path.display());
                return Ok(());
            }
            Ok(Some(other)) => tracing::debug!("candidate {id} is {other}, not {name}"),
            Ok(None) => tracing::debug!("candidate {id} has no profile"),
            Err(e) => tracing::warn!("verifying {id} failed: {e:#}"),
        }
    }
    anyhow::bail!(
        "found {} candidate id(s) but none verified as {name} \
         (network issue, or the id isn't in this log yet). Try again, or use `set-account`.",
        scan.candidates.len()
    )
}

/// Fetch and report the player's mastered set. `wf-lite mastery [account_id]`
/// (uses the config's `account_id` if none is given).
async fn mastery_cmd(config: &Config) -> Result<()> {
    let account_id = std::env::args()
        .nth(2)
        .or_else(|| config.account_id.clone())
        .context(
            "no account id — pass one (`wf-lite mastery <id>`) or set `account_id` in the config.\n\
             Find it at https://www.warframe.com/api/user-data (the `user_id` value)",
        )?;

    println!("\n== Mastery ({account_id}) ==");
    let client = http_client();
    let mastery = wf_relic::mastery::fetch(&client, &account_id).await?;
    println!("  mastered items: {}", mastery.len());

    // Spot-check a few well-known primes.
    for name in [
        "Mirage Prime Blueprint",
        "Braton Prime Receiver",
        "Titania Prime Blueprint",
    ] {
        println!(
            "  {name:<28} {}",
            if mastery.is_mastered(name) { "MASTERED" } else { "not mastered" }
        );
    }
    Ok(())
}

/// Map evaluated rewards to overlay rows (matched name, plat, best pick,
/// mastery, owned Prime Part count, wishlist status).
///
/// `owned_parts` supplies each unmastered row's `owned_count` — the
/// Inventory/Sell screen's scanned owned-Prime-Part counts (see issue #37's
/// downstream-wiring decision). A mastered row never carries a count: the
/// player already has the item, so "how many parts" stops being interesting.
/// `wishlist` supplies `wishlisted` via the same matched-name → `PrimePart`
/// key `wf-browse`'s Mastery tab marks/unmarks (see ADR-0004).
fn reward_rows(
    evals: &[wf_relic::RewardEval],
    mastery: &wf_relic::MasterySet,
    owned_parts: &wf_relic::OwnedPrimeParts,
    wishlist: &wf_relic::Wishlist,
) -> Vec<wf_overlay::RewardRow> {
    let bp = wf_relic::best_pick(evals, mastery);
    evals
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let mastered = e
                .matched_name
                .as_deref()
                .is_some_and(|n| mastery.is_mastered(n));
            let owned_count = e.matched_name.as_deref().filter(|_| !mastered).and_then(|n| {
                wf_relic::owned_parts::get(owned_parts, &wf_relic::mastery::prime_part(n))
            });
            let wishlisted = e.matched_name.as_deref().is_some_and(|n| {
                wishlist.contains(&wf_relic::wishlist::key(&wf_relic::mastery::prime_part(n)))
            });
            wf_overlay::RewardRow {
                name: e.matched_name.clone().unwrap_or_else(|| e.ocr.clone()),
                plat: e.plat,
                best_pick: Some(i) == bp,
                mastered,
                owned_count,
                vaulted: e.vaulted,
                wishlisted,
            }
        })
        .collect()
}

/// Render the reward-result overlay panel from a demo evaluation, to a PNG.
async fn reward_png(config: &Config) -> Result<()> {
    let names: Vec<String> = vec![
        "Titania Prime Blueprint".into(),
        "Volnus Prime Blueprint".into(),
        "Vadarya Prime Recelve".into(),
        "| Zaktl Prime Barrel".into(),
    ];
    println!("\n== Rendering reward panel ==");
    let client = http_client();
    let index = wf_relic::ItemIndex::load_cached(&client, CATALOGUE_TTL).await?;
    let market = MarketClient::new(client.clone(), config.market_platform.clone());
    let cache = wf_relic::price_cache();
    let vaulted = load_vaulted(&client, &index).await;
    let evals = wf_relic::evaluate_cached(
        &names,
        &index,
        &market,
        &cache,
        wf_relic::PriceOpts::default(),
        &vaulted,
    )
    .await;

    let mastery = load_mastery(config, &client).await;
    let owned_parts = wf_cache::load_blob::<wf_relic::OwnedPrimeParts>(wf_relic::OWNED_PRIME_PARTS_FILE)
        .map(|s| s.value)
        .unwrap_or_default();
    let wishlist = wf_cache::load_blob::<wf_relic::Wishlist>(wf_relic::WISHLIST_FILE)
        .map(|s| s.value)
        .unwrap_or_default();
    let font = wf_overlay::load_font()?;
    let canvas =
        wf_overlay::render_reward_panel(&reward_rows(&evals, &mastery, &owned_parts, &wishlist), &font);
    let img = image::RgbaImage::from_raw(canvas.width, canvas.height, canvas.buf)
        .context("canvas -> image")?;
    let out = "reward.png";
    img.save(out).map_err(|e| anyhow::anyhow!("saving {out}: {e}"))?;
    println!("  {}x{} reward panel saved to {out}", img.width(), img.height());
    Ok(())
}

/// Curated item names run through the real eval/mastery/owned-parts/wishlist
/// pipeline for the overlay's demo mode — same pattern as [`reward_png`], but
/// using clean names (not deliberately-garbled OCR text) since these render
/// straight into a live preview rather than exercising fuzzy matching.
/// Whether a given row actually lands on "vaulted"/"wishlisted"/"no price"
/// depends on the current player's live mastery/wishlist/vaulted state, so
/// coverage of those categories is pipeline-driven, not guaranteed.
const DEMO_REWARD_NAMES: &[&str] = &[
    "Volt Prime Systems",
    "Nyx Prime Chassis",
    "Loki Prime Neuroptics",
    "Ash Prime Blueprint",
];

/// How long each demo-mode state (reward panel, then fissures panel) stays
/// up before [`run_overlay`]'s frame builder cycles to the next one.
const DEMO_CYCLE: Duration = Duration::from_secs(4);

/// Curated overlay content for demo mode (see [`spawn_control_listener`]'s
/// `demo-on`/`demo-off` handling): a fixed reward panel and a fixed fissures
/// panel, computed once when `demo-on` arrives and then cycled by
/// `run_overlay`'s frame builder — so a settings UI can preview
/// placement/opacity against every visually-distinct panel state without
/// needing a live reward drop or live fissures to wait for.
struct DemoFrames {
    reward_rows: Vec<wf_overlay::RewardRow>,
    fissures: worldstate::WorldState,
    /// When these frames were computed — cycling is timed off this rather
    /// than a frame counter, so it stays correct regardless of render cadence.
    started: std::time::Instant,
}

/// Build [`DemoFrames`]: [`DEMO_REWARD_NAMES`] run through the real eval
/// pipeline (mirrors [`reward_png`]) for the reward panel, plus a small fixed
/// spread of synthetic Fissures for the fissures panel.
async fn build_demo_frames(config: &Config, client: &reqwest::Client) -> Result<DemoFrames> {
    let names: Vec<String> = DEMO_REWARD_NAMES.iter().map(|s| s.to_string()).collect();
    let index = wf_relic::ItemIndex::load_cached(client, CATALOGUE_TTL).await?;
    let market = MarketClient::new(client.clone(), config.market_platform.clone());
    let cache = wf_relic::price_cache();
    let vaulted = load_vaulted(client, &index).await;
    let evals =
        wf_relic::evaluate_cached(&names, &index, &market, &cache, wf_relic::PriceOpts::default(), &vaulted)
            .await;
    let mastery = load_mastery(config, client).await;
    let owned_parts = wf_cache::load_blob::<wf_relic::OwnedPrimeParts>(wf_relic::OWNED_PRIME_PARTS_FILE)
        .map(|s| s.value)
        .unwrap_or_default();
    let wishlist = wf_cache::load_blob::<wf_relic::Wishlist>(wf_relic::WISHLIST_FILE)
        .map(|s| s.value)
        .unwrap_or_default();
    Ok(DemoFrames {
        reward_rows: reward_rows(&evals, &mastery, &owned_parts, &wishlist),
        fissures: demo_fissures(),
        started: std::time::Instant::now(),
    })
}

/// Synthetic active Fissures for demo mode's fissures-panel state: a small
/// fixed spread of tiers/mission types — including a Steel Path and a Void
/// Storm fissure — so the panel's badge colors and layout are all visible at
/// once. Expiry is far enough out to outlast any demo session.
fn demo_fissures() -> worldstate::WorldState {
    let expiry = (time::OffsetDateTime::now_utc() + time::Duration::hours(6))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let fissure = |node: &str, mission_type: &str, tier: &str, is_hard: bool, is_storm: bool| {
        worldstate::Fissure {
            node: node.to_string(),
            mission_type: mission_type.to_string(),
            tier: tier.to_string(),
            is_hard,
            is_storm,
            expiry: expiry.clone(),
        }
    };
    worldstate::WorldState {
        fissures: vec![
            fissure("Hepit (Void)", "Capture", "Axi", false, false),
            fissure("Ukko (Void)", "Exterminate", "Neo", true, false),
            fissure("Override (Void)", "Void Storm", "Requiem", false, true),
            fissure("Cambria (Void)", "Survival", "Lith", false, false),
        ],
    }
}

/// Print a ranked reward table: plat, best-pick marker, and mastery status.
fn print_reward_table(evals: &[wf_relic::RewardEval], mastery: &wf_relic::MasterySet) {
    let best_pick = wf_relic::best_pick(evals, mastery);
    println!("  {:<26} {:>6}  {:<9} match", "reward", "plat", "mastery");
    for (i, e) in evals.iter().enumerate() {
        let mark = if Some(i) == best_pick { " ⭐pick" } else { "" };
        let name = e.matched_name.as_deref().unwrap_or("(no match)");
        let mastered = e
            .matched_name
            .as_deref()
            .is_some_and(|n| mastery.is_mastered(n));
        println!(
            "  {:<26} {:>6}  {:<9} {} ({:.0}%){}",
            truncate_str(name, 26),
            e.plat.map(|p| p.to_string()).unwrap_or_else(|| "—".into()),
            if mastered { "MASTERED" } else { "—" },
            truncate_str(&e.ocr, 22),
            e.score * 100.0,
            mark
        );
    }
}

fn truncate_str(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).chain(['…']).collect()
    }
}

/// Fixed overlay surface size. Both the world-state and reward panels are
/// embedded top-left into this; the unused remainder is transparent and
/// click-through, so a single fixed-size layer surface can show either panel.
const OVERLAY_W: u32 = wf_overlay::render::WIDTH;
const OVERLAY_H: u32 = 340;
/// How long a detected reward result stays on the overlay.
const REWARD_DISPLAY: Duration = Duration::from_secs(20);
/// How long the overlay polls for the game window before falling back to the
/// compositor's default output (lets it be launched together with the game).
const WINDOW_WAIT: Duration = Duration::from_secs(30);
/// Item catalogue is refetched at most this often (new primes are rare).
const CATALOGUE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// Mastery data is refreshed at most this often (it changes slowly).
const MASTERY_TTL: Duration = Duration::from_secs(24 * 3600);

/// Ceiling on the worldstate-refresh backoff (see `worldstate_retry_interval`)
/// — long enough to stop hammering a struggling warframestat.us, short enough
/// that fissures come back within a reasonable window once it recovers.
const WORLDSTATE_RETRY_CAP: Duration = Duration::from_secs(600);

/// Retry interval for the overlay's worldstate refresh loop, given how many
/// fetches have failed in a row: `base` on a clean run, doubling per
/// consecutive failure and capped at [`WORLDSTATE_RETRY_CAP`] (issue #40 — a
/// struggling/erroring API shouldn't be polled at the steady-state cadence
/// forever). Resets to `base` the moment a fetch succeeds.
fn worldstate_retry_interval(base: Duration, consecutive_failures: u32) -> Duration {
    if consecutive_failures == 0 {
        return base;
    }
    // Cap the shift, not just the result: `1u32 << 32` panics, and this many
    // consecutive failures already blows past the cap many times over.
    let multiplier = 1u32 << consecutive_failures.min(16);
    base.checked_mul(multiplier).unwrap_or(WORLDSTATE_RETRY_CAP).min(WORLDSTATE_RETRY_CAP)
}

/// Load the player's mastered set (cached) if an account id is configured,
/// otherwise an empty set (mastery indicators simply off).
async fn load_mastery(config: &Config, client: &reqwest::Client) -> wf_relic::MasterySet {
    match &config.account_id {
        Some(id) => wf_relic::mastery::load_cached(client, id, MASTERY_TTL).await,
        None => wf_relic::MasterySet::default(),
    }
}

/// Best-effort reward-name → vaulted-status lookup (see
/// [`wf_relic::vaulted_rewards`]): loads the cached relic drop tables and
/// joins them against `items`. An empty map (no vaulted badges shown) is
/// returned if the relic tables can't be loaded, so the reward panel still
/// renders normally.
async fn load_vaulted(client: &reqwest::Client, items: &wf_relic::ItemIndex) -> HashMap<String, bool> {
    match wf_relic::RelicIndex::load_cached(client, CATALOGUE_TTL).await {
        Ok(relics) => wf_relic::vaulted_rewards(&relics, items),
        Err(e) => {
            tracing::warn!("relic table load failed ({e:#}); vaulted status unavailable");
            HashMap::new()
        }
    }
}

/// Best-effort raw-owned-relic-name → player-facing-identity lookup (see
/// [`wf_relic::RelicNameIndex`]): an empty index (owned relics shown
/// undecoded, see `print_owned_relics`) is returned if the WFCD name
/// catalogue can't be loaded, so `mem-scan` still runs.
#[cfg(feature = "mem-scan")]
async fn load_relic_names(client: &reqwest::Client) -> wf_relic::RelicNameIndex {
    match wf_relic::RelicNameIndex::load_cached(client, CATALOGUE_TTL).await {
        Ok(index) => index,
        Err(e) => {
            tracing::warn!("relic name index load failed ({e:#}); owned relics shown undecoded");
            wf_relic::RelicNameIndex::empty()
        }
    }
}

/// Best-effort WFCD Prime-Part-build-quantities catalogue load (issue #81):
/// an empty catalogue (every raw owned-part entry dropped, see
/// [`write_owned_parts`]) is returned if the WFCD dataset can't be loaded,
/// so `mem-scan` still runs — mirrors [`load_relic_names`]'s same
/// fail-open convention for the relic side of mem-scan.
#[cfg(feature = "mem-scan")]
async fn load_part_quantities(client: &reqwest::Client) -> wf_relic::PartQuantities {
    match wf_relic::PartQuantities::load_cached(client, CATALOGUE_TTL).await {
        Ok(quantities) => quantities,
        Err(e) => {
            tracing::warn!("part quantities load failed ({e:#}); owned parts shown undecoded");
            wf_relic::PartQuantities::empty()
        }
    }
}

/// Print the outcome of writing owned-Prime-Part entries to
/// `owned-prime-parts.json` (issue #81) — mirrors [`write_owned_relics`]'s
/// wrapper. `skipped` isn't broken out from `written` in this line the way
/// undecoded relics are: most skips are ordinary non-Prime gear, not a
/// decode gap (see [`wf_mem::write_owned_parts`]'s doc).
#[cfg(feature = "mem-scan")]
fn write_owned_parts(state: &wf_mem::OwnedPartsState, quantities: &wf_relic::PartQuantities) {
    let report = wf_mem::write_owned_parts(state, quantities);
    if report.saved {
        println!(
            "  wrote {} entries to {} ({} non-Prime/unrecognized, skipped)",
            report.written,
            wf_relic::OWNED_PRIME_PARTS_FILE,
            report.skipped
        );
    }
}

type RewardState = std::sync::Arc<std::sync::Mutex<Option<(std::time::Instant, Vec<wf_overlay::RewardRow>)>>>;

/// The current reward rows, if a reward screen was detected within the last
/// [`REWARD_DISPLAY`] window — the same freshness check the overlay's own
/// reward panel (`make_frame` in [`run_overlay`]) and the `copy` control
/// command ([`copy_best_reward`]) both need before acting on `reward`.
fn current_reward_rows(reward: &RewardState) -> Option<Vec<wf_overlay::RewardRow>> {
    reward
        .lock()
        .unwrap()
        .as_ref()
        .filter(|(t, _)| t.elapsed() < REWARD_DISPLAY)
        .map(|(_, r)| r.clone())
}

/// Live progress for an in-flight relic scan, set the moment the Relics
/// screen is detected so the overlay reacts immediately instead of showing
/// nothing (see [`crate::run_overlay`]'s panel priority).
type RelicScanStatus = std::sync::Arc<std::sync::Mutex<Option<wf_overlay::ScanProgress>>>;

/// The current [`DemoFrames`], if demo mode is on — set by `demo-on` (once
/// [`build_demo_frames`] finishes fetching) and cleared by `demo-off` (see
/// [`spawn_control_listener`]). Takes top priority in `make_frame` over
/// [`crate::run_overlay`]'s normal reward/relic-scan/fissures panels.
type DemoState = std::sync::Arc<std::sync::Mutex<Option<DemoFrames>>>;

/// One step of the mission-state machine (issue #89's design): a
/// `MissionInfoSeen` arms `pending`; the next `LevelOpen` consumes it,
/// producing the new `in_mission` value (armed -> true, unarmed -> false).
/// Any other event leaves both unchanged. Pulled out of
/// [`mission_watch_loop`] as a pure function so the state machine itself is
/// unit-testable without a real `EE.log`.
fn mission_state_step(pending: bool, ev: &wf_log::Event) -> (bool, Option<bool>) {
    use wf_log::Event;
    match ev {
        Event::MissionInfoSeen => (true, None),
        Event::LevelOpen => (false, Some(pending)),
        _ => (pending, None),
    }
}

/// Watches the EE.log for mission-start/mission-end transitions (issue #89's
/// design) and publishes the result to `in_mission`, read by `make_frame` to
/// gate the fissures panel. Runs independently of relic auto-detection.
async fn mission_watch_loop(
    ee_log: std::path::PathBuf,
    in_mission: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;

    const POLL_INTERVAL: Duration = Duration::from_secs(1);

    let mut tailer = wf_log::LogTailer::from_end(&ee_log);
    let mut pending = false;

    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        for line in tailer.poll().unwrap_or_default() {
            if let Some(ev) = wf_log::event_from_line(&line) {
                let (new_pending, new_state) = mission_state_step(pending, &ev);
                pending = new_pending;
                if let Some(state) = new_state {
                    in_mission.store(state, Ordering::Relaxed);
                }
            }
        }
    }
}

/// Show the live overlay as a `wlr-layer-shell` surface: live Fissures normally,
/// automatically swapping to the relic reward result for a few seconds when a
/// fissure reward is detected in the log.
async fn run_overlay(config: Config) -> Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc, Mutex};

    let font = Arc::new(wf_overlay::load_font()?);
    let client = http_client();
    let platform = config.platform.clone();
    let refresh = Duration::from_secs(config.fissure_refresh_secs.max(15));
    let reward: RewardState = Arc::new(Mutex::new(None));
    let relic_scan_status: RelicScanStatus = Arc::new(Mutex::new(None));
    let demo: DemoState = Arc::new(Mutex::new(None));

    // Appearance/visibility knobs. `visible` is flipped at runtime by the control
    // socket (see `overlay_control`); `live` holds the anchor/margin/opacity/
    // fissures fields, also runtime-updatable via the control socket's
    // `apply-settings` command (see `spawn_control_listener`) so a settings UI
    // can push placement/appearance changes to a running overlay without a
    // restart.
    let visible = Arc::new(AtomicBool::new(true));
    let live = Arc::new(Mutex::new(wf_config::control::LiveOverlaySettings::from(&config.overlay)));

    // Mission-state gate for the fissures panel (issue #89/#90's design):
    // fails open (stays `false`, panel shows) if the EE.log can't be
    // resolved, matching this destination's fail-open requirement.
    let in_mission = Arc::new(AtomicBool::new(false));
    match config.resolve_ee_log() {
        Ok(ee_log) => {
            tokio::spawn(mission_watch_loop(ee_log, in_mission.clone()));
        }
        Err(e) => {
            tracing::warn!(
                "could not resolve EE.log path ({e:#}) — fissures panel will not be gated by mission state"
            );
        }
    }

    // Build one overlay frame from the current state, honoring reward-only mode,
    // the visibility toggle, and opacity. A hidden or empty frame is a fully
    // transparent (click-through) canvas.
    // Panel priority when shown: demo mode (a settings UI is previewing
    // placement/opacity — see `spawn_control_listener`'s `demo-on`/`demo-off`)
    // → reward screen (time-critical, ~20s) → an in-progress relic scan's live
    // status → live fissures (only outside a mission — see
    // `mission_watch_loop`) → blank. The ranked owned-relic guide (`wf-lite
    // relic-guide-png`'s `render_relic_panel`) isn't shown live here — that
    // view lives in `wf-lite browse` instead, which reads the same
    // `owned-relics.json` without the live overlay's latency budget, and
    // demo mode doesn't preview it either (it only cycles panels the live
    // overlay actually renders).
    let make_frame = {
        let font = font.clone();
        let reward = reward.clone();
        let relic_scan_status = relic_scan_status.clone();
        let demo = demo.clone();
        let live = live.clone();
        let in_mission = in_mission.clone();
        move |ws: &worldstate::WorldState, shown: bool| -> wf_overlay::Canvas {
            let (show_fissures, opacity, fissure_filter) = {
                let live = live.lock().unwrap();
                (live.fissures, live.opacity, live.fissure_filter.clone())
            };
            let blank = || wf_overlay::Canvas::new(OVERLAY_W, OVERLAY_H);
            let mut c = if !shown {
                blank()
            } else if let Some(frames) = demo.lock().unwrap().as_ref() {
                // Alternate between the reward panel and the fissures panel
                // every `DEMO_CYCLE`, timed off when the frames were built
                // rather than a render-frame counter.
                let cycle = frames.started.elapsed().as_secs() / DEMO_CYCLE.as_secs().max(1);
                if cycle.is_multiple_of(2) {
                    wf_overlay::render_reward_panel(&frames.reward_rows, &font).embed(OVERLAY_W, OVERLAY_H)
                } else {
                    // Demo mode previews the panel's full visual states
                    // regardless of the user's real filter, so its synthetic
                    // fissures always show unfiltered.
                    wf_overlay::render_panel(&frames.fissures, &font, &Default::default())
                        .embed(OVERLAY_W, OVERLAY_H)
                }
            } else if let Some(rows) = current_reward_rows(&reward) {
                wf_overlay::render_reward_panel(&rows, &font).embed(OVERLAY_W, OVERLAY_H)
            } else if let Some(progress) = *relic_scan_status.lock().unwrap() {
                wf_overlay::render_relic_scanning_panel(progress, &font).embed(OVERLAY_W, OVERLAY_H)
            } else if show_fissures && !in_mission.load(Ordering::Relaxed) {
                wf_overlay::render_panel(ws, &font, &fissure_filter).embed(OVERLAY_W, OVERLAY_H)
            } else {
                blank()
            };
            c.scale_alpha(opacity);
            c
        }
    };

    println!("\n== Live overlay (Ctrl-C to stop) ==");
    println!(
        "  placement: {} (margin {}x{}); fissure panel: {}; opacity: {}",
        config.overlay.anchor,
        config.overlay.margin_x,
        config.overlay.margin_y,
        if config.overlay.fissures { "on" } else { "reward-only" },
        config.overlay.opacity,
    );
    // A failed initial fetch (warframestat.us down/erroring right at launch —
    // see issue #40) must never take the overlay down with it: start with an
    // empty world-state and let the refresh loop below fill it in once the API
    // recovers, backing off its retry cadence the same way it would for any
    // later failure (`consecutive_failures` seeds at 1 here, not 0).
    let mut consecutive_failures: u32 = 0;
    let ws = match worldstate::fetch(&client, &platform).await {
        Ok(ws) => ws,
        Err(e) => {
            tracing::warn!("initial worldstate fetch failed: {e:#} — starting with empty fissures");
            consecutive_failures = 1;
            worldstate::WorldState::default()
        }
    };
    let initial = make_frame(&ws, visible.load(Ordering::Relaxed));

    let (tx, rx) = mpsc::channel();
    let (placement_tx, placement_rx) = mpsc::channel();

    // Control socket: `wf-lite toggle|show|hide` flips `visible` at runtime,
    // `copy` copies the current best-pick reward, `apply-settings` (from a
    // settings UI) updates `live` and re-anchors the layer surface via
    // `placement_tx`, and `demo-on`/`demo-off` swap `demo` in and out — so a
    // KDE global shortcut or a running settings tab can both act on a running
    // overlay on demand, no restart needed.
    spawn_control_listener(
        visible.clone(),
        reward.clone(),
        live.clone(),
        demo.clone(),
        placement_tx,
        config.clone(),
        client.clone(),
    );

    // Relic auto-detection: needs tesseract, an EE.log, and the item catalogue.
    // Delegates to the `ocr` module — the real implementation when the `ocr`
    // feature is compiled in, a friendly "OFF" stub otherwise (see its
    // module docs). Built before the renderer loop below so the renderer can
    // wire up background fissure-start price pre-warming (`prewarm_ctx`)
    // from the same catalogues/cache/market that block already loads.
    let prewarm_ctx: Option<ocr::PrewarmCtx> =
        ocr::start_relic_watch(&config, &client, reward.clone(), relic_scan_status.clone()).await;

    // Renderer: rebuild the frame each second (ETAs tick, reward panel expires,
    // visibility may have toggled) and push it to the layer surface. Also
    // drives fissure-start price pre-warming off this same worldstate refresh
    // (see `prewarm_ctx`) rather than polling warframestat.us a second time.
    {
        let client = client.clone();
        let visible = visible.clone();
        tokio::spawn(async move {
            let mut cached = ws;
            let mut last_fetch = tokio::time::Instant::now();
            // Tiers a pre-warm has already been dispatched for this session —
            // re-checked against `active` below so a tier that goes quiet and
            // later starts again (a fresh fissure of the same era) re-triggers
            // pre-warming instead of being skipped forever.
            let mut warmed_tiers: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut consecutive_failures = consecutive_failures;
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let interval = worldstate_retry_interval(refresh, consecutive_failures);
                if last_fetch.elapsed() >= interval {
                    match worldstate::fetch(&client, &platform).await {
                        Ok(fresh) => {
                            consecutive_failures = 0;
                            if let Some(ctx) = &prewarm_ctx {
                                let active = fresh.active_fissure_tiers();
                                warmed_tiers.retain(|t| active.contains(t));
                                let new_tiers: std::collections::HashSet<String> =
                                    active.difference(&warmed_tiers).cloned().collect();
                                if !new_tiers.is_empty() {
                                    warmed_tiers.extend(new_tiers.iter().cloned());
                                    tokio::spawn(ocr::prewarm_new_tiers(ctx.clone(), new_tiers));
                                }
                            }
                            cached = fresh;
                        }
                        Err(e) => {
                            consecutive_failures += 1;
                            let next = worldstate_retry_interval(refresh, consecutive_failures);
                            tracing::warn!(
                                "worldstate refresh failed: {e:#} — backing off to {}s",
                                next.as_secs()
                            );
                        }
                    }
                    last_fetch = tokio::time::Instant::now();
                }
                let frame = make_frame(&cached, visible.load(Ordering::Relaxed));
                if tx.send(frame).is_err() {
                    break; // overlay closed
                }
            }
        });
    }

    // Place the overlay on the game's monitor, hugging its window corner. Poll
    // briefly for the window so the overlay can be started *with* the game (e.g.
    // from Steam launch options) before Xwayland has mapped it.
    let window = wait_for_window(WINDOW_WAIT).await;
    match window {
        Some((x, y, w, h)) => println!("  overlay target: game window {w}x{h} at ({x},{y})"),
        None => println!("  overlay target: compositor default (game window not found)"),
    }

    let placement = wf_overlay::layer::Placement::parse(
        &config.overlay.anchor,
        config.overlay.margin_x,
        config.overlay.margin_y,
    );

    // The Wayland event loop is blocking and uses non-Send types; run it on a
    // dedicated blocking thread.
    tokio::task::spawn_blocking(move || {
        wf_overlay::layer::run(initial, rx, placement, placement_rx, window)
    })
    .await
    .context("overlay thread panicked")?
}

/// Listen on the control socket for `toggle` / `show` / `hide` / `copy` /
/// `apply-settings ...` / `demo-on` / `demo-off` lines and act on the shared
/// `visible` flag / `reward` state / `live` settings (re-anchoring the layer
/// surface live via `placement_tx` for `apply-settings`) / `demo` state.
/// `demo-on` fetches [`build_demo_frames`] on its own task rather than
/// blocking this accept loop — it hits the network (item catalogue, market,
/// mastery) — so `demo` only flips once the fetch completes; `demo-off` clears
/// it immediately. A stale socket file from a previous run is removed first.
/// Runs for the life of the overlay.
fn spawn_control_listener(
    visible: std::sync::Arc<std::sync::atomic::AtomicBool>,
    reward: RewardState,
    live: std::sync::Arc<std::sync::Mutex<wf_config::control::LiveOverlaySettings>>,
    demo: DemoState,
    placement_tx: std::sync::mpsc::Sender<wf_overlay::layer::Placement>,
    config: Config,
    client: reqwest::Client,
) {
    use std::sync::atomic::Ordering;
    use tokio::io::AsyncReadExt;

    let path = wf_config::control::socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = match tokio::net::UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("overlay control socket unavailable ({e}); toggle disabled");
            return;
        }
    };
    println!("  control:   {} (wf-lite toggle|show|hide|copy)", path.display());
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                continue;
            };
            let mut buf = String::new();
            if stream.read_to_string(&mut buf).await.is_err() {
                continue;
            }
            match buf.trim() {
                "toggle" => {
                    let now = !visible.fetch_xor(true, Ordering::Relaxed);
                    tracing::info!("overlay {}", if now { "shown" } else { "hidden" });
                }
                "show" => visible.store(true, Ordering::Relaxed),
                "hide" => visible.store(false, Ordering::Relaxed),
                "copy" => copy_best_reward(&reward),
                wf_config::control::DEMO_ON_CMD => {
                    let demo = demo.clone();
                    let config = config.clone();
                    let client = client.clone();
                    tokio::spawn(async move {
                        match build_demo_frames(&config, &client).await {
                            Ok(frames) => {
                                *demo.lock().unwrap() = Some(frames);
                                tracing::info!("demo mode on");
                            }
                            Err(e) => tracing::warn!("demo mode: building demo frames failed: {e:#}"),
                        }
                    });
                }
                wf_config::control::DEMO_OFF_CMD => {
                    *demo.lock().unwrap() = None;
                    tracing::info!("demo mode off");
                }
                other if other.starts_with(wf_config::control::APPLY_SETTINGS_CMD) => {
                    match wf_config::control::parse_apply_settings(other) {
                        Some(p) => {
                            {
                                let mut live = live.lock().unwrap();
                                live.anchor = p.anchor.clone();
                                live.margin_x = p.margin_x;
                                live.margin_y = p.margin_y;
                                live.opacity = p.opacity;
                                live.fissures = p.fissures;
                                live.fissure_filter = p.fissure_filter.clone();
                            }
                            let placement = wf_overlay::layer::Placement::parse(
                                &p.anchor,
                                p.margin_x,
                                p.margin_y,
                            );
                            let _ = placement_tx.send(placement);
                            tracing::info!("applied live overlay settings: {p:?}");
                        }
                        None => tracing::warn!("malformed apply-settings command: {other:?}"),
                    }
                }
                other => tracing::warn!("unknown overlay control command {other:?}"),
            }
        }
    });
}

/// Env var to override the `wl-copy` binary path, for users with a
/// differently-named/pathed clipboard binary.
fn wl_copy_bin() -> String {
    std::env::var("WF_WL_COPY").unwrap_or_else(|_| "wl-copy".into())
}

/// Format a reward row as clipboard-ready text for Warframe's trade chat, e.g.
/// `"Mirage Prime Systems 45p"`. Falls back to just the name when the price is
/// unresolved (shown as "—" in the overlay).
fn clipboard_text(row: &wf_overlay::RewardRow) -> String {
    match row.plat {
        Some(p) => format!("{} {p}p", row.name),
        None => row.name.clone(),
    }
}

/// Handle the `copy` control-socket command: find the current best-pick reward
/// row (same source and freshness window as the overlay's own reward panel)
/// and copy it to the clipboard. Logs a clear message — never crashes or
/// hangs — if there's no active reward or `wl-copy` isn't available.
fn copy_best_reward(reward: &RewardState) {
    let Some(rows) = current_reward_rows(reward) else {
        tracing::warn!("copy requested but no active reward to copy");
        return;
    };
    let Some(best) = rows.iter().find(|r| r.best_pick) else {
        tracing::warn!("copy requested but no best-pick reward row");
        return;
    };
    let text = clipboard_text(best);
    match copy_to_clipboard(&text) {
        Ok(()) => tracing::info!("copied to clipboard: {text}"),
        Err(e) => tracing::warn!("clipboard copy failed ({e:#}); is wl-copy installed?"),
    }
}

/// Shell out to `wl-copy` (or `$WF_WL_COPY`) with `text` on stdin. Not
/// practically unit-testable (external process, real Wayland compositor) —
/// rely on manual verification instead.
fn copy_to_clipboard(text: &str) -> Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new(wl_copy_bin())
        .stdin(Stdio::piped())
        .spawn()
        .context("spawning wl-copy")?;
    child
        .stdin
        .take()
        .context("wl-copy stdin unavailable")?
        .write_all(text.as_bytes())
        .context("writing to wl-copy stdin")?;
    let status = child.wait().context("waiting for wl-copy")?;
    anyhow::ensure!(status.success(), "wl-copy exited with {status}");
    Ok(())
}

/// Client side of the control socket: send a single command to a running overlay.
fn overlay_control(cmd: &str) -> Result<()> {
    wf_config::control::send_command(cmd)?;
    println!("sent '{cmd}' to the overlay");
    Ok(())
}

/// Poll for the Warframe window up to `timeout`, returning its root-space
/// rectangle once found (or `None` if it never appears).
async fn wait_for_window(timeout: Duration) -> Option<(i32, i32, u32, u32)> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(rect) = wf_capture::warframe_geometry() {
            return Some(rect);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Render the live world-state panel offscreen to a PNG (for visual verification
/// of the overlay renderer without needing the on-screen layer surface).
async fn overlay_png(config: &Config, out: Option<String>) -> Result<()> {
    let out = out.unwrap_or_else(|| "overlay.png".to_string());
    println!("\n== Rendering overlay panel ==");
    let ws = worldstate::fetch(&http_client(), &config.platform).await?;
    let font = wf_overlay::load_font()?;
    let canvas = wf_overlay::render_panel(&ws, &font, &config.overlay.fissure_filter);
    let img = image::RgbaImage::from_raw(canvas.width, canvas.height, canvas.buf)
        .context("canvas buffer -> image")?;
    img.save(&out).map_err(|e| anyhow::anyhow!("saving {out}: {e}"))?;
    println!("  {}x{} panel saved to {out}", img.width(), img.height());
    Ok(())
}

/// Follow EE.log live and print recognized events as they occur. Runs until the
/// process is interrupted.
fn log_watch(ee_log: Option<std::path::PathBuf>) -> Result<()> {
    let Some(path) = ee_log else {
        anyhow::bail!("no EE.log to watch");
    };
    println!("\n== Watching EE.log (Ctrl-C to stop) ==");
    let mut tailer = wf_log::LogTailer::from_end(&path);
    loop {
        let mut saw_event = false;
        for line in tailer.poll()? {
            if let Some(ev) = wf_log::event_from_line(&line) {
                saw_event = true;
                println!("  event: {ev:?}   ⟵  {line}");
            }
        }
        if !saw_event {
            // Nothing recognized this tick; keep polling.
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod clipboard_tests {
    use super::clipboard_text;
    use wf_overlay::RewardRow;

    fn row(name: &str, plat: Option<u32>) -> RewardRow {
        RewardRow {
            name: name.into(),
            plat,
            best_pick: true,
            mastered: false,
            owned_count: None,
            vaulted: false,
            wishlisted: false,
        }
    }

    #[test]
    fn formats_name_and_plat() {
        assert_eq!(
            clipboard_text(&row("Mirage Prime Systems", Some(45))),
            "Mirage Prime Systems 45p"
        );
    }

    #[test]
    fn falls_back_to_name_when_plat_unresolved() {
        assert_eq!(clipboard_text(&row("Volnus Prime Blueprint", None)), "Volnus Prime Blueprint");
    }
}

#[cfg(test)]
mod worldstate_retry_tests {
    use super::worldstate_retry_interval;
    use std::time::Duration;

    #[test]
    fn no_failures_uses_the_base_interval() {
        assert_eq!(worldstate_retry_interval(Duration::from_secs(60), 0), Duration::from_secs(60));
    }

    #[test]
    fn doubles_per_consecutive_failure() {
        let base = Duration::from_secs(60);
        assert_eq!(worldstate_retry_interval(base, 1), Duration::from_secs(120));
        assert_eq!(worldstate_retry_interval(base, 2), Duration::from_secs(240));
        assert_eq!(worldstate_retry_interval(base, 3), Duration::from_secs(480));
    }

    #[test]
    fn caps_at_ten_minutes() {
        let base = Duration::from_secs(60);
        assert_eq!(worldstate_retry_interval(base, 4), Duration::from_secs(600));
        assert_eq!(worldstate_retry_interval(base, 30), Duration::from_secs(600));
    }
}

#[cfg(test)]
mod demo_fissures_tests {
    use super::demo_fissures;

    #[test]
    fn every_synthetic_fissure_is_active() {
        let ws = demo_fissures();
        assert!(!ws.fissures.is_empty());
        assert!(ws.fissures.iter().all(|f| f.active()));
    }

    #[test]
    fn covers_a_steel_path_and_a_void_storm_fissure() {
        let ws = demo_fissures();
        assert!(ws.fissures.iter().any(|f| f.is_hard), "expected a Steel Path fissure");
        assert!(ws.fissures.iter().any(|f| f.is_storm), "expected a Void Storm fissure");
    }
}
