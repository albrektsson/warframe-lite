//! Configuration and environment discovery for warframe-lite.
//!
//! Handles loading/saving the TOML config file and auto-detecting the Warframe
//! `EE.log` inside the Steam Proton prefix (Steam app id `230410`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub mod control;

/// Warframe's Steam app id. The Proton compatibility prefix lives under
/// `steamapps/compatdata/<APPID>/`.
pub const WARFRAME_APPID: &str = "230410";

/// Relative path of `EE.log` inside a Proton prefix (`pfx`).
const EE_LOG_REL: &str =
    "pfx/drive_c/users/steamuser/AppData/Local/Warframe/EE.log";

/// Top-level configuration, persisted as TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Path to Warframe's `EE.log`. `None` means "auto-detect at runtime".
    pub ee_log_path: Option<PathBuf>,
    /// Which platform to query live Fissures for (pc, ps4, xb1, swi, mob).
    pub platform: String,
    /// Warframe.market platform (pc, ps4, xbox, switch).
    pub market_platform: String,
    /// How often to refresh the live Fissure list, in seconds.
    pub fissure_refresh_secs: u64,
    /// Warframe account id (24-hex) for mastery lookup via the public profile
    /// API. Find it at <https://www.warframe.com/api/user-data> (`user_id`).
    /// `None` disables mastery indicators.
    pub account_id: Option<String>,
    /// Overlay placement and appearance.
    pub overlay: OverlayConfig,
    /// `wf-browse` window appearance (font size).
    pub ui: UiConfig,
}

/// Where the overlay sits and how visible it is. Warframe uses every screen
/// corner for HUD/menu elements, so position, opacity, and whether the
/// persistent Fissure panel shows at all are all configurable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlayConfig {
    /// Which corner/edge to anchor to: `top-left`, `top-right`, `bottom-left`,
    /// `bottom-right`, `top`, `bottom`, `left`, `right`, or `center`.
    pub anchor: String,
    /// Horizontal inset from the anchored edge(s), as a fraction of the
    /// maximum meaningful inset on that axis (`0.0` flush against the edge,
    /// `1.0` centered) — resolution-independent, unlike a raw pixel count,
    /// so the same value places the panel the same relative distance from
    /// the edge on any monitor. See `wf_overlay::layer::edge_margins` for
    /// the real-pixel conversion.
    pub margin_x: f32,
    /// Vertical inset from the anchored edge(s), as a fraction — see
    /// `margin_x`.
    pub margin_y: f32,
    /// Show the persistent live-Fissure panel. When `false`, the overlay is
    /// invisible until a relic reward screen is detected (reward-only mode).
    pub fissures: bool,
    /// Which fissures the panel shows when `fissures` is on — e.g. only Axi
    /// Capture. An empty/default filter shows every active fissure.
    pub fissure_filter: wf_data::worldstate::FissureFilter,
    /// Panel opacity, `0.0` (invisible) to `1.0` (as-drawn). Scales the whole
    /// panel's alpha so it obscures less of the game behind it.
    pub opacity: f32,
    /// Override for the reward screen's candidate-centre spacing (distance
    /// between adjacent card centres, in reference-resolution pixels; see
    /// `wf_relic::RewardRegions::pitch`). `None` uses the built-in
    /// calibration. Candidate centres are spaced at half of this value.
    pub reward_pitch: Option<u32>,
    /// Override for the reward screen's horizontal centre (in
    /// reference-resolution pixels; see `wf_relic::RewardRegions::center_x`).
    /// `None` uses the built-in calibration. Retune either of these two
    /// fields when OCR on a given display consistently misses the reward
    /// names — no rebuild required.
    pub reward_center_x: Option<u32>,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            anchor: "top-right".to_string(),
            margin_x: 0.05,
            margin_y: 0.05,
            fissures: true,
            fissure_filter: wf_data::worldstate::FissureFilter::default(),
            opacity: 1.0,
            reward_pitch: None,
            reward_center_x: None,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ee_log_path: None,
            platform: "pc".to_string(),
            market_platform: "pc".to_string(),
            fissure_refresh_secs: 60,
            account_id: None,
            overlay: OverlayConfig::default(),
            ui: UiConfig::default(),
        }
    }
}

/// `wf-browse` window appearance, independent of [`OverlayConfig`] (the
/// in-game overlay renders its own fixed text, unaffected by this).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Multiplier applied to every egui text style's font size. `1.0` is
    /// egui's own default sizing.
    pub font_scale: f32,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { font_scale: 1.0 }
    }
}

