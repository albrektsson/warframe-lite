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
        Some("ocr") => return ocr_test(),
        Some("ocr-file") => return ocr_file(),
        Some("relic-file") => return relic_file(&config).await,
        Some("mastery") => return mastery_cmd(&config).await,
        Some("set-account") => return set_account_cmd(&config_path),
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
    use std::sync::{mpsc, Arc, Mutex};

    let font = Arc::new(wf_overlay::load_font()?);
    let client = http_client();
    let platform = config.platform.clone();
    let refresh = Duration::from_secs(config.worldstate_refresh_secs.max(15));
    let reward: RewardState = Arc::new(Mutex::new(None));

    println!("\n== Live overlay (Ctrl-C to stop) ==");
    let ws = worldstate::fetch(&client, &platform)
        .await
        .context("initial worldstate fetch")?;
    let initial = wf_overlay::render_panel(&ws, &font).embed(OVERLAY_W, OVERLAY_H);

    let (tx, rx) = mpsc::channel();

    // Renderer: world panel each second (ETAs tick), reward panel while active.
    {
        let font = font.clone();
        let reward = reward.clone();
        let client = client.clone();
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
                let panel = {
                    let r = reward.lock().unwrap();
                    match r.as_ref() {
                        Some((t, rows)) if t.elapsed() < REWARD_DISPLAY => {
                            wf_overlay::render_reward_panel(rows, &font)
                        }
                        _ => wf_overlay::render_panel(&cached, &font),
                    }
                };
                if tx.send(panel.embed(OVERLAY_W, OVERLAY_H)).is_err() {
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

    // Place the overlay on the monitor Warframe is on (centre of its window).
    let target = wf_capture::warframe_geometry()
        .ok()
        .map(|(x, y, w, h)| (x + w as i32 / 2, y + h as i32 / 2));
    match target {
        Some(p) => println!("  overlay target: game monitor (window centre {p:?})"),
        None => println!("  overlay target: compositor default (game window not found)"),
    }

    // The Wayland event loop is blocking and uses non-Send types; run it on a
    // dedicated blocking thread.
    tokio::task::spawn_blocking(move || {
        wf_overlay::layer::run(initial, rx, wf_overlay::layer::Placement::default(), target)
    })
    .await
    .context("overlay thread panicked")?
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
