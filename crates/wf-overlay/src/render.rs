//! Renders the live-Fissure info panel to a [`Canvas`].

use anyhow::Result;
use fontdue::Font;
use wf_data::worldstate::WorldState;

use crate::canvas::{Canvas, Color};

/// Fixed panel width in pixels.
pub const WIDTH: u32 = 460;

const PAD: i32 = 16;
const FONT_PX: f32 = 16.0;
const TITLE_PX: f32 = 13.0;
const LINE_H: i32 = 22;
const MAX_FISSURES: usize = 7;

// Palette.
const BG: Color = Color::rgba(18, 20, 26, 235);
const TITLE: Color = Color::rgb(120, 200, 255);
const TEXT: Color = Color::rgb(220, 224, 230);
const DIM: Color = Color::rgb(150, 156, 166);

/// Candidate monospace fonts, first existing wins.
const FONT_CANDIDATES: &[&str] = &[
    "/usr/share/fonts/liberation-mono-fonts/LiberationMono-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoSansMono-Regular.ttf",
    "/usr/share/fonts/adwaita-mono-fonts/AdwaitaMono-Regular.ttf",
    "/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
];

/// Load a monospace font from the first available candidate path.
pub fn load_font() -> Result<Font> {
    for path in FONT_CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(font) = Font::from_bytes(bytes, fontdue::FontSettings::default()) {
                tracing::info!("overlay font: {path}");
                return Ok(font);
            }
        }
    }
    anyhow::bail!(
        "no monospace font found in {:?}; install one or set a path",
        FONT_CANDIDATES
    )
}

/// Colour for a relic tier badge.
fn tier_color(tier: &str) -> Color {
    match tier {
        "Lith" => Color::rgb(176, 176, 176),
        "Meso" => Color::rgb(126, 200, 227),
        "Neo" => Color::rgb(240, 198, 120),
        "Axi" => Color::rgb(210, 150, 255),
        "Requiem" => Color::rgb(255, 120, 120),
        _ => TEXT,
    }
}

/// Render the panel for `ws` and return the finished canvas.
pub fn render_panel(ws: &WorldState, font: &Font) -> Canvas {
    let mut fissures: Vec<_> = ws.fissures.iter().filter(|f| f.active()).collect();
    // Prioritise normal fissures (best for relic running) over Steel Path, then
    // Void Storms; within a group keep the API order.
    fissures.sort_by_key(|f| (f.is_hard, f.is_storm));
    let shown = fissures.len().min(MAX_FISSURES);

    // Compute height from the rows we will draw.
    let mut rows = 1; // title
    rows += shown as i32; // fissures
    rows += 1; // "+N more" or spacing
    let height = (PAD * 2 + rows * LINE_H) as u32;

    let mut canvas = Canvas::new(WIDTH, height);
    canvas.fill_round_rect(0, 0, WIDTH, height, 12, BG);

    let mut y = PAD + 14; // baseline of first line

    // Title.
    canvas.draw_text(font, "WARFRAME — LIVE", PAD as f32, y as f32, TITLE_PX, TITLE);
    y += LINE_H;

    // Fissures, laid out in fixed monospace columns so the ETA never overlaps
    // the label. Column plan (in chars): [badge 8][label ...][eta 11, right].
    let charw = font.metrics('0', FONT_PX).advance_width;
    const BADGE_COLS: usize = 8;
    const ETA_COLS: usize = 10;
    let total_cols = ((WIDTH as f32 - 2.0 * PAD as f32) / charw).floor() as usize;
    let label_cols = total_cols.saturating_sub(BADGE_COLS + ETA_COLS);
    let label_x = PAD as f32 + BADGE_COLS as f32 * charw;

    for f in fissures.iter().take(MAX_FISSURES) {
        // Tier badge.
        canvas.draw_text(font, &f.tier, PAD as f32, y as f32, FONT_PX, tier_color(&f.tier));
        // Mission · node, truncated to its column.
        let label = truncate(&format!("{} · {}", f.mission_type, f.node), label_cols);
        canvas.draw_text(font, &label, label_x, y as f32, FONT_PX, TEXT);
        // ETA (+ SP tag), right-aligned to the panel edge.
        let sp = if f.is_hard { " SP" } else { "" };
        let eta = format!("{}{}", f.eta(), sp);
        let eta_w = Canvas::text_width(font, &eta, FONT_PX);
        canvas.draw_text(font, &eta, WIDTH as f32 - PAD as f32 - eta_w, y as f32, FONT_PX, DIM);
        y += LINE_H;
    }
    if fissures.len() > MAX_FISSURES {
        let more = format!("+{} more fissures", fissures.len() - MAX_FISSURES);
        canvas.draw_text(font, &more, PAD as f32, y as f32, FONT_PX, DIM);
    }

    canvas
}

