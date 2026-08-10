//! warframe-lite — command-line entry point.
//!
//! Orchestrates the overlay, relic picker, mastery, and live-Fissure/market
//! lookups. Running the binary with **no command prints usage** ([`print_help`]);
//! every subcommand is dispatched from `main`. `status` shows live Void Fissures
//! and a bare `<market_slug>` prices that item.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use wf_config::Config;
use wf_data::{http_client, market::MarketClient, worldstate};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "wf_lite=info,wf_data=info,wf_config=info,wf_log=info,wf_overlay=info,wf_relic=info,wf_ocr=info".into()
            }),
        )
        .with_target(false)
        .init();

    // No command (or an explicit help flag) prints usage instead of running
    // anything, so the bare binary is discoverable.
    match std::env::args().nth(1).as_deref() {
        None | Some("help") | Some("-h") | Some("--help") => {
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
        Some("ocr") => return ocr_test(),
        Some("ocr-file") => return ocr_file(),
        Some("relic-file") => return relic_file(&config).await,
        Some("mastery") => return mastery_cmd(&config).await,
        Some("set-account") => return set_account_cmd(&config_path),
        Some("detect-account") => return detect_account_cmd(&config, &config_path).await,
        Some("settings") => return launch_companion("wf-settings"),
        Some("tray") => return launch_companion("wf-tray"),
        Some("browse") => return launch_companion("wf-browse"),
        Some("relic") => return relic_eval(&config).await,
        Some("relics") => return relics_cmd(&config).await,
        Some("mastery-plan") => return mastery_plan_cmd(&config).await,
        Some("relic-guide-png") => return relic_guide_png(&config).await,
        Some("relic-grid-file") => return relic_grid_file().await,
        Some("inventory-grid-file") => return inventory_grid_file().await,
        Some("relic-scan") => return relic_scan(&config).await,
        Some("reward-png") => return reward_png(&config).await,
        #[cfg(feature = "mem-scan")]
        Some("mem-scan") => return mem_scan_cmd().await,
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

/// Print grouped usage. Shown when the binary is run with no command (or `help`).
fn print_help() {
    print!(
        "\
wf-lite — Linux-native Warframe companion (overlay, relic picker, mastery)

USAGE:
    wf-lite <command> [args]        (no command shows this help)

RUN IT
    tray                  Tray icon: waits for the game, auto-runs the overlay
    overlay               Show the live overlay (live fissures + relic picker)
    settings              Open the graphical settings window
    browse                Open the mastery/relic browser (Mastery/Relics/Sell)
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
        "\nPHASE 4 (feature-gated, needs your live confirmation)\n    \
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
async fn mem_scan_cmd() -> Result<()> {
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

/// Launch a companion binary (`wf-settings` GUI, `wf-tray` tray) that lives in a
/// separate crate so the overlay stays lean. Prefers a sibling of this
/// executable, then `PATH`; if it isn't installed, say so.
fn launch_companion(name: &str) -> Result<()> {
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(name)))
        .filter(|p| p.is_file());
    let bin = sibling.unwrap_or_else(|| std::path::PathBuf::from(name));
    match std::process::Command::new(&bin).status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => anyhow::bail!("{name} exited with {status}"),
        Err(e) => anyhow::bail!(
            "could not launch {name} ({}): {e}. Install `{name}` alongside `wf-lite`.",
            bin.display(),
        ),
    }
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

/// Build the reward-screen candidate-centre geometry, applying any
/// `[overlay]` config overrides (see issue #6) on top of the built-in
/// calibration.
fn reward_regions(config: &Config) -> wf_relic::RewardRegions {
    let mut regions = wf_relic::RewardRegions::default_calibration();
    if let Some(pitch) = config.overlay.reward_pitch {
        regions.pitch = pitch;
    }
    if let Some(center_x) = config.overlay.reward_center_x {
        regions.center_x = center_x;
    }
    regions
}

/// Run the full reward pipeline on a saved PNG (calibration/validation):
/// OCR the candidate slots, show each slot's text, pick the real rewards, and
/// rank them. Usage: `wf-lite relic-file <path>`.
async fn relic_file(config: &Config) -> Result<()> {
    let path = std::env::args()
        .nth(2)
        .context("usage: relic-file <path-to-reward.png>")?;
    println!("\n== Relic file: {path} ==");
    let image = image::open(&path)
        .with_context(|| format!("opening {path}"))?
        .to_rgba8();

    let client = http_client();
    let index = wf_relic::ItemIndex::load_cached(&client, CATALOGUE_TTL).await?;
    let regions = reward_regions(config);
    let ocr = wf_ocr::Ocr::new()?;

    let slots = ocr_regions(&image, &ocr, &regions);
    for (i, s) in slots.iter().enumerate() {
        let m = index.best_match(s);
        println!(
            "  slot {i}: {:<32} -> {}",
            format!("{:?}", s),
            m.map(|m| format!("{} ({:.0}%)", m.item.name, m.score * 100.0))
                .unwrap_or_else(|| "—".into())
        );
    }
    let names = wf_relic::select_rewards(&slots, &index);
    println!("  selected {} rewards: {names:?}", names.len());
    if names.len() < 2 {
        return Ok(());
    }

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

/// Normalized (mean-subtracted, unit-norm) grayscale of the bundled "unowned"
/// eye-icon template, plus its dimensions. Decoded once.
fn eye_template() -> &'static (Vec<f32>, usize, usize) {
    static T: std::sync::OnceLock<(Vec<f32>, usize, usize)> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let img = image::load_from_memory(include_bytes!("../assets/relic-unowned-eye.png"))
            .expect("bundled eye template decodes")
            .to_luma8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let vals: Vec<f32> = img.pixels().map(|p| p[0] as f32).collect();
        let mean = vals.iter().sum::<f32>() / vals.len() as f32;
        let centered: Vec<f32> = vals.iter().map(|v| v - mean).collect();
        let norm = centered.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-6);
        (centered.iter().map(|v| v / norm).collect(), w, h)
    })
}

/// Peak brightness-invariant (normalized cross-correlation) match of the "unowned"
/// eye template anywhere in the `eye` search window. NCC is contrast-invariant (a
/// card brightens on hover, but the eye persists) and the window tolerates
/// column-pitch drift. `1.0` = perfect match.
fn eye_ncc(image: &image::RgbaImage, eye: &wf_relic::Rect) -> f32 {
    let (tmpl, tw, th) = eye_template();
    let (rw, rh) = (eye.w as usize, eye.h as usize);
    if rw < *tw || rh < *th {
        return -1.0;
    }
    // Luma of the search window (clamped to the frame).
    let mut luma = vec![0f32; rw * rh];
    for y in 0..rh {
        for x in 0..rw {
            let px = image
                .get_pixel_checked(eye.x + x as u32, eye.y + y as u32)
                .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
                .unwrap_or(0.0);
            luma[y * rw + x] = px;
        }
    }
    let mut best = -1.0f32;
    for oy in 0..=(rh - th) {
        for ox in 0..=(rw - tw) {
            let mut sum = 0.0;
            for ty in 0..*th {
                for tx in 0..*tw {
                    sum += luma[(oy + ty) * rw + ox + tx];
                }
            }
            let mean = sum / (tw * th) as f32;
            let (mut dot, mut nrm) = (0.0f32, 0.0f32);
            for ty in 0..*th {
                for tx in 0..*tw {
                    let pv = luma[(oy + ty) * rw + ox + tx] - mean;
                    dot += pv * tmpl[ty * tw + tx];
                    nrm += pv * pv;
                }
            }
            let ncc = if nrm > 1e-6 { dot / nrm.sqrt() } else { 0.0 };
            best = best.max(ncc);
        }
    }
    best
}