impl Config {
    /// Standard config file location, e.g. `~/.config/warframe-lite/config.toml`.
    pub fn default_path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "warframe-lite")
            .context("could not determine a home/config directory")?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Load config from `path`, returning defaults if the file does not exist.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!("no config at {}, using defaults", path.display());
                Ok(Self::default())
            }
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Persist config to `path`, creating parent directories as needed.
    ///
    /// Writes to a sibling temp file and `rename`s it over `path` rather than
    /// writing `path` in place: a plain in-place write is visible to
    /// concurrent readers mid-write (e.g. `wf-tray` spawning a fresh
    /// `wf-lite overlay` right as a settings UI saves a placement change) —
    /// `read_to_string` can catch it truncated or empty, and an empty file
    /// parses as valid, fully-defaulted TOML rather than erroring, silently
    /// resetting whatever reads it to `Config::default()`. `rename` on the
    /// same filesystem is atomic, so a concurrent reader only ever sees the
    /// old complete file or the new complete file, never a partial one.
    pub fn save(&self, path: &Path) -> Result<()> {
        let parent = path.parent().context("config path has no parent directory")?;
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        let text = toml::to_string_pretty(self).context("serializing config")?;
        let file_name = path.file_name().context("config path has no file name")?.to_string_lossy();
        let tmp = parent.join(format!(".{file_name}.tmp-{}", std::process::id()));
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("renaming {} to {}", tmp.display(), path.display()))?;
        Ok(())
    }

    /// Resolve the `EE.log` path: use the configured value if set and present,
    /// otherwise fall back to auto-detection.
    pub fn resolve_ee_log(&self) -> Result<PathBuf> {
        if let Some(p) = &self.ee_log_path {
            if p.exists() {
                return Ok(p.clone());
            }
            tracing::warn!(
                "configured ee_log_path {} does not exist, auto-detecting",
                p.display()
            );
        }
        detect_ee_log().context(
            "could not auto-detect Warframe EE.log; set `ee_log_path` in the config file",
        )
    }
}

/// Attempt to locate `EE.log` for the Steam Proton install of Warframe.
///
/// Scans the common Steam roots and every library listed in
/// `libraryfolders.vdf`, checking each for
/// `steamapps/compatdata/230410/<EE_LOG_REL>`.
pub fn detect_ee_log() -> Result<PathBuf> {
    for library in steam_libraries() {
        let candidate = library
            .join("steamapps")
            .join("compatdata")
            .join(WARFRAME_APPID)
            .join(EE_LOG_REL);
        if candidate.is_file() {
            tracing::info!("detected EE.log at {}", candidate.display());
            return Ok(candidate);
        }
    }
    anyhow::bail!("EE.log not found in any known Steam library")
}

/// Collect candidate Steam library roots: the well-known install locations plus
/// any extra libraries declared in `libraryfolders.vdf`.
fn steam_libraries() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());

    if let Some(home) = &home {
        for rel in [
            ".local/share/Steam",
            ".steam/steam",
            ".steam/root",
            ".var/app/com.valvesoftware.Steam/data/Steam", // Flatpak Steam
        ] {
            roots.push(home.join(rel));
        }
    }

    // Expand with libraries declared in libraryfolders.vdf, found in any root.
    let mut extra: Vec<PathBuf> = Vec::new();
    for root in &roots {
        let vdf = root.join("steamapps/libraryfolders.vdf");
        if let Ok(text) = std::fs::read_to_string(&vdf) {
            extra.extend(parse_library_paths(&text));
        }
    }
    roots.extend(extra);

    // De-duplicate by canonical path where possible, keeping only existing dirs.
    let mut seen: Vec<PathBuf> = Vec::new();
    roots
        .into_iter()
        .filter_map(|p| std::fs::canonicalize(&p).ok().or(Some(p)))
        .filter(|p| p.is_dir())
        .filter(|p| {
            if seen.contains(p) {
                false
            } else {
                seen.push(p.clone());
                true
            }
        })
        .collect()
}

/// Extract `"path"  "<value>"` entries from a `libraryfolders.vdf` blob.
///
/// The VDF format is simple enough that a full parser is unnecessary here: each
/// library entry contains a line like `    "path"   "/mnt/games/SteamLibrary"`.
fn parse_library_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("\"path\"")?;
            // Remaining is like: `   "/some/path"` — take the quoted value.
            let start = rest.find('"')? + 1;
            let end = rest[start..].find('"')? + start;
            Some(PathBuf::from(rest[start..end].replace("\\\\", "/")))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_library_paths() {
        let vdf = r#"
"libraryfolders"
{
    "0"
    {
        "path"		"/home/user/.local/share/Steam"
        "label"		""
    }
    "1"
    {
        "path"		"/mnt/games/SteamLibrary"
    }
}
"#;
        let paths = parse_library_paths(vdf);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/user/.local/share/Steam"),
                PathBuf::from("/mnt/games/SteamLibrary"),
            ]
        );
    }

    #[test]
    fn default_config_roundtrips() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.platform, cfg.platform);
        assert_eq!(back.fissure_refresh_secs, cfg.fissure_refresh_secs);
    }

    #[test]
    fn save_leaves_no_temp_file_behind_and_round_trips() {
        let dir = std::env::temp_dir().join(format!("wf-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let mut cfg = Config::default();
        cfg.overlay.margin_x = 0.42;
        cfg.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.overlay.margin_x, 0.42);

        // save() must land exactly one file (the final rename target), not a
        // leftover `.config.toml.tmp-<pid>` alongside it — a leftover would
        // mean the rename step never ran and a plain in-place write happened
        // instead, reintroducing the torn-write race this atomic save exists
        // to close.
        let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("config.toml")]);

        std::fs::remove_dir_all(&dir).ok();
    }
}