/// One reward row for the reward-result panel.
#[derive(Debug, Clone)]
pub struct RewardRow {
    pub name: String,
    pub plat: Option<u32>,
    pub best_plat: bool,
    /// Whether the built item this reward belongs to is already mastered.
    pub mastered: bool,
}

// Reward panel palette.
const BEST_BG: Color = Color::rgba(40, 70, 45, 235);
const PLAT: Color = Color::rgb(120, 200, 255);
const MASTERED: Color = Color::rgb(130, 200, 140);

// Mastery emblem (laurel wreath): reserved left-column width + gap, and the drawn
// wreath size.
const MARK_W: u32 = 20;
const MARK_GAP: u32 = 6;
const MARK_INNER_W: u32 = 18;
const MARK_H: u32 = 16;

/// Render the relic reward-choice result panel: each choice with its plat/ducat
/// value, the best-plat row highlighted.
pub fn render_reward_panel(rows: &[RewardRow], font: &Font) -> Canvas {
    let n = rows.len().max(1) as i32;
    let height = (PAD * 2 + (n + 1) * LINE_H) as u32;
    let mut canvas = Canvas::new(WIDTH, height);
    canvas.fill_round_rect(0, 0, WIDTH, height, 12, BG);

    let mut y = PAD + 14;
    canvas.draw_text(font, "RELIC REWARD — BEST PICK", PAD as f32, y as f32, TITLE_PX, TITLE);
    y += LINE_H;

    let charw = font.metrics('0', FONT_PX).advance_width;
    let plat_x = WIDTH as f32 - PAD as f32 - 6.0 * charw; // "NNNNp"
    // Reserve a left column for the mastery emblem so names stay aligned whether or
    // not a row is mastered.
    let name_x = PAD as f32 + MARK_W as f32 + MARK_GAP as f32;
    let name_cols = ((plat_x - name_x) / charw).floor() as usize;

    for r in rows {
        // Highlight the best-plat row.
        if r.best_plat {
            canvas.fill_round_rect(
                (PAD / 2) as u32 as i32,
                y - 15,
                WIDTH - PAD as u32,
                LINE_H as u32,
                6,
                BEST_BG,
            );
        }
        // Mastery emblem in front of the name for items already mastered.
        if r.mastered {
            let mark_x = PAD + (MARK_W as i32 - MARK_INNER_W as i32) / 2;
            canvas.draw_mastery_mark(mark_x, y - MARK_H as i32 + 1, MARK_INNER_W, MARK_H, MASTERED);
        }
        let star = if r.best_plat { "* " } else { "  " };
        let label = format!("{star}{}", r.name);
        // Mastered items are dimmed (you already have them for mastery).
        let name_color = if r.mastered { DIM } else { TEXT };
        canvas.draw_text(font, &truncate(&label, name_cols), name_x, y as f32, FONT_PX, name_color);

        let plat = r.plat.map(|p| format!("{p}p")).unwrap_or_else(|| "—".into());
        canvas.draw_text(font, &plat, plat_x, y as f32, FONT_PX, PLAT);
        y += LINE_H;
    }
    canvas
}

/// One row of the owned-relic guide panel.
#[derive(Debug, Clone)]
pub struct RelicRow {
    /// Relic label, e.g. "Axi A1".
    pub name: String,
    /// Owned count.
    pub count: u32,
    /// Number of distinct unmastered built primes this relic can drop.
    pub unmastered: u32,
    /// The first unmastered prime, shown as a preview (e.g. "Nidus Prime").
    pub top_reward: String,
    /// Lowest market sell price in platinum, if resolved.
    pub plat: Option<u32>,
}