/// Threshold on [`eye_ncc`] above which a card is treated as unowned.
const EYE_THRESHOLD: f32 = 0.5;

/// Whether the "unowned" eye icon appears in `eye` (player does not own the relic).
fn card_has_eye(image: &image::RgbaImage, eye: &wf_relic::Rect) -> bool {
    eye_ncc(image, eye) >= EYE_THRESHOLD
}

/// Upper bound on a plausible per-refinement Intact owned count. Any badge that
/// reads above this is rejected as OCR garbage rather than trusted (see
/// ADR-0005). Deliberately well above a realistic hoard of one relic so genuine
/// large counts still register; the observed failure mode was a spurious "145"
/// for a real "15". Tunable — one of the two scan-calibration knobs.
const RELIC_COUNT_CAP: u32 = 99;

/// Max ink coverage for a name-crop to be treated as a text line rather than
/// relic-orb artwork. A name line is thin strokes on dark (well under this); an
/// orb or item render fills most of the crop. Lets the dense phase sampling skip
/// the many candidates that land on artwork without an OCR call.
const MAX_NAME_COVERAGE: f32 = 0.30;

/// Min horizontal ink spread (fraction of crop width) for a name crop to count
/// as a name *line* during phase alignment. A relic name spans most of the crop;
/// the `xN` count badge one row up is text too but only a narrow left strip, so
/// this stops phase selection from locking onto the badge row.
const MIN_NAME_HSPAN: f32 = 0.35;

/// How many frames must agree on a relic's count before it is believed. Two is
/// enough to defeat a lone OCR outlier while still confirming within a single
/// dwell on the card; mode-voting lets the true value overtake a wrong pair as
/// the player keeps scrolling (see ADR-0005).
const RELIC_AGREEMENT: u32 = 2;

/// How many frames must resolve the same `(code, refinement)` identity before
/// it's marked Seen. `RelicIndex::best_match` still occasionally ties two
/// different real codes on a single garbled read; a lone frame's identity is
/// cheaper to reach agreement on than a full [`RELIC_AGREEMENT`]-frame count
/// match, but requiring a second one guards Seen against acting on a single
/// tie-broken bad match (see ADR-0009).
const SEEN_AGREEMENT: u32 = 2;

/// What a single frame concluded about one card — the relic and Inventory/
/// Sell scanners share this exact shape, so both use [`wf_gridscan::ObsKind`]
/// directly rather than each keeping their own copy.
use wf_gridscan::ObsKind;

/// One relic card's resolved reading on a single frame.
struct RelicObservation {
    /// Relic display code, e.g. "Meso B9".
    display: String,
    refinement: wf_relic::Refinement,
    kind: ObsKind,
}

/// Preprocessing for the relic grid: 4× upscale, light-on-dark threshold.
fn grid_pre() -> wf_ocr::Preprocess {
    wf_gridscan::default_grid_preprocess()
}

/// OCR the Void Relics grid in `image` and resolve each visible card to a
/// `(relic, refinement)` observation, via [`wf_gridscan::scan_grid`] — the
/// screen-agnostic phase-anchored scan/OCR-confirm loop shared with the
/// Inventory/Sell scanner (see ADR-0006). Reads *every* card — including ones
/// flagged with the "unowned" eye icon, whose name we still need so a later
/// scan can zero the right relic — at the single best-aligned vertical phase,
/// collapsing the frame to **one** vote per `(code, refinement)` so the
/// caller's agreement gate counts *frames*, not slots.
fn scan_relic_grid(
    image: &image::RgbaImage,
    ocr: &wf_ocr::Ocr,
    regions: &wf_relic::RelicGridRegions,
    index: &wf_relic::RelicIndex,
) -> Vec<RelicObservation> {
    let resolve = |raw: &str| -> Option<(String, wf_relic::Refinement)> {
        let (base, refinement) = wf_relic::parse_refinement(raw);
        let info = index.best_match(&base)?;
        Some((info.display.clone(), refinement))
    };
    let ownership_signal = |image: &image::RgbaImage, eye: &wf_gridscan::Rect| card_has_eye(image, eye);
    let cfg = wf_gridscan::GridConfig {
        pre: grid_pre(),
        max_name_coverage: MAX_NAME_COVERAGE,
        min_name_hspan: MIN_NAME_HSPAN,
        badge_cap: RELIC_COUNT_CAP,
        resolve: &resolve,
        ownership_signal: Some(&ownership_signal),
    };
    let slots_for_phase = |w: u32, h: u32, phase: f32| -> Vec<wf_gridscan::Slot> {
        regions.slots(w, h, phase).into_iter().map(Into::into).collect()
    };

    wf_gridscan::scan_grid(image, ocr, slots_for_phase, &cfg)
        .into_iter()
        .map(|obs| RelicObservation {
            display: obs.key.0,
            refinement: obs.key.1,
            kind: obs.kind,
        })
        .collect()
}

/// Upper bound on a plausible owned-count reading on the Inventory/Sell
/// screen. Meaningfully higher than the relic grid's cap: a real capture
/// showed `✓303` on a non-Prime item in the (out-of-scope) All tab, and even
/// scoped to the Prime Parts tab a common Blueprint/Systems/Chassis part can
/// pile up well past a relic's realistic hoard (see issue #37's
/// region-calibration research, resolving #32).
const INVENTORY_COUNT_CAP: u32 = 999;

/// How many frames must agree on an Inventory/Sell part's count before it's
/// believed (see ADR-0005) — same floor the relic scanner uses. Unlike the
/// relic scanner there is no separate Seen tier here (see issue #37's
/// catalog-matching decision: this screen's badge is always a single-frame
/// passive read, so a Seen/Confirmed split has nothing to distinguish);
/// identity and count are trusted together, once frames agree.
const INVENTORY_AGREEMENT: u32 = 2;

/// One Inventory/Sell card's resolved reading on a single frame.
struct InventoryObservation {
    part: wf_relic::PrimePart,
    kind: ObsKind,
}

