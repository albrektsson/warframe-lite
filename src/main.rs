//! warframe-lite — command-line entry point.
//!
//! Orchestrates the overlay, relic picker, mastery, and world-state/market
//! lookups. Running the binary with **no command prints usage** ([`print_help`]);
//! every subcommand is dispatched from `main`. `status` shows live world state and
//! a bare `<market_slug>` prices that item.

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
        Some(cmd @ ("toggle" | "show" | "hide")) => return overlay_control(cmd),
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
        Some("relic-scan") => return relic_scan(&config).await,
        Some("reward-png") => return reward_png(&config).await,
        _ => {}
    }

    let client = http_client();

    // --- World state ------------------------------------------------------
    println!("\n== World state ({}) ==", config.platform);
    match worldstate::fetch(&client, &config.platform).await {
        Ok(ws) => print_worldstate(&ws),
        Err(e) => println!("  worldstate fetch failed: {e:#}"),
    }

    // --- Optional market lookup ------------------------------------------
    // Reached by `status` (world state only) or a bare `<slug>` (world state +
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
    overlay               Show the live overlay (world state + relic picker)
    settings              Open the graphical settings window
    browse                Open the mastery/relic browser (Mastery/Relics/Sell)
    toggle | show | hide  Show/hide a running overlay

RELICS & MASTERY
    relics <codes…>       Owned-relic guide: unmastered rewards + prices
    mastery-plan          Unmastered primes + which of your relics drop them
    mastery [id]          Report how many items you've mastered
    detect-account        Auto-detect your account id from EE.log
    set-account <id>      Save your account id for mastery lookup

WORLD STATE & PRICES
    status                Show live fissures, Baro, and world cycles
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
}

fn print_worldstate(ws: &worldstate::WorldState) {
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

    let vt = &ws.void_trader;
    if vt.active {
        println!("  Baro: HERE at {} (leaves in {})", vt.location, vt.leaves_in());
    } else if !vt.location.is_empty() {
        println!("  Baro: {} (arrives in {})", vt.location, vt.arrives_in());
    }

    print_cycle("Cetus", &ws.cetus_cycle);
    print_cycle("Vallis", &ws.vallis_cycle);
    print_cycle("Cambion", &ws.cambion_cycle);
}