/// Render the owned-relic guide: relics that can still drop something you haven't
/// mastered, each with its owned count, a preview of the unmastered reward(s), and
/// the relic's market price.
pub fn render_relic_panel(rows: &[RelicRow], font: &Font) -> Canvas {
    let n = rows.len().max(1) as i32;
    let height = (PAD * 2 + (n + 1) * LINE_H) as u32;
    let mut canvas = Canvas::new(WIDTH, height);
    canvas.fill_round_rect(0, 0, WIDTH, height, 12, BG);

    let mut y = PAD + 14;
    canvas.draw_text(font, "RELICS — UNMASTERED REWARDS", PAD as f32, y as f32, TITLE_PX, TITLE);
    y += LINE_H;

    let charw = font.metrics('0', FONT_PX).advance_width;
    let plat_x = WIDTH as f32 - PAD as f32 - 6.0 * charw; // "NNNNp"
    let reward_x = PAD as f32 + 13.0 * charw; // after "Requiem III x9"
    let reward_cols = ((plat_x - reward_x) / charw).floor() as usize;

    for r in rows {
        let name = format!("{}  x{}", r.name, r.count);
        canvas.draw_text(font, &name, PAD as f32, y as f32, FONT_PX, TEXT);

        // Preview the unmastered reward(s): first prime, "+N" if there are more.
        let extra = if r.unmastered > 1 {
            format!(" +{}", r.unmastered - 1)
        } else {
            String::new()
        };
        let summary = format!("{}{}", r.top_reward, extra);
        canvas.draw_text(font, &truncate(&summary, reward_cols), reward_x, y as f32, FONT_PX, DIM);

        let plat = r.plat.map(|p| format!("{p}p")).unwrap_or_else(|| "—".into());
        canvas.draw_text(font, &plat, plat_x, y as f32, FONT_PX, PLAT);
        y += LINE_H;
    }
    canvas
}

/// Live progress for an in-flight owned-relic scan, shown before enough
/// relics have cleared their trust bar to render a real [`render_relic_panel`]
/// guide — so the player sees the app react to opening the Relics screen
/// immediately, rather than nothing until the first confirmed relic.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScanProgress {
    /// Relics marked Seen this session (see ADR-0009).
    pub seen: usize,
    /// Relics with a Confirmed count this session (see ADR-0005).
    pub confirmed: usize,
}

/// Render a "scanning in progress" status panel: shown from the moment the
/// Relics screen is detected until the first real guide row is ready.
pub fn render_relic_scanning_panel(progress: ScanProgress, font: &Font) -> Canvas {
    let height = (PAD * 2 + 2 * LINE_H) as u32;
    let mut canvas = Canvas::new(WIDTH, height);
    canvas.fill_round_rect(0, 0, WIDTH, height, 12, BG);

    let mut y = PAD + 14;
    canvas.draw_text(font, "SCANNING VOID RELICS…", PAD as f32, y as f32, TITLE_PX, TITLE);
    y += LINE_H;

    let status = format!("{} seen · {} confirmed", progress.seen, progress.confirmed);
    canvas.draw_text(font, &status, PAD as f32, y as f32, FONT_PX, DIM);
    canvas
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_nonempty_panel() {
        let font = load_font().expect("a system monospace font");
        let ws = WorldState::default();
        let canvas = render_panel(&ws, &font);
        assert_eq!(canvas.width, WIDTH);
        assert!(canvas.height > 0);
        // The background alone should make many pixels non-transparent.
        let opaque = canvas.buf.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(opaque > 100, "expected a visible panel background");
    }

    #[test]
    fn renders_scanning_panel_with_progress() {
        let font = load_font().expect("a system monospace font");
        let canvas = render_relic_scanning_panel(ScanProgress { seen: 3, confirmed: 1 }, &font);
        assert_eq!(canvas.width, WIDTH);
        let opaque = canvas.buf.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(opaque > 100, "expected a visible panel background");
    }

    #[test]
    fn truncate_helper() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("ab", 4), "ab");
    }
}
