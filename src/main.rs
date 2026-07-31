//! warframe-lite — Phase 0 smoke test.
//!
//! Verifies the end-to-end data plumbing:
//!   * config load + `EE.log` auto-detection,
//!   * live world-state (fissures / Void Trader / cycles),
//!   * a warframe.market price lookup.
//!
//! Usage:
//!   wf-lite                 # world-state + EE.log detection + price lookup
//!   wf-lite <market_slug>   # also print a price summary, e.g. `mirage_prime_set`
//!   wf-lite logstats        # parse whole EE.log history, report parse/event stats
//!   wf-lite logwatch        # follow EE.log live and print recognized events
//!   wf-lite capture [path]  # capture the Warframe (Xwayland) window to a PNG
//!   wf-lite overlay-png [p] # render the live world-state panel to a PNG (offscreen)
//!   wf-lite overlay         # show the live world-state overlay (wlr-layer-shell)
//!   wf-lite ocr [x y w h]   # OCR the Warframe window (or a region) — pipeline test
//!   wf-lite relic [names…]  # evaluate reward names → matched item, plat, ducats
//!   wf-lite relic-scan      # capture the reward screen, OCR 4 names, rank them

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
        Some("relic") => return relic_eval(&config).await,
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
    let skip = [
        "logstats", "logwatch", "capture", "overlay-png", "overlay", "ocr", "ocr-file", "relic",
        "relic-scan", "relic-file", "reward-png", "mastery", "set-account",
    ];
    if let Some(slug) = std::env::args().nth(1).filter(|a| !skip.contains(&a.as_str())) {
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
        println!("\n(tip: pass a warframe.market slug to test pricing, e.g. `wf-lite mirage_prime_set`)");
    }

    Ok(())
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
    let cache = price_cache();

    let evals =
        wf_relic::evaluate_cached(&names, &index, &market, &cache, wf_relic::PriceOpts::default())
            .await;
    let mastery = load_mastery(config, &client).await;
    print_reward_table(&evals, &mastery);
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
    let cache = price_cache();
    let evals =
        wf_relic::evaluate_cached(&names, &index, &market, &cache, wf_relic::PriceOpts::default())
            .await;
    let mastery = load_mastery(config, &client).await;
    print_reward_table(&evals, &mastery);
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
    let cache = price_cache();
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
    let cache = price_cache();
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
/// How long the overlay polls for the game window before falling back to the
/// compositor's default output (lets it be launched together with the game).
const WINDOW_WAIT: Duration = Duration::from_secs(30);
/// Item catalogue is refetched at most this often (new primes are rare).
const CATALOGUE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// Mastery data is refreshed at most this often (it changes slowly).
const MASTERY_TTL: Duration = Duration::from_secs(24 * 3600);

/// Load the shared on-disk price cache.
fn price_cache() -> wf_relic::PriceCache {
    wf_relic::PriceCache::load("prices.json")
}

/// Load the player's mastered set (cached) if an account id is configured,
/// otherwise an empty set (mastery indicators simply off).
async fn load_mastery(config: &Config, client: &reqwest::Client) -> wf_relic::MasterySet {
    match &config.account_id {
        Some(id) => wf_relic::mastery::load_cached(client, id, MASTERY_TTL).await,
        None => wf_relic::MasterySet::default(),
    }
}

type RewardState = std::sync::Arc<std::sync::Mutex<Option<(std::time::Instant, Vec<wf_overlay::RewardRow>)>>>;

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

    // Appearance/visibility knobs. `visible` is flipped at runtime by the control
    // socket (see `overlay_control`); `show_world` and `opacity` come from config.
    let visible = Arc::new(AtomicBool::new(true));
    let show_world = config.overlay.world_state;
    let opacity = config.overlay.opacity;

    // Build one overlay frame from the current state, honoring reward-only mode,
    // the visibility toggle, and opacity. A hidden or empty frame is a fully
    // transparent (click-through) canvas.
    let make_frame = {
        let font = font.clone();
        let reward = reward.clone();
        move |ws: &worldstate::WorldState, shown: bool| -> wf_overlay::Canvas {
            let mut c = if !shown {
                wf_overlay::Canvas::new(OVERLAY_W, OVERLAY_H)
            } else {
                let r = reward.lock().unwrap();
                match r.as_ref() {
                    Some((t, rows)) if t.elapsed() < REWARD_DISPLAY => {
                        wf_overlay::render_reward_panel(rows, &font).embed(OVERLAY_W, OVERLAY_H)
                    }
                    _ if show_world => {
                        wf_overlay::render_panel(ws, &font).embed(OVERLAY_W, OVERLAY_H)
                    }
                    _ => wf_overlay::Canvas::new(OVERLAY_W, OVERLAY_H),
                }
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
                let cache = Arc::new(price_cache());
                let mastery = Arc::new(load_mastery(&config, &client).await);
                println!(
                    "  relic auto-detect: ON ({} items; {} cached prices; {} mastered; watching {})",
                    index.len(),
                    cache.len(),
                    mastery.len(),
                    ee_log.display()
                );
                let market = MarketClient::new(client.clone(), config.market_platform.clone());
                let reward = reward.clone();
                tokio::spawn(async move {
                    if let Err(e) = relic_watch_loop(
                        ee_log,
                        Arc::new(ocr),
                        Arc::new(index),
                        market,
                        cache,
                        mastery,
                        reward,
                        wf_relic::RewardRegions::default_calibration(),
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

/// Watch the EE.log for a relic crack (`DVRCAftermath`); on each, scan the
/// reward screen with retries until at least two names resolve (the OCR guard
/// confirms the screen is up), then publish the ranked result to `reward`.
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
                _ => {}
            }
        }

        // Are we inside an active polling window?
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