fn print_cycle(name: &str, c: &worldstate::Cycle) {
    if !c.state.is_empty() {
        println!("  {name}: {} ({} left)", c.state, c.time_left());
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

    let evals =
        wf_relic::evaluate_cached(&names, &index, &market, &cache, wf_relic::PriceOpts::default())
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
    let intact = wf_relic::intact_counts(&owned.value);
    let plans = wf_relic::mastery_plan(&intact, &index, &mastery);

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

    println!("  {:<24} {:>6}  relics you own that can still drop it", "unmastered prime", "owned");
    for p in &plans {
        let breakdown = p
            .relics
            .iter()
            .map(|r| {
                let live = if active_tiers.contains(wf_relic::tier_of(&r.relic_display)) { "*" } else { "" };
                format!("{}{live} x{}", r.relic_display, r.owned_count)
            })
            .collect::<Vec<_>>()
            .join(", ");
        println!("  {:<24} {:>6}  {breakdown}", truncate_str(&p.prime, 24), p.total_owned);
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
    let regions = wf_relic::RewardRegions::default_calibration();
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
    let evals =
        wf_relic::evaluate_cached(&names, &index, &market, &cache, wf_relic::PriceOpts::default())
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

/// How many frames must agree on a relic's count before it is believed. Two is
/// enough to defeat a lone OCR outlier while still confirming within a single
/// dwell on the card; mode-voting lets the true value overtake a wrong pair as
/// the player keeps scrolling (see ADR-0005).
const RELIC_AGREEMENT: u32 = 2;

/// One relic card's resolved reading on a single frame.
struct RelicObservation {
    /// Relic display code, e.g. "Meso B9".
    display: String,
    refinement: wf_relic::Refinement,
    kind: ObsKind,
}

/// What a single frame concluded about one relic card.
enum ObsKind {
    /// The count badge read as this value (a genuinely blank badge → 1, since a
    /// single copy shows no badge on the Void Relics screen).
    Count(u32),
    /// The card resolved to a relic but its badge was present-yet-unreadable, so
    /// this frame casts no count vote rather than guessing.
    Abstain,
    /// The "unowned" eye icon is present — positive proof the player owns zero.
    Unowned,
}

/// OCR the Void Relics grid in `image` and resolve each visible card to a
/// `(relic, refinement)` observation. Unlike the old max-merge scanner this reads
/// *every* card — including ones flagged with the "unowned" eye icon, whose name
/// we now need so a later scan can zero the right relic — and it collapses each
/// frame to **one** vote per `(code, refinement)`: a frame that reads a card two
/// ways (across sampled row phases) abstains rather than double-voting, so the
/// caller's agreement gate really counts *frames*, not slots.
fn scan_relic_grid(
    image: &image::RgbaImage,
    ocr: &wf_ocr::Ocr,
    regions: &wf_relic::RelicGridRegions,
    index: &wf_relic::RelicIndex,
) -> Vec<RelicObservation> {
    let pre = wf_ocr::Preprocess { scale: 4, threshold: 140, light_text: true };
    let slots = regions.slots(image.width(), image.height());

    // OCR every card concurrently — each slot shells out to tesseract, so ~32
    // calls run in parallel across cores instead of ~2s×32 serially.
    let resolved: Vec<Option<(String, wf_relic::Refinement, ObsKind)>> =
        std::thread::scope(|scope| {
            let handles: Vec<_> = slots
                .iter()
                .map(|slot| {
                    scope.spawn(move || {
                        let name_crop = image::imageops::crop_imm(
                            image, slot.name.x, slot.name.y, slot.name.w, slot.name.h,
                        )
                        .to_image();
                        let raw = ocr
                            .recognize(&name_crop, pre, wf_ocr::PageMode::Line)
                            .unwrap_or_default()
                            .replace('\n', " ");
                        let (base, refinement) = wf_relic::parse_refinement(&raw);
                        let info = index.best_match(&base)?;
                        // Read the name even on eye cards (done above) so we can
                        // attribute the "unowned" proof to the right relic.
                        let kind = if card_has_eye(image, &slot.eye) {
                            ObsKind::Unowned
                        } else {
                            read_count_badge(image, &slot.count, ocr, pre)
                        };
                        Some((info.display.clone(), refinement, kind))
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

    dedupe_frame(resolved.into_iter().flatten())
}

/// Read one card's count badge: a blank crop means the player owns exactly one
/// (singles show no badge); otherwise a strict `x?NN` parse, abstaining on
/// anything unreadable (see [`wf_ocr::parse_badge`]).
fn read_count_badge(
    image: &image::RgbaImage,
    count: &wf_relic::Rect,
    ocr: &wf_ocr::Ocr,
    pre: wf_ocr::Preprocess,
) -> ObsKind {
    let crop =
        image::imageops::crop_imm(image, count.x, count.y, count.w, count.h).to_image();
    if wf_ocr::is_blank(&crop, pre) {
        return ObsKind::Count(1);
    }
    let text = ocr.recognize(&crop, pre, wf_ocr::PageMode::Line).unwrap_or_default();
    match wf_ocr::parse_badge(&text, RELIC_COUNT_CAP) {
        Some(n) => ObsKind::Count(n),
        None => ObsKind::Abstain,
    }
}

/// Collapse a frame's per-slot reads to one [`RelicObservation`] per
/// `(code, refinement)`. Owned reads win over an eye flag for the same card
/// (never zero on an ambiguous frame); the owned count is trusted only if the
/// slots that read a value agree, otherwise the frame abstains for that card.
fn dedupe_frame(
    reads: impl Iterator<Item = (String, wf_relic::Refinement, ObsKind)>,
) -> Vec<RelicObservation> {
    use std::collections::HashMap;
    let mut groups: HashMap<(String, wf_relic::Refinement), Vec<ObsKind>> = HashMap::new();
    for (display, refinement, kind) in reads {
        groups.entry((display, refinement)).or_default().push(kind);
    }
    groups
        .into_iter()
        .map(|((display, refinement), kinds)| {
            let counts: Vec<u32> = kinds
                .iter()
                .filter_map(|k| if let ObsKind::Count(n) = k { Some(*n) } else { None })
                .collect();
            let owned_reads =
                counts.len() + kinds.iter().filter(|k| matches!(k, ObsKind::Abstain)).count();
            let has_unowned = kinds.iter().any(|k| matches!(k, ObsKind::Unowned));
            let kind = if owned_reads == 0 && has_unowned {
                ObsKind::Unowned
            } else if let Some(&first) = counts.first() {
                // Trust the count only if every slot that read one agrees.
                if counts.iter().all(|&c| c == first) {
                    ObsKind::Count(first)
                } else {
                    ObsKind::Abstain
                }
            } else {
                ObsKind::Abstain // owned card, but no slot produced a usable count
            };
            RelicObservation { display, refinement, kind }
        })
        .collect()
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

    // Per-slot debug (eye NCC + OCR) to calibrate ownership + regions.
    let pre = wf_ocr::Preprocess { scale: 4, threshold: 140, light_text: true };
    for (i, slot) in regions.slots(image.width(), image.height()).iter().enumerate() {
        let ncc = eye_ncc(&image, &slot.eye);
        let raw = ocr
            .recognize(
                &image::imageops::crop_imm(&image, slot.name.x, slot.name.y, slot.name.w, slot.name.h)
                    .to_image(),
                pre,
                wf_ocr::PageMode::Line,
            )
            .unwrap_or_default()
            .replace('\n', " ");
        let (base, refinement) = wf_relic::parse_refinement(&raw);
        let matched = index.best_match(&base).map(|r| r.display.as_str());
        let badge = read_count_badge(&image, &slot.count, &ocr, pre);
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
    let regions = wf_relic::RewardRegions::default_calibration();
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
    let evals =
        wf_relic::evaluate_cached(&names, &index, &market, &cache, wf_relic::PriceOpts::default())
            .await;
    let mastery = load_mastery(config, &client).await;
    print_reward_table(&evals, &mastery);
    Ok(())
}

/// Map evaluated rewards to overlay rows (matched name, plat, best pick, mastery).
fn reward_rows(
    evals: &[wf_relic::RewardEval],
    mastery: &wf_relic::MasterySet,
) -> Vec<wf_overlay::RewardRow> {
    let bp = wf_relic::best_by_plat(evals);
    evals
        .iter()
        .enumerate()
        .map(|(i, e)| wf_overlay::RewardRow {
            name: e.matched_name.clone().unwrap_or_else(|| e.ocr.clone()),
            plat: e.plat,
            best_plat: Some(i) == bp,
            mastered: e
                .matched_name
                .as_deref()
                .is_some_and(|n| mastery.is_mastered(n)),
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
    let evals =
        wf_relic::evaluate_cached(&names, &index, &market, &cache, wf_relic::PriceOpts::default())
            .await;

    let mastery = load_mastery(config, &client).await;
    let font = wf_overlay::load_font()?;
    let canvas = wf_overlay::render_reward_panel(&reward_rows(&evals, &mastery), &font);
    let img = image::RgbaImage::from_raw(canvas.width, canvas.height, canvas.buf)
        .context("canvas -> image")?;
    let out = "reward.png";
    img.save(out).map_err(|e| anyhow::anyhow!("saving {out}: {e}"))?;
    println!("  {}x{} reward panel saved to {out}", img.width(), img.height());
    Ok(())
}

/// Print a ranked reward table: plat, best-plat marker, and mastery status.
fn print_reward_table(evals: &[wf_relic::RewardEval], mastery: &wf_relic::MasterySet) {
    let best_plat = wf_relic::best_by_plat(evals);
    println!("  {:<26} {:>6}  {:<9} match", "reward", "plat", "mastery");
    for (i, e) in evals.iter().enumerate() {
        let mark = if Some(i) == best_plat { " ⭐plat" } else { "" };
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
/// How long the owned-relic guide stays on the overlay after the last scan update.
const RELIC_DISPLAY: Duration = Duration::from_secs(30);
/// How long the overlay polls for the game window before falling back to the
/// compositor's default output (lets it be launched together with the game).
const WINDOW_WAIT: Duration = Duration::from_secs(30);
/// Item catalogue is refetched at most this often (new primes are rare).
const CATALOGUE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// Mastery data is refreshed at most this often (it changes slowly).
const MASTERY_TTL: Duration = Duration::from_secs(24 * 3600);

/// Load the player's mastered set (cached) if an account id is configured,
/// otherwise an empty set (mastery indicators simply off).
async fn load_mastery(config: &Config, client: &reqwest::Client) -> wf_relic::MasterySet {
    match &config.account_id {
        Some(id) => wf_relic::mastery::load_cached(client, id, MASTERY_TTL).await,
        None => wf_relic::MasterySet::default(),
    }
}

type RewardState = std::sync::Arc<std::sync::Mutex<Option<(std::time::Instant, Vec<wf_overlay::RewardRow>)>>>;
type RelicState = std::sync::Arc<std::sync::Mutex<Option<(std::time::Instant, Vec<wf_overlay::RelicRow>)>>>;
/// Deadline the owned-relic scan should keep running until, shared between
/// `relic_watch_loop` (which extends it from `EE.log`) and `relic_scan_loop`
/// (which reads it every iteration to decide whether to scan or idle).
type RelicDeadline = std::sync::Arc<std::sync::Mutex<Option<std::time::Instant>>>;

/// Show the live overlay as a `wlr-layer-shell` surface: world state normally,
/// automatically swapping to the relic reward result for a few seconds when a
/// fissure reward is detected in the log.
async fn run_overlay(config: Config) -> Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc, Mutex};

    let font = Arc::new(wf_overlay::load_font()?);
    let client = http_client();
    let platform = config.platform.clone();
    let refresh = Duration::from_secs(config.worldstate_refresh_secs.max(15));
    let reward: RewardState = Arc::new(Mutex::new(None));
    let relic: RelicState = Arc::new(Mutex::new(None));

    // Appearance/visibility knobs. `visible` is flipped at runtime by the control
    // socket (see `overlay_control`); `show_world` and `opacity` come from config.
    let visible = Arc::new(AtomicBool::new(true));
    let show_world = config.overlay.world_state;
    let opacity = config.overlay.opacity;

    // Build one overlay frame from the current state, honoring reward-only mode,
    // the visibility toggle, and opacity. A hidden or empty frame is a fully
    // transparent (click-through) canvas.
    // Panel priority when shown: reward screen (time-critical, ~20s) → owned-relic
    // guide (while/after a Relics-screen scan) → world state → blank.
    let make_frame = {
        let font = font.clone();
        let reward = reward.clone();
        let relic = relic.clone();
        move |ws: &worldstate::WorldState, shown: bool| -> wf_overlay::Canvas {
            let blank = || wf_overlay::Canvas::new(OVERLAY_W, OVERLAY_H);
            let mut c = if !shown {
                blank()
            } else if let Some(rows) = reward
                .lock()
                .unwrap()
                .as_ref()
                .filter(|(t, _)| t.elapsed() < REWARD_DISPLAY)
                .map(|(_, r)| r.clone())
            {
                wf_overlay::render_reward_panel(&rows, &font).embed(OVERLAY_W, OVERLAY_H)
            } else if let Some(rows) = relic
                .lock()
                .unwrap()
                .as_ref()
                .filter(|(t, _)| t.elapsed() < RELIC_DISPLAY)
                .map(|(_, r)| r.clone())
            {
                wf_overlay::render_relic_panel(&rows, &font).embed(OVERLAY_W, OVERLAY_H)
            } else if show_world {
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
        "  placement: {} (margin {}x{}); world panel: {}; opacity: {opacity}",
        config.overlay.anchor,
        config.overlay.margin_x,
        config.overlay.margin_y,
        if show_world { "on" } else { "reward-only" },
    );
    let ws = worldstate::fetch(&client, &platform)
        .await
        .context("initial worldstate fetch")?;
    let initial = make_frame(&ws, visible.load(Ordering::Relaxed));

    let (tx, rx) = mpsc::channel();

    // Control socket: `wf-lite toggle|show|hide` flips `visible` at runtime, so a
    // KDE global shortcut bound to those commands can hide the overlay on demand.
    spawn_control_listener(visible.clone());

    // Renderer: rebuild the frame each second (ETAs tick, reward panel expires,
    // visibility may have toggled) and push it to the layer surface.
    {
        let client = client.clone();
        let visible = visible.clone();
        tokio::spawn(async move {
            let mut cached = ws;
            let mut last_fetch = tokio::time::Instant::now();
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                if last_fetch.elapsed() >= refresh {
                    match worldstate::fetch(&client, &platform).await {
                        Ok(fresh) => cached = fresh,
                        Err(e) => tracing::warn!("worldstate refresh failed: {e:#}"),
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

    // Relic auto-detection: needs tesseract, an EE.log, and the item catalogue.
    match (wf_ocr::Ocr::new(), config.resolve_ee_log()) {
        (Ok(ocr), Ok(ee_log)) => match wf_relic::ItemIndex::load_cached(&client, CATALOGUE_TTL).await {
            Ok(index) => {
                let cache = Arc::new(wf_relic::price_cache());
                let mastery = Arc::new(load_mastery(&config, &client).await);
                // Relic drop tables for the owned-relic guide (best-effort).
                let relic_index = wf_relic::RelicIndex::load_cached(&client, CATALOGUE_TTL)
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
                        market.clone(),
                        cache.clone(),
                        mastery.clone(),
                        relic.clone(),
                    ));
                    Some(deadline)
                } else {
                    None
                };

                tokio::spawn(async move {
                    if let Err(e) = relic_watch_loop(
                        ee_log,
                        ocr,
                        Arc::new(index),
                        market,
                        cache,
                        mastery,
                        reward,
                        wf_relic::RewardRegions::default_calibration(),
                        relic_deadline,
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

/// Listen on the control socket for `toggle` / `show` / `hide` lines and update
/// the shared `visible` flag. A stale socket file from a previous run is removed
/// first. Runs for the life of the overlay.
fn spawn_control_listener(visible: std::sync::Arc<std::sync::atomic::AtomicBool>) {
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
    println!("  control:   {} (wf-lite toggle|show|hide)", path.display());
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
                other => tracing::warn!("unknown overlay control command {other:?}"),
            }
        }
    });
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

/// How long a relic-inventory-open event keeps [`relic_scan_loop`] scanning
/// (shared: `relic_watch_loop` extends the deadline, `relic_scan_loop` reads it).
const RELIC_SCAN_WINDOW: Duration = Duration::from_secs(180);

/// Watch the EE.log for a relic **crack** / reward-screen line, which opens a
/// poll window scanned until the 4-choice screen resolves (publishing the
/// ranked reward to `reward`). A **Relics inventory** open
/// (`RelicInventoryOpen`) instead extends `relic_deadline` — the actual owned-
/// relic scanning happens in the separate, tightly-looped [`relic_scan_loop`],
/// so a slow or expensive scan never throttles reward-screen detection (and
/// vice versa: reward-screen debounce logic never throttles the relic scan).
#[allow(clippy::too_many_arguments)]
async fn relic_watch_loop(
    ee_log: std::path::PathBuf,
    ocr: std::sync::Arc<wf_ocr::Ocr>,
    index: std::sync::Arc<wf_relic::ItemIndex>,
    market: MarketClient,
    cache: std::sync::Arc<wf_relic::PriceCache>,
    mastery: std::sync::Arc<wf_relic::MasterySet>,
    reward: RewardState,
    regions: wf_relic::RewardRegions,
    relic_deadline: Option<RelicDeadline>,
) -> Result<()> {
    use std::time::Instant;
    use wf_log::Event;

    // After a crack (or a reward-screen line), poll the screen for this long.
    // Covers the ~1min crack→screen gap, log-flush lag, and the player bringing
    // the screen up manually (Tab / progress).
    const POLL_WINDOW: Duration = Duration::from_secs(150);
    const POLL_INTERVAL: Duration = Duration::from_secs(2);
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
        let names = wf_relic::select_rewards(&slots, &index);
        if names.len() < 2 {
            continue; // screen not up yet (or already gone) — keep polling
        }

        let evals =
            wf_relic::evaluate_cached(&names, &index, &market, &cache, wf_relic::PriceOpts::default())
                .await;
        let rows = reward_rows(&evals, &mastery);
        if let Some(best) = wf_relic::best_by_plat(&evals) {
            tracing::info!("reward screen captured — best plat pick = {}", rows[best].name);
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
#[allow(clippy::too_many_arguments)]
async fn relic_scan_loop(
    relic_deadline: RelicDeadline,
    ocr: std::sync::Arc<wf_ocr::Ocr>,
    relic_index: std::sync::Arc<wf_relic::RelicIndex>,
    relic_regions: wf_relic::RelicGridRegions,
    market: MarketClient,
    cache: std::sync::Arc<wf_relic::PriceCache>,
    mastery: std::sync::Arc<wf_relic::MasterySet>,
    relic: RelicState,
) {
    use std::collections::HashMap;
    use std::time::Instant;

    // End a scan once no new relic has been seen for this long (the player
    // stopped scrolling / left the screen), capped at RELIC_SCAN_WINDOW.
    const RELIC_SCAN_IDLE: Duration = Duration::from_secs(12);
    // Between-scan gap while actively scanning — a scan itself already takes
    // well over this, so it mainly just yields rather than limiting throughput.
    const SCAN_COOLDOWN: Duration = Duration::from_millis(100);
    // How often to check for a new deadline while idle (no need to poll fast
    // when there's nothing to do).
    const IDLE_POLL: Duration = Duration::from_millis(400);

    // `relic_owned` is the cumulative, disk-persisted owned set, keyed per
    // (code, refinement) (see ADR-0005) — it survives restarts and Relics-screen
    // visits, so the mastery planner (`wf-lite mastery-plan`) has data even
    // outside a live scan. Within the *current* continuous scan (cleared each
    // fresh open), `tally` votes across frames and only confirms a count once
    // enough frames agree, and `session_applied` records which confirmed value
    // we've already written this session so a stable count refreshes its
    // last-seen stamp once, not on every frame.
    let owned_path = wf_cache::cache_dir().ok().map(|d| d.join(wf_relic::OWNED_RELICS_FILE));
    let mut relic_owned: wf_relic::OwnedRelics =
        match wf_cache::load_blob::<wf_relic::OwnedRelics>(wf_relic::OWNED_RELICS_FILE) {
            Some(s) => s.value,
            None => {
                // Absent, or a legacy/foreign format we can no longer trust — back
                // up any existing file and start clean (ADR-0005).
                if let Some(p) = &owned_path {
                    if p.exists() {
                        let bak = p.with_extension("json.bak");
                        if std::fs::rename(p, &bak).is_ok() {
                            tracing::info!("backed up unrecognised {} to {}", p.display(), bak.display());
                        }
                    }
                }
                wf_relic::OwnedRelics::default()
            }
        };
    let mut tally: wf_ocr::Tally<(String, wf_relic::Refinement)> = wf_ocr::Tally::new();
    let mut session_applied: HashMap<(String, wf_relic::Refinement), u32> = HashMap::new();
    let mut was_active = false;
    // Bogus initial value (not a real deadline the watcher could ever set) so the
    // very first observed deadline always registers as a change below.
    let mut last_seen_deadline: Option<Instant> = Some(Instant::now() - RELIC_SCAN_IDLE * 2);
    let mut last_new = Instant::now() - RELIC_SCAN_IDLE * 2;

    loop {
        let deadline = *relic_deadline.lock().unwrap();
        // relic_watch_loop just (re)armed the window from a fresh EE.log event —
        // treat that as activity so the idle check below doesn't see a `last_new`
        // that's stale from long before the Relics screen was ever opened (this
        // task runs for the overlay's whole lifetime, so without this, "idle"
        // would already be true the instant a deadline first appears).
        if deadline != last_seen_deadline {
            last_seen_deadline = deadline;
            if deadline.is_some() {
                last_new = Instant::now();
            }
        }
        let active = deadline.is_some_and(|t| Instant::now() < t) && last_new.elapsed() <= RELIC_SCAN_IDLE;

        if !active {
            was_active = false;
            tokio::time::sleep(IDLE_POLL).await;
            continue;
        }
        if !was_active {
            was_active = true;
            tally = wf_ocr::Tally::new();
            session_applied.clear();
            tracing::info!(
                "relics screen opened — scanning as you scroll ({} relics known from before)",
                relic_owned.len()
            );
        }

        let (ocr2, regions2, ridx2) = (ocr.clone(), relic_regions.clone(), relic_index.clone());
        let scanned = tokio::task::spawn_blocking(move || {
            wf_capture::capture_warframe(None)
                .map(|cap| scan_relic_grid(&cap.image, &ocr2, &regions2, &ridx2))
        })
        .await;
        if let Ok(Ok(found)) = scanned {
            let mut changed = false;
            for obs in found {
                let key = (obs.display.clone(), obs.refinement);
                match obs.kind {
                    ObsKind::Count(n) => tally.record(key.clone(), n),
                    ObsKind::Unowned => tally.record(key.clone(), 0), // 0 = confirmed unowned
                    ObsKind::Abstain => continue,
                }
                let Some(confirmed) = tally.confirmed(&key, RELIC_AGREEMENT) else {
                    continue;
                };
                // Write each confirmed value once per session; a stable count then
                // just refreshes nothing until it actually changes.
                if session_applied.get(&key) == Some(&confirmed) {
                    continue;
                }
                session_applied.insert(key.clone(), confirmed);
                apply_confirmed_count(&mut relic_owned, &key, confirmed);
                changed = true;
            }
            if changed {
                last_new = Instant::now();
                let _ = wf_cache::save_blob(wf_relic::OWNED_RELICS_FILE, &relic_owned);
                let intact = wf_relic::intact_counts(&relic_owned);
                let rows =
                    build_relic_rows(&intact, &relic_index, &mastery, &cache, &market, RELIC_ROWS)
                        .await;
                tracing::info!(
                    "relic scan: {} relics owned; showing {}",
                    relic_owned.len(),
                    rows.iter()
                        .map(|r| format!("{} ({})", r.name, r.top_reward))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                *relic.lock().unwrap() = Some((Instant::now(), rows));
            }
        }
        tokio::time::sleep(SCAN_COOLDOWN).await;
    }
}

/// Apply a confirmed `(code, refinement)` count to the owned set: a value of 0
/// (a confirmed "unowned" eye reading) removes that entry — positive proof the
/// player owns none — while any positive value replaces the count and refreshes
/// its last-seen stamp (see ADR-0005).
fn apply_confirmed_count(
    owned: &mut wf_relic::OwnedRelics,
    key: &(String, wf_relic::Refinement),
    value: u32,
) {
    let (code, refinement) = key;
    if value == 0 {
        if let Some(by_ref) = owned.get_mut(code) {
            by_ref.remove(refinement);
            if by_ref.is_empty() {
                owned.remove(code);
            }
        }
    } else {
        owned.entry(code.clone()).or_default().insert(
            *refinement,
            wf_cache::Stamped { value, fetched_at: wf_cache::now_unix() },
        );
    }
}

/// How many relic rows fit the overlay panel height.
const RELIC_ROWS: usize = 12;

/// Build the ranked owned-relic guide rows: for each owned relic that can still
/// drop an unmastered prime, its unmastered count + a market price, top-N by value.
async fn build_relic_rows(
    owned: &std::collections::HashMap<String, u32>,
    index: &wf_relic::RelicIndex,
    mastery: &wf_relic::MasterySet,
    cache: &wf_relic::PriceCache,
    market: &MarketClient,
    max_rows: usize,
) -> Vec<wf_overlay::RelicRow> {
    let mut picks: Vec<wf_relic::RelicPick> = Vec::new();
    for r in index.all() {
        let Some(&count) = owned.get(&r.display) else {
            continue;
        };
        let unmastered = r.unmastered(mastery);
        if unmastered.is_empty() {
            continue;
        }
        let plat =
            wf_relic::cached_plat(cache, market, &r.slug(), wf_relic::PriceOpts::default()).await;
        picks.push(wf_relic::RelicPick { display: r.display.clone(), count, unmastered, plat });
    }
    cache.save();
    wf_relic::rank_relics(&mut picks);
    picks.truncate(max_rows);
    relic_rows(&picks)
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