/// OCR the Inventory/Sell screen's Prime Parts grid in `image` and resolve
/// each visible card to a [`wf_relic::PrimePart`] observation, via
/// [`wf_gridscan::scan_grid`] — the same screen-agnostic phase-anchored
/// scan/OCR-confirm loop [`scan_relic_grid`] uses. No ownership-signal icon:
/// this screen never lists a 0-owned card (see issue #37's
/// region-calibration research, resolving #32), so `ObsKind::Unowned` is
/// structurally possible but never produced here.
fn scan_inventory_grid(
    image: &image::RgbaImage,
    ocr: &wf_ocr::Ocr,
    regions: &wf_relic::InventoryGridRegions,
    quantities: &wf_relic::PartQuantities,
) -> Vec<InventoryObservation> {
    let resolve = |raw: &str| wf_relic::inventory_prime_part(raw, quantities);
    let cfg = wf_gridscan::GridConfig {
        pre: grid_pre(),
        max_name_coverage: MAX_NAME_COVERAGE,
        min_name_hspan: MIN_NAME_HSPAN,
        badge_cap: INVENTORY_COUNT_CAP,
        resolve: &resolve,
        ownership_signal: None,
    };
    let slots_for_phase = |w: u32, h: u32, phase: f32| -> Vec<wf_gridscan::Slot> {
        regions.slots(w, h, phase).into_iter().map(Into::into).collect()
    };

    wf_gridscan::scan_grid(image, ocr, slots_for_phase, &cfg)
        .into_iter()
        .map(|obs| InventoryObservation { part: obs.key, kind: obs.kind })
        .collect()
}

/// Calibration: OCR the Inventory/Sell Prime Parts grid from a PNG and print
/// what resolved. `wf-lite inventory-grid-file <path-to-inventory.png>` — use
/// this against a real capture to correct [`wf_relic::InventoryGridRegions`]'s
/// currently-estimated absolute-position constants (see its doc comment).
async fn inventory_grid_file() -> Result<()> {
    let path = std::env::args()
        .nth(2)
        .context("usage: inventory-grid-file <path-to-inventory.png>")?;
    println!("\n== Inventory grid file: {path} ==");
    let image = image::open(&path).with_context(|| format!("opening {path}"))?.to_rgba8();
    let client = http_client();
    let quantities = wf_relic::PartQuantities::load_cached(&client, CATALOGUE_TTL).await?;
    let ocr = wf_ocr::Ocr::new()?;
    let regions = wf_relic::InventoryGridRegions::default_calibration();

    let found = scan_inventory_grid(&image, &ocr, &regions, &quantities);
    println!("  resolved {} card observations:", found.len());
    for o in &found {
        let what = match o.kind {
            ObsKind::Count(n) => format!("x{n}"),
            ObsKind::Abstain => "badge unreadable (abstain)".to_string(),
            ObsKind::Unowned => "UNOWNED (unexpected on this screen)".to_string(),
        };
        println!("    {} {} {what}", o.part.prime, o.part.part);
    }
    Ok(())
}

/// Calibration: OCR the Void Relics grid from a PNG and print what resolved.
/// `wf-lite relic-grid-file <path-to-relics.png>`.
async fn relic_grid_file() -> Result<()> {
    let path = std::env::args()
        .nth(2)
        .context("usage: relic-grid-file <path-to-relics.png>")?;
    println!("\n== Relic grid file: {path} ==");
    let image = image::open(&path)
        .with_context(|| format!("opening {path}"))?
        .to_rgba8();
    let client = http_client();
    let index = wf_relic::RelicIndex::load_cached(&client, CATALOGUE_TTL).await?;
    let ocr = wf_ocr::Ocr::new()?;
    let regions = wf_relic::RelicGridRegions::default_calibration();

    // Per-slot debug (eye NCC + OCR) to calibrate ownership + regions, at the
    // same best-aligned phase the live scanner picks.
    let pre = grid_pre();
    let resolve = |raw: &str| -> Option<(String, wf_relic::Refinement)> {
        let (base, refinement) = wf_relic::parse_refinement(raw);
        let info = index.best_match(&base)?;
        Some((info.display.clone(), refinement))
    };
    let ownership_signal = |image: &image::RgbaImage, eye: &wf_gridscan::Rect| card_has_eye(image, eye);
    let cfg = wf_gridscan::GridConfig {
        pre,
        max_name_coverage: MAX_NAME_COVERAGE,
        min_name_hspan: MIN_NAME_HSPAN,
        badge_cap: RELIC_COUNT_CAP,
        resolve: &resolve,
        ownership_signal: Some(&ownership_signal),
    };
    let slots_for_phase = |w: u32, h: u32, phase: f32| -> Vec<wf_gridscan::Slot> {
        regions.slots(w, h, phase).into_iter().map(Into::into).collect()
    };
    let phase = wf_gridscan::best_phase(&image, slots_for_phase, &cfg);
    for (i, slot) in regions.slots(image.width(), image.height(), phase).iter().enumerate() {
        let ncc = eye_ncc(&image, &slot.eye);
        let raw = ocr
            .recognize(
                &image::imageops::crop_imm(&image, slot.name.x, slot.name.y, slot.name.w, slot.name.h)
                    .to_image(),
                pre,
                wf_ocr::PageMode::Block,
            )
            .unwrap_or_default()
            .replace('\n', " ");
        let (base, refinement) = wf_relic::parse_refinement(&raw);
        let matched = index.best_match(&base).map(|r| r.display.as_str());
        let badge = wf_gridscan::read_badge(&image, &slot.count, &ocr, pre, RELIC_COUNT_CAP);
        println!(
            "  slot {i:2}: eye={ncc:+.2} {} ocr={raw:?} -> {} [{refinement:?}] {}",
            if ncc >= EYE_THRESHOLD { "UNOWNED" } else { "owned  " },
            matched.unwrap_or("—"),
            match badge {
                ObsKind::Count(n) => format!("x{n}"),
                ObsKind::Abstain => "badge?".to_string(),
                ObsKind::Unowned => "-".to_string(),
            }
        );
    }

    let found = scan_relic_grid(&image, &ocr, &regions, &index);
    println!("  resolved {} card observations:", found.len());
    for o in &found {
        let what = match o.kind {
            ObsKind::Count(n) => format!("x{n}"),
            ObsKind::Abstain => "badge unreadable (abstain)".to_string(),
            ObsKind::Unowned => "UNOWNED (eye)".to_string(),
        };
        println!("    {:<16} [{:?}] {what}", o.display, o.refinement);
    }
    Ok(())
}

