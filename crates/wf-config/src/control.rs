//! The overlay's Unix control socket protocol: bare `toggle`/`show`/`hide`/
//! `copy`/`demo-on`/`demo-off` commands (unstructured single words, parsed
//! directly by the overlay's own listener) plus a structured
//! `apply-settings ...` command carrying the anchor/margin/opacity/fissures
//! fields a settings UI can push to a running overlay without a restart.
//!
//! Live-apply is possible because reconfiguring an already-committed
//! `zwlr_layer_shell_v1` surface in place — `set_anchor`/`set_margin`/
//! `set_size` again, followed by another `commit()` — is protocol-correct
//! and needs no surface teardown or Wayland reconnection.
//!
//! Shared between the overlay process (the listener) and anyone driving it:
//! the CLI's `toggle`/`show`/`hide`/`copy` subcommands, and the settings UIs
//! pushing `apply-settings` on commit and `demo-on`/`demo-off` around focus.

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::OverlayConfig;

/// Command name for the apply-settings message (see
/// [`format_apply_settings`]/[`parse_apply_settings`]).
pub const APPLY_SETTINGS_CMD: &str = "apply-settings";

/// Swap a running overlay to curated, hardcoded content — a fixed reward
/// panel and a fixed fissures panel, cycled — so a settings UI can preview
/// placement/opacity against every visually-distinct panel state without
/// waiting on a live reward drop or live fissures. `demo-off` resumes
/// showing whatever the overlay's background eval/poll loop has been
/// computing the whole time (that loop keeps running underneath demo mode).
/// Bare words, no payload, matching `toggle`/`show`/`hide`/`copy`'s style.
pub const DEMO_ON_CMD: &str = "demo-on";
pub const DEMO_OFF_CMD: &str = "demo-off";

/// Filesystem path of the overlay's control socket. Placed in the per-user
/// runtime dir when available, falling back to the temp dir.
pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join("warframe-lite-overlay.sock")
}

/// The visual/placement fields a running overlay can be reconfigured with at
/// runtime — the subset of [`OverlayConfig`] live-apply covers. The rest
/// (`reward_pitch`, `reward_center_x`) stay restart-only.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveOverlaySettings {
    pub anchor: String,
    pub margin_x: f32,
    pub margin_y: f32,
    pub opacity: f32,
    pub fissures: bool,
    pub fissure_filter: wf_data::worldstate::FissureFilter,
}

impl From<&OverlayConfig> for LiveOverlaySettings {
    fn from(o: &OverlayConfig) -> Self {
        Self {
            anchor: o.anchor.clone(),
            margin_x: o.margin_x,
            margin_y: o.margin_y,
            opacity: o.opacity,
            fissures: o.fissures,
            fissure_filter: o.fissure_filter.clone(),
        }
    }
}

/// Encode a `HashSet<String>` as a comma-joined, sorted list for a single
/// `apply-settings` token — spaces become underscores (no known mission-type
/// name uses one) so the list survives the wire format's whitespace-based
/// tokenizing untouched.
fn encode_list(items: &std::collections::HashSet<String>) -> String {
    let mut v: Vec<String> = items.iter().map(|s| s.replace(' ', "_")).collect();
    v.sort();
    v.join(",")
}

/// Inverse of [`encode_list`].
fn decode_list(s: &str) -> std::collections::HashSet<String> {
    if s.is_empty() {
        return std::collections::HashSet::new();
    }
    s.split(',').map(|t| t.replace('_', " ")).collect()
}

/// Encode an `apply-settings` control-socket message. Wire format is one
/// line of space-separated `key=value` tokens, matching the existing
/// single-word `toggle`/`show`/`hide`/`copy` commands' plain-text style
/// rather than introducing a new dependency (JSON etc) for five scalars.
/// The fissure filter's three sets ride the same scheme via [`encode_list`].
pub fn format_apply_settings(s: &LiveOverlaySettings) -> String {
    format!(
        "{APPLY_SETTINGS_CMD} anchor={} margin_x={} margin_y={} opacity={} fissures={} \
         fissure_tiers={} fissure_types={} fissure_kinds={}",
        s.anchor,
        s.margin_x,
        s.margin_y,
        s.opacity,
        s.fissures,
        encode_list(&s.fissure_filter.tiers),
        encode_list(&s.fissure_filter.mission_types),
        encode_list(&s.fissure_filter.kinds),
    )
}