/// Capture the live reward screen, OCR the candidate slots, pick the real
/// rewards (handles 2–4 centred cards + wrapped names), and rank by plat/ducats.
async fn relic_scan(config: &Config) -> Result<()> {
    println!("\n== Relic scan ==");
    let cap = wf_capture::capture_warframe(None)?;
    println!("  captured {}x{}", cap.image.width(), cap.image.height());

    let client = http_client();
    let index = wf_relic::ItemIndex::load_cached(&client, CATALOGUE_TTL).await?;
    let regions = reward_regions(config);
    let ocr = wf_ocr::Ocr::new()?;

    let slots = ocr_regions(&cap.image, &ocr, &regions);
    let names = wf_relic::select_rewards(&slots, &index);
    if names.len() < 2 {
        println!(
            "  only {} reward name(s) resolved — likely not on the Void Fissure reward screen",
            names.len()
        );
        return Ok(());
    }

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

/// OCR a region of a saved PNG using the real `wf-ocr` pipeline — used to
/// calibrate reward-name crop coordinates and preprocessing against a captured
/// reward screen. Threshold/scale tunable via `WF_OCR_THRESHOLD` / `WF_OCR_SCALE`.
/// Usage: `wf-lite ocr-file <path> <x> <y> <w> <h>`; saves `<path>.pre.png`.
fn ocr_file() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(2).collect();
    anyhow::ensure!(a.len() == 5, "usage: ocr-file <path> <x> <y> <w> <h>");
    let (path, x, y, w, h): (String, u32, u32, u32, u32) = (
        a[0].clone(),
        a[1].parse().context("x")?,
        a[2].parse().context("y")?,
        a[3].parse().context("w")?,
        a[4].parse().context("h")?,
    );

    let threshold: u8 = std::env::var("WF_OCR_THRESHOLD").ok().and_then(|v| v.parse().ok()).unwrap_or(140);
    let scale: u32 = std::env::var("WF_OCR_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    let pre = wf_ocr::Preprocess { scale, threshold, light_text: true };

    let img = image::open(&path).with_context(|| format!("opening {path}"))?.to_rgba8();
    let crop = image::imageops::crop_imm(&img, x, y, w, h).to_image();
    wf_ocr::preprocess(&crop, pre)
        .save(format!("{path}.pre.png"))
        .ok();

    let ocr = wf_ocr::Ocr::new()?;
    let text = ocr.recognize(&crop, pre, wf_ocr::PageMode::Line)?;
    println!("region ({x},{y},{w},{h}) thr={threshold} scale={scale}: '{text}'");
    Ok(())
}

/// Capture the Warframe window (or a sub-region `x y w h`) and OCR it — an
/// end-to-end test of the capture → preprocess → tesseract pipeline.
fn ocr_test() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(2).collect();
    let region = if a.len() == 4 {
        Some(wf_capture::Region {
            x: a[0].parse().context("x")?,
            y: a[1].parse().context("y")?,
            width: a[2].parse().context("w")?,
            height: a[3].parse().context("h")?,
        })
    } else {
        None
    };

    println!("\n== OCR test ==");
    let cap = wf_capture::capture_warframe(region)?;
    println!("  captured {}x{}", cap.image.width(), cap.image.height());

    let ocr = wf_ocr::Ocr::new()?;
    let text = ocr.recognize(&cap.image, wf_ocr::Preprocess::default(), wf_ocr::PageMode::Block)?;
    println!("  --- recognized text ---\n{}\n  --- end ---", text);
    Ok(())
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
/// Deadline the owned-relic scan should keep running until, shared between
/// `relic_watch_loop` (which extends it from `EE.log`) and `relic_scan_loop`
/// (which reads it every iteration to decide whether to scan or idle).
type RelicDeadline = std::sync::Arc<std::sync::Mutex<Option<std::time::Instant>>>;
/// Deadline the owned-Prime-Part scan should keep running until — the
/// Inventory/Sell screen's counterpart to [`RelicDeadline`], extended by
/// whatever detects the Inventory/Sell screen opening and read every
/// iteration by [`inventory_scan_loop`].
type InventoryDeadline = std::sync::Arc<std::sync::Mutex<Option<std::time::Instant>>>;

/// Shared handles the renderer loop needs to background-warm the price cache
/// when a relic tier the player owns becomes crackable (see
/// [`prewarm_new_tiers`]) — the four pieces of the "Relic auto-detection"
/// block's state that pricing actually needs, `None` (via the caller's
/// `Option<PrewarmCtx>`) when that block couldn't load the relic catalogue.
#[derive(Clone)]
struct PrewarmCtx {
    market: MarketClient,
    cache: std::sync::Arc<wf_relic::PriceCache>,
    relic_index: std::sync::Arc<wf_relic::RelicIndex>,
    item_index: std::sync::Arc<wf_relic::ItemIndex>,
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

    // Appearance/visibility knobs. `visible` is flipped at runtime by the control
    // socket (see `overlay_control`); `show_fissures` and `opacity` come from config.
    let visible = Arc::new(AtomicBool::new(true));
    let show_fissures = config.overlay.fissures;
    let opacity = config.overlay.opacity;

    // Build one overlay frame from the current state, honoring reward-only mode,
    // the visibility toggle, and opacity. A hidden or empty frame is a fully
    // transparent (click-through) canvas.
    // Panel priority when shown: reward screen (time-critical, ~20s) → an
    // in-progress relic scan's live status → live fissures → blank. The ranked
    // owned-relic guide (`wf-lite relic-guide-png`'s `render_relic_panel`) isn't
    // shown live here — that view lives in `wf-lite browse` instead, which reads
    // the same `owned-relics.json` without the live overlay's latency budget.
    let make_frame = {
        let font = font.clone();
        let reward = reward.clone();
        let relic_scan_status = relic_scan_status.clone();
        move |ws: &worldstate::WorldState, shown: bool| -> wf_overlay::Canvas {
            let blank = || wf_overlay::Canvas::new(OVERLAY_W, OVERLAY_H);
            let mut c = if !shown {
                blank()
            } else if let Some(rows) = current_reward_rows(&reward) {
                wf_overlay::render_reward_panel(&rows, &font).embed(OVERLAY_W, OVERLAY_H)
            } else if let Some(progress) = *relic_scan_status.lock().unwrap() {
                wf_overlay::render_relic_scanning_panel(progress, &font).embed(OVERLAY_W, OVERLAY_H)
            } else if show_fissures {
                wf_overlay::render_panel(ws, &font).embed(OVERLAY_W, OVERLAY_H)
            } else {
                blank()
            };
            c.scale_alpha(opacity);
            c
        }
    };

    println!("\n== Live overlay (Ctrl-C to stop) ==");
    println!(
        "  placement: {} (margin {}x{}); fissure panel: {}; opacity: {opacity}",
        config.overlay.anchor,
        config.overlay.margin_x,
        config.overlay.margin_y,
        if show_fissures { "on" } else { "reward-only" },
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

    // Control socket: `wf-lite toggle|show|hide` flips `visible` at runtime, and
    // `copy` copies the current best-pick reward, so a KDE global shortcut bound
    // to those commands can act on a running overlay on demand.
    spawn_control_listener(visible.clone(), reward.clone());

    // Relic auto-detection: needs tesseract, an EE.log, and the item catalogue.
    // Built before the renderer loop below so the renderer can wire up
    // background fissure-start price pre-warming (`prewarm_ctx`) from the
    // same catalogues/cache/market this block already loads.
    let mut prewarm_ctx: Option<PrewarmCtx> = None;
    match (wf_ocr::Ocr::new(), config.resolve_ee_log()) {
        (Ok(ocr), Ok(ee_log)) => match wf_relic::ItemIndex::load_cached(&client, CATALOGUE_TTL).await {
            Ok(index) => {
                let index = Arc::new(index);
                let cache = Arc::new(wf_relic::price_cache());
                let mastery = Arc::new(load_mastery(&config, &client).await);
                // Relic drop tables for the owned-relic guide (best-effort).
                let relic_index = wf_relic::RelicIndex::load_cached(&client, CATALOGUE_TTL)
                    .await
                    .ok()
                    .map(Arc::new);
                // Reward-name → vaulted-status lookup (best-effort, empty if the
                // relic drop tables above didn't load) — computed once here since
                // it's a pure join over already-cached catalogues, not something
                // to redo on every reward-screen detection.
                let vaulted: Arc<HashMap<String, bool>> = Arc::new(
                    relic_index
                        .as_ref()
                        .map(|ri| wf_relic::vaulted_rewards(ri, &index))
                        .unwrap_or_default(),
                );
                // Prime Part build quantities, doubling as the Inventory/Sell
                // scanner's catalog-matching authority (see
                // `wf_relic::inventory_prime_part`) — best-effort, same as
                // the relic catalogue above.
                let part_quantities = wf_relic::PartQuantities::load_cached(&client, CATALOGUE_TTL)
                    .await
                    .ok()
                    .map(Arc::new);
                println!(
                    "  relic auto-detect: ON ({} items; {} relics; {} cached prices; {} mastered; watching {})",
                    index.len(),
                    relic_index.as_ref().map_or(0, |r| r.len()),
                    cache.len(),
                    mastery.len(),
                    ee_log.display()
                );
                let market = MarketClient::new(client.clone(), config.market_platform.clone());
                let ocr = Arc::new(ocr);
                let reward = reward.clone();

                // Background price pre-warm (see `prewarm_new_tiers` and the
                // renderer loop below): only possible once the relic drop
                // tables have loaded, since it needs a relic's reward pool.
                prewarm_ctx = relic_index.clone().map(|relic_index| PrewarmCtx {
                    market: market.clone(),
                    cache: cache.clone(),
                    relic_index,
                    item_index: index.clone(),
                });

                // The owned-relic scan runs in its own tightly-looped task (see
                // relic_scan_loop) so it isn't throttled by relic_watch_loop's
                // POLL_INTERVAL or by how long a scan itself takes — the Relics
                // list scrolls continuously, so sampling as fast as possible is
                // what actually catches it, not waiting out a fixed interval.
                let relic_deadline: Option<RelicDeadline> = if let Some(ridx) = relic_index.clone() {
                    let deadline: RelicDeadline = Arc::new(Mutex::new(None));
                    tokio::spawn(relic_scan_loop(
                        deadline.clone(),
                        ocr.clone(),
                        ridx,
                        wf_relic::RelicGridRegions::default_calibration(),
                        relic_scan_status.clone(),
                    ));
                    Some(deadline)
                } else {
                    None
                };

                // The owned-Prime-Part scan mirrors the owned-relic scan
                // above: its own tightly-looped task, armed by an EE.log
                // event via `relic_watch_loop`, independent of both the
                // relic scan and reward-screen detection.
                let inventory_deadline: Option<InventoryDeadline> =
                    if let Some(quantities) = part_quantities.clone() {
                        let deadline: InventoryDeadline = Arc::new(Mutex::new(None));
                        tokio::spawn(inventory_scan_loop(
                            deadline.clone(),
                            ocr.clone(),
                            quantities,
                            wf_relic::InventoryGridRegions::default_calibration(),
                        ));
                        Some(deadline)
                    } else {
                        None
                    };

                let reward_regions = reward_regions(&config);
                tokio::spawn(async move {
                    if let Err(e) = relic_watch_loop(
                        ee_log,
                        ocr,
                        index,
                        market,
                        cache,
                        mastery,
                        vaulted,
                        reward,
                        reward_regions,
                        relic_deadline,
                        inventory_deadline,
                    )
                    .await
                    {
                        tracing::error!("relic watcher stopped: {e:#}");
                    }
                });
            }
            Err(e) => println!("  relic auto-detect: OFF (item catalogue load failed: {e:#})"),
        },
        (Err(e), _) => println!("  relic auto-detect: OFF ({e})"),
        (_, Err(e)) => println!("  relic auto-detect: OFF (no EE.log: {e:#})"),
    }

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
                                    tokio::spawn(prewarm_new_tiers(ctx.clone(), new_tiers));
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
    tokio::task::spawn_blocking(move || wf_overlay::layer::run(initial, rx, placement, window))
        .await
        .context("overlay thread panicked")?
}

/// Filesystem path of the overlay's control socket. Placed in the per-user
/// runtime dir when available, falling back to the temp dir.
fn control_socket_path() -> std::path::PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join("warframe-lite-overlay.sock")
}

/// Listen on the control socket for `toggle` / `show` / `hide` / `copy` lines
/// and act on the shared `visible` flag / `reward` state. A stale socket file
/// from a previous run is removed first. Runs for the life of the overlay.
fn spawn_control_listener(visible: std::sync::Arc<std::sync::atomic::AtomicBool>, reward: RewardState) {
    use std::sync::atomic::Ordering;
    use tokio::io::AsyncReadExt;

    let path = control_socket_path();
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
                other => tracing::warn!("unknown overlay control command {other:?}"),
            }
        }
    });
}

/// Env var to override the `wl-copy` binary path (mirrors `WF_TESSERACT`), for
/// users with a differently-named/pathed clipboard binary.
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
/// same tradeoff as the existing `WF_TESSERACT` shell-out; rely on manual
/// verification instead.
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
    use std::io::Write;

    let path = control_socket_path();
    match std::os::unix::net::UnixStream::connect(&path) {
        Ok(mut stream) => {
            stream
                .write_all(cmd.as_bytes())
                .with_context(|| format!("sending {cmd:?} to overlay"))?;
            println!("sent '{cmd}' to the overlay");
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound
            || e.kind() == std::io::ErrorKind::ConnectionRefused =>
        {
            anyhow::bail!("no overlay is running (control socket {} absent)", path.display())
        }
        Err(e) => Err(e).with_context(|| format!("connecting to overlay at {}", path.display())),
    }
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

/// Load the current owned-relic set from disk and warm the price cache for
/// every tradable reward of an owned relic whose tier is in `tiers` (see
/// [`wf_relic::active_tier_reward_names`]) — the actual pre-warm work behind
/// the renderer loop's newly-active-tier dispatch. Spawned as its own
/// detached task each time new tiers appear, so a slow warframe.market
/// response never delays the next rendered frame.
/// [`wf_relic::PriceOpts::fetch_timeout`] used for pre-warming, well past
/// [`wf_relic::PriceOpts::default`]'s 2.5s — that default is tuned for the
/// ~15s reward-screen selection window, but a pre-warm runs minutes ahead of
/// it with no such deadline, so a slow warframe.market response should get a
/// real chance to land here instead of timing out and leaving the item to
/// warm cold later anyway.
const PREWARM_FETCH_TIMEOUT: Duration = Duration::from_secs(8);

async fn prewarm_new_tiers(ctx: PrewarmCtx, tiers: std::collections::HashSet<String>) {
    let owned = wf_cache::load_blob::<wf_relic::OwnedRelics>(wf_relic::OWNED_RELICS_FILE)
        .map(|s| s.value)
        .unwrap_or_default();
    let evidence = wf_relic::owned_evidence(&owned);
    let names = wf_relic::active_tier_reward_names(&evidence, &ctx.relic_index, &tiers);
    if names.is_empty() {
        return;
    }
    tracing::info!(
        "fissure pre-warm: tier(s) {} active — warming {} reward price(s)",
        tiers.iter().cloned().collect::<Vec<_>>().join(", "),
        names.len()
    );
    let opts = wf_relic::PriceOpts { fetch_timeout: PREWARM_FETCH_TIMEOUT, ..wf_relic::PriceOpts::default() };
    wf_relic::prewarm_reward_prices(&names, &ctx.item_index, &ctx.market, &ctx.cache, opts).await;
}

/// How long a relic-inventory-open event keeps [`relic_scan_loop`] scanning
/// (shared: `relic_watch_loop` extends the deadline, `relic_scan_loop` reads it).
const RELIC_SCAN_WINDOW: Duration = Duration::from_secs(180);
/// [`RELIC_SCAN_WINDOW`]'s counterpart for [`inventory_scan_loop`], armed by
/// `Event::InventorySellOpen`.
const INVENTORY_SCAN_WINDOW: Duration = Duration::from_secs(180);

/// Watch the EE.log for a relic **crack** / reward-screen line, which opens a
/// poll window scanned until the 4-choice screen resolves (publishing the
/// ranked reward to `reward`). A **Relics inventory** open
/// (`RelicInventoryOpen`) instead extends `relic_deadline`, and an
/// **Inventory/Sell** open (`InventorySellOpen`) extends `inventory_deadline`
/// the same way — the actual owned-relic/owned-Prime-Part scanning happens in
/// their own separate, tightly-looped tasks ([`relic_scan_loop`],
/// [`inventory_scan_loop`]), so a slow or expensive scan never throttles
/// reward-screen detection (and vice versa: reward-screen debounce logic
/// never throttles either scan).
#[allow(clippy::too_many_arguments)]
async fn relic_watch_loop(
    ee_log: std::path::PathBuf,
    ocr: std::sync::Arc<wf_ocr::Ocr>,
    index: std::sync::Arc<wf_relic::ItemIndex>,
    market: MarketClient,
    cache: std::sync::Arc<wf_relic::PriceCache>,
    mastery: std::sync::Arc<wf_relic::MasterySet>,
    vaulted: std::sync::Arc<HashMap<String, bool>>,
    reward: RewardState,
    regions: wf_relic::RewardRegions,
    relic_deadline: Option<RelicDeadline>,
    inventory_deadline: Option<InventoryDeadline>,
) -> Result<()> {
    use std::time::Instant;
    use wf_log::Event;

    // After a crack (or a reward-screen line), poll the screen for this long.
    // Covers the ~1min crack→screen gap, log-flush lag, and the player bringing
    // the screen up manually (Tab / progress).
    const POLL_WINDOW: Duration = Duration::from_secs(150);
    // Also bounds how quickly a RelicInventoryOpen line arms relic_scan_loop's
    // deadline — kept short so opening the Relics screen registers almost
    // immediately. Cheap now that a reward-screen scan attempt (when active) is
    // a single in-process OCR pass rather than a CLI subprocess (ADR-0008).
    const POLL_INTERVAL: Duration = Duration::from_millis(500);
    // Don't re-show the same 15s screen repeatedly.
    const SHOW_DEBOUNCE: Duration = Duration::from_secs(20);

    let mut tailer = wf_log::LogTailer::from_end(&ee_log);
    let mut scan_until: Option<Instant> = None;
    let mut window_open = false; // whether we've logged the current window
    let mut last_shown = Instant::now() - SHOW_DEBOUNCE;

    loop {
        tokio::time::sleep(POLL_INTERVAL).await;

        // Fold new log lines into the polling-window state.
        for line in tailer.poll().unwrap_or_default() {
            match wf_log::event_from_line(&line) {
                Some(Event::RelicCrack) | Some(Event::RelicRewardScreen) => {
                    scan_until = Some(Instant::now() + POLL_WINDOW);
                    if !window_open {
                        window_open = true;
                        tracing::info!("relic activity — watching for the reward screen");
                    }
                }
                Some(Event::RelicInventoryOpen) => {
                    if let Some(d) = &relic_deadline {
                        *d.lock().unwrap() = Some(Instant::now() + RELIC_SCAN_WINDOW);
                    }
                }
                Some(Event::InventorySellOpen) => {
                    if let Some(d) = &inventory_deadline {
                        *d.lock().unwrap() = Some(Instant::now() + INVENTORY_SCAN_WINDOW);
                    }
                }
                _ => {}
            }
        }

        // --- Reward screen scan -------------------------------------------
        let active = scan_until.is_some_and(|t| Instant::now() < t);
        if !active {
            window_open = false;
            continue;
        }
        if last_shown.elapsed() < SHOW_DEBOUNCE {
            continue;
        }

        // Scan the screen. If the reward screen is up, ≥2 slots resolve.
        let (ocr2, regions2) = (ocr.clone(), regions.clone());
        let slots = match tokio::task::spawn_blocking(move || capture_and_ocr(&ocr2, &regions2)).await
        {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => {
                tracing::warn!("reward capture failed: {e:#}");
                continue;
            }
            Err(e) => {
                tracing::warn!("scan task join error: {e}");
                continue;
            }
        };
        let mut names = wf_relic::select_rewards(&slots, &index);
        if names.len() < 2 {
            continue; // screen not up yet (or already gone) — keep polling
        }

        // The reward cards fan in with a brief entrance animation, so this first
        // hit can catch the screen mid-render with a card's text not yet legible
        // (see the "only detected 3 of 4 parts" report). Resample a few times
        // over the settle window and keep whichever pass resolved the most
        // rewards, rather than locking in an incomplete first read.
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let (ocr2, regions2) = (ocr.clone(), regions.clone());
            let Ok(Ok(slots)) =
                tokio::task::spawn_blocking(move || capture_and_ocr(&ocr2, &regions2)).await
            else {
                continue;
            };
            let found = wf_relic::select_rewards(&slots, &index);
            if found.len() > names.len() {
                names = found;
            }
        }

        let evals = wf_relic::evaluate_cached(
            &names,
            &index,
            &market,
            &cache,
            wf_relic::PriceOpts::default(),
            &vaulted,
        )
        .await;
        // Re-read fresh each time: the Inventory/Sell scanner (a separate
        // task) can update this file independently of this loop's cadence.
        let owned_parts =
            wf_cache::load_blob::<wf_relic::OwnedPrimeParts>(wf_relic::OWNED_PRIME_PARTS_FILE)
                .map(|s| s.value)
                .unwrap_or_default();
        // Also re-read fresh: `wf-browse`'s Mastery tab can mark/unmark a
        // wishlist entry directly (ADR-0004) while this loop keeps running.
        let wishlist = wf_cache::load_blob::<wf_relic::Wishlist>(wf_relic::WISHLIST_FILE)
            .map(|s| s.value)
            .unwrap_or_default();
        let rows = reward_rows(&evals, &mastery, &owned_parts, &wishlist);
        if let Some(best) = wf_relic::best_pick(&evals, &mastery) {
            tracing::info!("reward screen captured — best pick = {}", rows[best].name);
        }
        *reward.lock().unwrap() = Some((Instant::now(), rows));
        last_shown = Instant::now();
    }
}