/// Decode an `apply-settings` message built by [`format_apply_settings`].
/// `None` on anything malformed or missing a field — the listener logs and
/// ignores rather than acting on a partial update.
pub fn parse_apply_settings(line: &str) -> Option<LiveOverlaySettings> {
    let rest = line.trim().strip_prefix(APPLY_SETTINGS_CMD)?.trim();
    let (mut anchor, mut margin_x, mut margin_y, mut opacity, mut fissures) =
        (None, None, None, None, None);
    let (mut fissure_tiers, mut fissure_types, mut fissure_kinds) = (None, None, None);
    for token in rest.split_whitespace() {
        let (key, value) = token.split_once('=')?;
        match key {
            "anchor" => anchor = Some(value.to_string()),
            "margin_x" => margin_x = value.parse().ok(),
            "margin_y" => margin_y = value.parse().ok(),
            "opacity" => opacity = value.parse().ok(),
            "fissures" => fissures = value.parse().ok(),
            "fissure_tiers" => fissure_tiers = Some(decode_list(value)),
            "fissure_types" => fissure_types = Some(decode_list(value)),
            "fissure_kinds" => fissure_kinds = Some(decode_list(value)),
            _ => {}
        }
    }
    Some(LiveOverlaySettings {
        anchor: anchor?,
        margin_x: margin_x?,
        margin_y: margin_y?,
        opacity: opacity?,
        fissures: fissures?,
        fissure_filter: wf_data::worldstate::FissureFilter {
            tiers: fissure_tiers?,
            mission_types: fissure_types?,
            kinds: fissure_kinds?,
        },
    })
}

/// Send a single command (`toggle`/`show`/`hide`/`copy`, or a
/// [`format_apply_settings`] payload) to a running overlay's control socket.
/// A missing/refused socket is reported as "no overlay is running" rather
/// than a raw IO error, since that's the common case when settings are
/// edited while the overlay isn't up.
pub fn send_command(payload: &str) -> Result<()> {
    use std::io::Write;

    let path = socket_path();
    match std::os::unix::net::UnixStream::connect(&path) {
        Ok(mut stream) => stream
            .write_all(payload.as_bytes())
            .with_context(|| format!("sending to overlay at {}", path.display())),
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                || e.kind() == std::io::ErrorKind::ConnectionRefused =>
        {
            anyhow::bail!("no overlay is running (control socket {} absent)", path.display())
        }
        Err(e) => Err(e).with_context(|| format!("connecting to overlay at {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_settings_roundtrips() {
        let s = LiveOverlaySettings {
            anchor: "bottom-left".to_string(),
            margin_x: 0.12,
            margin_y: 0.34,
            opacity: 0.75,
            fissures: false,
            fissure_filter: wf_data::worldstate::FissureFilter {
                tiers: ["Axi".to_string(), "Neo".to_string()].into_iter().collect(),
                mission_types: ["Capture".to_string(), "Mobile Defense".to_string()]
                    .into_iter()
                    .collect(),
                kinds: ["SteelPath".to_string()].into_iter().collect(),
            },
        };
        let line = format_apply_settings(&s);
        assert_eq!(parse_apply_settings(&line), Some(s));
    }

    #[test]
    fn apply_settings_roundtrips_with_empty_fissure_filter() {
        let s = LiveOverlaySettings {
            anchor: "top-right".to_string(),
            margin_x: 0.24,
            margin_y: 0.24,
            opacity: 1.0,
            fissures: true,
            fissure_filter: wf_data::worldstate::FissureFilter::default(),
        };
        let line = format_apply_settings(&s);
        assert_eq!(parse_apply_settings(&line), Some(s));
    }

    #[test]
    fn parse_rejects_missing_or_foreign_commands() {
        assert_eq!(parse_apply_settings("apply-settings anchor=top-right"), None);
        assert_eq!(parse_apply_settings("toggle"), None);
    }

    #[test]
    fn live_settings_from_overlay_config() {
        let cfg = OverlayConfig { opacity: 0.5, ..OverlayConfig::default() };
        let live = LiveOverlaySettings::from(&cfg);
        assert_eq!(live.anchor, cfg.anchor);
        assert_eq!(live.opacity, 0.5);
    }
}