/// Scan the owned-relic grid on its own tight cadence, independent of
/// `relic_watch_loop`'s fixed `POLL_INTERVAL`.
///
/// The Relics list scrolls **continuously** (not snapped to row boundaries), so
/// any single frame's fixed grid positions only line up with the real card
/// text some of the time (see [`wf_relic::RelicGridRegions::row_phases`] for the
/// per-frame half of the fix). The other half is here: sampling as fast as the
/// hardware allows — rather than waiting out a fixed poll interval on top of
/// however long a scan itself takes — is what actually catches more of the
/// list quickly, since each additional sample is another chance to catch cards
/// at a favorable scroll offset.
/// [`wf_gridscan::ScanLoopBody`] for the Void Relics screen: on top of the
/// shared deadline/idle/cadence skeleton, tracks per-session tally/identity
/// bookkeeping and the overlay's scan-progress indicator (see ADR-0009's
/// Seen tier for why relic tracking needs `identity_reads`/`session_seen_count`
/// where the simpler Inventory/Sell scanner does not).
struct RelicScanBody {
    ocr: std::sync::Arc<wf_ocr::Ocr>,
    relic_index: std::sync::Arc<wf_relic::RelicIndex>,
    relic_regions: wf_relic::RelicGridRegions,
    scan_status: RelicScanStatus,
    // `relic_owned` is the cumulative, disk-persisted owned set, keyed per
    // (code, refinement) (see ADR-0005) — it survives restarts and
    // Relics-screen visits, so the mastery planner (`wf-lite mastery-plan`)
    // has data even outside a live scan. Within the *current* continuous scan
    // (cleared each fresh open), `tally` votes across frames and only
    // confirms a count once enough frames agree, and `session_applied`
    // records which confirmed value we've already written this session so a
    // stable count refreshes its last-seen stamp once, not on every frame.
    relic_owned: wf_relic::OwnedRelics,
    tally: wf_ocr::Tally<(String, wf_relic::Refinement)>,
    session_applied: std::collections::HashMap<(String, wf_relic::Refinement), u32>,
    identity_reads: std::collections::HashMap<(String, wf_relic::Refinement), u32>,
    session_seen_count: usize,
}

impl wf_gridscan::ScanLoopBody for RelicScanBody {
    fn activate(&mut self) {
        self.tally = wf_ocr::Tally::new();
        self.session_applied.clear();
        self.identity_reads.clear();
        self.session_seen_count = 0;
        // Show scanning has started before the first capture even completes,
        // rather than nothing until a relic clears its trust bar.
        *self.scan_status.lock().unwrap() = Some(wf_overlay::ScanProgress::default());
        tracing::info!(
            "relics screen opened — scanning as you scroll ({} relics known from before)",
            self.relic_owned.len()
        );
    }

    fn deactivate(&mut self) {
        *self.scan_status.lock().unwrap() = None;
    }

    async fn tick(&mut self) -> bool {
        let (ocr2, regions2, ridx2) =
            (self.ocr.clone(), self.relic_regions.clone(), self.relic_index.clone());
        let scanned = tokio::task::spawn_blocking(move || {
            let t0 = std::time::Instant::now();
            let cap = wf_capture::capture_warframe(None)?;
            let captured = t0.elapsed();
            let found = scan_relic_grid(&cap.image, &ocr2, &regions2, &ridx2);
            tracing::debug!(
                "relic scan cycle: capture {captured:?}, scan {:?}",
                t0.elapsed() - captured
            );
            Ok::<_, anyhow::Error>(found)
        })
        .await;
        let Ok(Ok(found)) = scanned else {
            return false;
        };

        let mut changed = false;
        for obs in found {
            let key = (obs.display.clone(), obs.refinement);
            // Any cleanly-resolved identity marks Seen, independent of count (ADR-0009).
            match obs.kind {
                ObsKind::Count(n) => {
                    if mark_seen_if_agreed(&mut self.identity_reads, &mut self.relic_owned, &key) {
                        changed = true;
                        self.session_seen_count += 1;
                    }
                    self.tally.record(key.clone(), n);
                }
                ObsKind::Unowned => self.tally.record(key.clone(), 0), // 0 = confirmed unowned
                ObsKind::Abstain => {
                    if mark_seen_if_agreed(&mut self.identity_reads, &mut self.relic_owned, &key) {
                        changed = true;
                        self.session_seen_count += 1;
                    }
                    continue;
                }
            }
            let relic_owned = &mut self.relic_owned;
            if wf_gridscan::confirm_once(&self.tally, &mut self.session_applied, &key, RELIC_AGREEMENT, |confirmed| {
                wf_relic::apply_confirmed_count(relic_owned, &key.0, key.1, confirmed);
            }) {
                changed = true;
            }
        }
        if changed {
            *self.scan_status.lock().unwrap() = Some(wf_overlay::ScanProgress {
                seen: self.session_seen_count,
                confirmed: self.session_applied.len(),
            });
            let _ = wf_cache::save_blob(wf_relic::OWNED_RELICS_FILE, &self.relic_owned);
            tracing::info!("relic scan: {} relics owned", self.relic_owned.len());
        }
        changed
    }
}

async fn relic_scan_loop(
    relic_deadline: RelicDeadline,
    ocr: std::sync::Arc<wf_ocr::Ocr>,
    relic_index: std::sync::Arc<wf_relic::RelicIndex>,
    relic_regions: wf_relic::RelicGridRegions,
    scan_status: RelicScanStatus,
) {
    // Absent, or a legacy/foreign format we can no longer trust — back up any
    // existing file and start clean (ADR-0005).
    let relic_owned: wf_relic::OwnedRelics = wf_cache::load_blob_or_reset(wf_relic::OWNED_RELICS_FILE);

    let body = RelicScanBody {
        ocr,
        relic_index,
        relic_regions,
        scan_status,
        relic_owned,
        tally: wf_ocr::Tally::new(),
        session_applied: std::collections::HashMap::new(),
        identity_reads: std::collections::HashMap::new(),
        session_seen_count: 0,
    };
    wf_gridscan::run_scan_loop(relic_deadline, wf_gridscan::ScanCadence::default(), body).await;
}

/// Bump `key`'s identity-read counter and mark Seen once it reaches
/// [`SEEN_AGREEMENT`]. Returns whether this call actually marked Seen (a new
/// change worth persisting).
fn mark_seen_if_agreed(
    identity_reads: &mut std::collections::HashMap<(String, wf_relic::Refinement), u32>,
    relic_owned: &mut wf_relic::OwnedRelics,
    key: &(String, wf_relic::Refinement),
) -> bool {
    let reads = identity_reads.entry(key.clone()).or_insert(0);
    *reads += 1;
    *reads >= SEEN_AGREEMENT && wf_relic::mark_seen(relic_owned, &key.0, key.1)
}

/// [`wf_gridscan::ScanLoopBody`] for the Inventory/Sell screen's Prime Parts
/// grid — simpler than [`RelicScanBody`] in one respect: no Seen tier (see
/// issue #37's catalog-matching decision — this screen's badge is always a
/// single-frame passive read, so a Seen/Confirmed split has nothing to
/// distinguish), so a card only ever needs [`INVENTORY_AGREEMENT`] agreeing
/// frames before its count is applied.
struct InventoryScanBody {
    ocr: std::sync::Arc<wf_ocr::Ocr>,
    quantities: std::sync::Arc<wf_relic::PartQuantities>,
    inventory_regions: wf_relic::InventoryGridRegions,
    owned: wf_relic::OwnedPrimeParts,
    tally: wf_ocr::Tally<wf_relic::PrimePart>,
    session_applied: std::collections::HashMap<wf_relic::PrimePart, u32>,
}

impl wf_gridscan::ScanLoopBody for InventoryScanBody {
    fn activate(&mut self) {
        self.tally = wf_ocr::Tally::new();
        self.session_applied.clear();
        tracing::info!(
            "inventory/sell screen opened — scanning as you scroll ({} parts known from before)",
            self.owned.values().map(|m| m.len()).sum::<usize>()
        );
    }

    async fn tick(&mut self) -> bool {
        let (ocr2, regions2, quantities2) =
            (self.ocr.clone(), self.inventory_regions.clone(), self.quantities.clone());
        let scanned = tokio::task::spawn_blocking(move || {
            let t0 = std::time::Instant::now();
            let cap = wf_capture::capture_warframe(None)?;
            let captured = t0.elapsed();
            let found = scan_inventory_grid(&cap.image, &ocr2, &regions2, &quantities2);
            tracing::debug!(
                "inventory scan cycle: capture {captured:?}, scan {:?}",
                t0.elapsed() - captured
            );
            Ok::<_, anyhow::Error>(found)
        })
        .await;
        let Ok(Ok(found)) = scanned else {
            return false;
        };

        let mut changed = false;
        for obs in found {
            let ObsKind::Count(n) = obs.kind else {
                continue; // abstain — no vote (Unowned never occurs on this screen)
            };
            self.tally.record(obs.part.clone(), n);
            let owned = &mut self.owned;
            if wf_gridscan::confirm_once(
                &self.tally,
                &mut self.session_applied,
                &obs.part,
                INVENTORY_AGREEMENT,
                |confirmed| wf_relic::owned_parts::apply_count(owned, &obs.part, confirmed),
            ) {
                changed = true;
            }
        }
        if changed {
            let _ = wf_cache::save_blob(wf_relic::OWNED_PRIME_PARTS_FILE, &self.owned);
            tracing::info!(
                "inventory scan: {} parts owned",
                self.owned.values().map(|m| m.len()).sum::<usize>()
            );
        }
        changed
    }
}

/// Scan the Inventory/Sell screen's Prime Parts grid on its own tight
/// cadence, mirroring [`relic_scan_loop`] — same continuous-scroll,
/// phase-anchored sampling rationale (see [`wf_relic::InventoryGridRegions`]),
/// same disk-persisted cumulative owned set (see [`InventoryScanBody`]).
async fn inventory_scan_loop(
    inventory_deadline: InventoryDeadline,
    ocr: std::sync::Arc<wf_ocr::Ocr>,
    quantities: std::sync::Arc<wf_relic::PartQuantities>,
    inventory_regions: wf_relic::InventoryGridRegions,
) {
    // Absent, or a legacy/foreign format we can no longer trust — back up any
    // existing file and start clean (ADR-0005).
    let owned: wf_relic::OwnedPrimeParts = wf_cache::load_blob_or_reset(wf_relic::OWNED_PRIME_PARTS_FILE);

    let body = InventoryScanBody {
        ocr,
        quantities,
        inventory_regions,
        owned,
        tally: wf_ocr::Tally::new(),
        session_applied: std::collections::HashMap::new(),
    };
    wf_gridscan::run_scan_loop(inventory_deadline, wf_gridscan::ScanCadence::default(), body).await;
}

/// OCR each candidate reward-name slot of an already-captured frame. Slots are
/// read as two-line blocks (long reward names wrap). Returns one string per
/// candidate slot; callers use [`wf_relic::select_rewards`] to keep the real
/// rewards.
fn ocr_regions(
    image: &image::RgbaImage,
    ocr: &wf_ocr::Ocr,
    regions: &wf_relic::RewardRegions,
) -> Vec<String> {
    regions
        .candidate_slots(image.width(), image.height())
        .iter()
        .map(|r| {
            let crop = image::imageops::crop_imm(image, r.x, r.y, r.w, r.h).to_image();
            ocr.recognize(&crop, wf_ocr::Preprocess::default(), wf_ocr::PageMode::Block)
                .unwrap_or_default()
                .replace('\n', " ")
        })
        .collect()
}

/// Blocking: capture the Warframe window and OCR all candidate reward slots.
fn capture_and_ocr(ocr: &wf_ocr::Ocr, regions: &wf_relic::RewardRegions) -> Result<Vec<String>> {
    let cap = wf_capture::capture_warframe(None)?;
    Ok(ocr_regions(&cap.image, ocr, regions))
}

/// Render the live world-state panel offscreen to a PNG (for visual verification
/// of the overlay renderer without needing the on-screen layer surface).
async fn overlay_png(config: &Config, out: Option<String>) -> Result<()> {
    let out = out.unwrap_or_else(|| "overlay.png".to_string());
    println!("\n== Rendering overlay panel ==");
    let ws = worldstate::fetch(&http_client(), &config.platform).await?;
    let font = wf_overlay::load_font()?;
    let canvas = wf_overlay::render_panel(&ws, &font);
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
