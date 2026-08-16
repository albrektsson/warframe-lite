//! Displays the overlay panel as a Wayland `wlr-layer-shell` surface.
//!
//! KWin (KDE Plasma) implements `wlr-layer-shell`, so this creates a top-layer,
//! click-through, always-on-top surface anchored to a screen corner. New frames
//! are pushed in via an [`mpsc`](std::sync::mpsc) channel of [`Canvas`]es; a
//! calloop timer drains the channel and repaints, so counting-down ETAs stay
//! fresh without the caller driving Wayland directly.

use std::sync::mpsc::Receiver;
use std::time::Duration;

use anyhow::{Context, Result};
use smithay_client_toolkit::reexports::calloop::{
    timer::{TimeoutAction, Timer},
    EventLoop,
};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
    Connection, QueueHandle,
};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};

use crate::canvas::Canvas;

/// A window rectangle on the X root: `(x, y, width, height)`.
pub type WindowRect = (i32, i32, u32, u32);

/// A live placement update pushed over `placement_rx`: the new [`Placement`]
/// paired with a freshly re-queried [`WindowRect`] (see [`run`]'s docs and
/// [`State::apply_placement`]). `pub` so the caller wiring up `run`'s
/// `placement_tx`/`placement_rx` channel (`wf-lite`'s control-socket
/// listener) can name the same type rather than repeating the tuple.
pub type PlacementUpdate = (Placement, Option<WindowRect>);

/// Where to anchor the overlay on screen. `margin_x`/`margin_y` are
/// fractions of the maximum meaningful inset on that axis (`0.0` flush
/// against the anchored edge, `1.0` centered) rather than raw pixels — see
/// [`edge_margins`] for the real-pixel conversion. Resolution-independent by
/// construction, so the same `Placement` lands in the same relative spot
/// whichever monitor it's ultimately applied to.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub anchor: Anchor,
    pub margin_x: f32,
    pub margin_y: f32,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            anchor: Anchor::TOP.union(Anchor::RIGHT),
            margin_x: 0.05,
            margin_y: 0.05,
        }
    }
}

impl Placement {
    /// Build a placement from a config anchor string (`top-left`, `top-right`,
    /// `bottom-left`, `bottom-right`, `top`, `bottom`, `left`, `right`,
    /// `center`). Unrecognised values fall back to top-right.
    pub fn parse(anchor: &str, margin_x: f32, margin_y: f32) -> Self {
        let a = match anchor.trim().to_lowercase().as_str() {
            "top-left" | "top_left" | "topleft" => Anchor::TOP.union(Anchor::LEFT),
            "top-right" | "top_right" | "topright" => Anchor::TOP.union(Anchor::RIGHT),
            "bottom-left" | "bottom_left" | "bottomleft" => Anchor::BOTTOM.union(Anchor::LEFT),
            "bottom-right" | "bottom_right" | "bottomright" => Anchor::BOTTOM.union(Anchor::RIGHT),
            "top" => Anchor::TOP,
            "bottom" => Anchor::BOTTOM,
            "left" => Anchor::LEFT,
            "right" => Anchor::RIGHT,
            "center" | "centre" => Anchor::empty(),
            other => {
                tracing::warn!("unknown overlay anchor {other:?}, using top-right");
                Anchor::TOP.union(Anchor::RIGHT)
            }
        };
        Self { anchor: a, margin_x, margin_y }
    }
}

/// Run the overlay until the compositor closes it or the process exits.
///
/// `initial` is drawn immediately; every [`Canvas`] received on `rx` thereafter
/// replaces it on the next timer tick. `window` is the game window's rectangle on
/// the X root (`x, y, w, h`): its centre picks the monitor the overlay appears on,
/// and its edges are folded into the anchor margins so the panel hugs the game
/// window's corner even when the game is borderless-windowed rather than
/// fullscreen. `None` lets the compositor choose the output and uses plain insets.
///
/// Every [`Placement`] received on `placement_rx` thereafter re-anchors the
/// already-committed layer surface in place — `set_anchor`/`set_margin`/
/// `set_size` again, then another `commit()` — with no teardown or
/// reconnection, so a settings UI can drive placement live (see
/// [`State::apply_placement`]).
pub fn run(
    initial: Canvas,
    rx: Receiver<Canvas>,
    placement: Placement,
    placement_rx: Receiver<PlacementUpdate>,
    window: Option<WindowRect>,
) -> Result<()> {
    let conn = Connection::connect_to_env().context("connecting to Wayland ($WAYLAND_DISPLAY)")?;
    let (globals, mut event_queue) = registry_queue_init(&conn).context("registry init")?;
    let qh: QueueHandle<State> = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).context("wl_compositor missing")?;
    let layer_shell =
        LayerShell::bind(&globals, &qh).context("wlr-layer-shell not supported by compositor")?;
    let shm = Shm::bind(&globals, &qh).context("wl_shm missing")?;

    let (width, height) = (initial.width, initial.height);
    let pool = SlotPool::new((width * height * 4) as usize, &shm).context("creating shm pool")?;

    let mut state = State {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        shm,
        pool,
        layer: None,
        width,
        height,
        canvas: initial,
        rx,
        placement_rx,
        window,
        chosen_geom: None,
        configured: false,
        closed: false,
    };

    // Learn the output layout (needs a couple of roundtrips for xdg-output info),
    // then pick the monitor containing `target`.
    for _ in 0..2 {
        event_queue.roundtrip(&mut state).context("output roundtrip")?;
    }
    let centre = window.map(|(x, y, w, h)| (x + w as i32 / 2, y + h as i32 / 2));
    let chosen = centre.and_then(|(tx, ty)| pick_output(&state.output_state, tx, ty));
    let chosen_geom = chosen
        .as_ref()
        .and_then(|o| state.output_state.info(o))
        .and_then(|i| Some((i.logical_position?, i.logical_size?)));
    if chosen.is_some() {
        tracing::info!("overlay placed on the monitor containing {centre:?}");
    }
    state.chosen_geom = chosen_geom;

    let (top, right, bottom, left) = edge_margins(placement, window, chosen_geom, (width, height));

    let surface = compositor.create_surface(&qh);
    let layer = layer_shell.create_layer_surface(
        &qh,
        surface,
        Layer::Overlay,
        Some("warframe-lite"),
        chosen.as_ref(),
    );
    layer.set_anchor(placement.anchor);
    layer.set_size(width, height);
    layer.set_margin(top, right, bottom, left);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer.set_exclusive_zone(0); // don't reserve desktop space

    // Empty input region → fully click-through (input passes to the game).
    if let Ok(region) = Region::new(&compositor) {
        layer.wl_surface().set_input_region(Some(region.wl_region()));
        region.wl_region().destroy();
    }
    layer.commit(); // triggers the initial configure
    state.layer = Some(layer);

    let mut event_loop: EventLoop<State> =
        EventLoop::try_new().context("creating calloop event loop")?;
    WaylandSource::new(conn, event_queue)
        .insert(event_loop.handle())
        .map_err(|e| anyhow::anyhow!("inserting Wayland source: {e}"))?;

    // Redraw timer: drain pending frames/placement updates and repaint/reposition.
    event_loop
        .handle()
        .insert_source(Timer::from_duration(Duration::from_millis(250)), |_, _, state| {
            let mut latest = None;
            while let Ok(c) = state.rx.try_recv() {
                latest = Some(c);
            }
            if let Some(c) = latest {
                state.canvas = c;
                if state.configured {
                    state.draw();
                }
            }
            let mut latest_placement = None;
            while let Ok(p) = state.placement_rx.try_recv() {
                latest_placement = Some(p);
            }
            if let Some((p, w)) = latest_placement {
                state.apply_placement(p, w);
            }
            TimeoutAction::ToDuration(Duration::from_millis(250))
        })
        .map_err(|e| anyhow::anyhow!("inserting timer: {e}"))?;

    tracing::info!("overlay running ({width}x{height})");
    loop {
        event_loop
            .dispatch(Duration::from_millis(500), &mut state)
            .context("event loop dispatch")?;
        if state.closed {
            tracing::info!("overlay closed by compositor");
            break;
        }
    }
    Ok(())
}

struct State {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,
    layer: Option<LayerSurface>,
    width: u32,
    height: u32,
    canvas: Canvas,
    rx: Receiver<Canvas>,
    /// New [`Placement`]s pushed live by a settings UI, paired with a
    /// freshly re-queried game window rectangle (see [`run`]'s docs and
    /// [`Self::apply_placement`]).
    placement_rx: Receiver<PlacementUpdate>,
    /// The game window's rectangle. Set once at startup, then overwritten by
    /// [`Self::apply_placement`] with whatever the caller re-queried just
    /// before pushing a live [`Placement`] — the startup snapshot alone
    /// isn't safe to keep reusing: the overlay is deliberately started early
    /// (see [`run`]'s docs), often while the game is still on a
    /// loading/menu-sized window, so a margin recomputed against that stale
    /// rectangle can land far from where the same anchor/margin values
    /// would land against the game's actual, current window (see the
    /// regression this fixed: settings showing `top-right` while the
    /// rendered overlay sat near screen-center, traced to exactly this
    /// staleness).
    window: Option<WindowRect>,
    /// The chosen output's logical geometry, captured once at startup — see
    /// `window`'s docs.
    chosen_geom: Option<((i32, i32), (i32, i32))>,
    configured: bool,
    closed: bool,
}

impl State {
    /// Blit the current canvas into a fresh shm buffer and commit it.
    fn draw(&mut self) {
        if self.layer.is_none() {
            return;
        }
        let stride = self.width as i32 * 4;
        let (buffer, canvas_slice) = match self.pool.create_buffer(
            self.width as i32,
            self.height as i32,
            stride,
            wl_shm::Format::Argb8888,
        ) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("shm create_buffer failed: {e}");
                return;
            }
        };

        let src = self.canvas.to_argb_premul();
        let n = src.len().min(canvas_slice.len());
        canvas_slice[..n].copy_from_slice(&src[..n]);

        let surface = self.layer.as_ref().unwrap().wl_surface();
        if let Err(e) = buffer.attach_to(surface) {
            tracing::error!("attach buffer failed: {e}");
            return;
        }
        surface.damage_buffer(0, 0, self.width as i32, self.height as i32);
        surface.commit();
    }

    /// Re-anchor the already-committed layer surface to a new [`Placement`]
    /// pushed live over the control socket — no teardown, just
    /// `set_anchor`/`set_margin`/`set_size` again and a fresh `commit()`
    /// (see [`run`]'s docs). `window` is the caller's freshly re-queried
    /// game window rectangle, replacing `self.window` before recomputing
    /// margins — see [`State::window`]'s docs for why the startup snapshot
    /// alone goes stale.
    fn apply_placement(&mut self, placement: Placement, window: Option<WindowRect>) {
        let Some(layer) = &self.layer else {
            return;
        };
        self.window = window;
        let (top, right, bottom, left) =
            edge_margins(placement, self.window, self.chosen_geom, (self.width, self.height));
        layer.set_anchor(placement.anchor);
        layer.set_size(self.width, self.height);
        layer.set_margin(top, right, bottom, left);
        layer.commit();
        tracing::info!("overlay placement updated live");
    }
}

/// Convert a placement's x/y margin *fractions* (see [`Placement`]'s docs)
/// into real pixel insets against a `(width, height)` container — the game
/// window when known, else the output, else a synthetic fallback (see
/// [`edge_margins`]). `0.0` gives `0` px (flush against the edge); `1.0`
/// gives half of whatever room is left after the panel itself, which is
/// exactly the offset at which the panel's anchored edge coincides with the
/// container's centre.
fn margin_px(fraction_x: f32, fraction_y: f32, container: (i32, i32), panel: (u32, u32)) -> (i32, i32) {
    let max_x = (container.0 - panel.0 as i32).max(0) as f32 / 2.0;
    let max_y = (container.1 - panel.1 as i32).max(0) as f32 / 2.0;
    (
        (fraction_x.clamp(0.0, 1.0) * max_x).round() as i32,
        (fraction_y.clamp(0.0, 1.0) * max_y).round() as i32,
    )
}

/// Margins (`top, right, bottom, left`) that place the panel at the placement's
/// x/y insets from the game window's corner. Given the window rect and its
/// output's logical geometry, each edge margin is the gap from that output edge to
/// the matching window edge plus the inset (in real pixels — see [`margin_px`],
/// resolved against the window's own size so the inset fraction is relative to
/// the window the panel is being placed inside of); the compositor only applies
/// the margins on the anchored edges, so whichever corner [`Placement`] anchors
/// to, the panel lands the requested inset inside the game window there. Falls
/// back to the output's own size when the window rect is unknown (compositor-
/// chosen output / fullscreen, where window edges equal output edges), or to a
/// synthetic container sized off the panel itself when neither is known.
fn edge_margins(
    placement: Placement,
    window: Option<WindowRect>,
    output: Option<((i32, i32), (i32, i32))>,
    panel: (u32, u32),
) -> (i32, i32, i32, i32) {
    if let (Some((wx, wy, ww, wh)), Some(((lx, ly), (lw, lh)))) = (window, output) {
        let (mx, my) = margin_px(placement.margin_x, placement.margin_y, (ww as i32, wh as i32), panel);
        let top = (wy - ly) + my;
        let left = (wx - lx) + mx;
        let right = (lx + lw) - (wx + ww as i32) + mx;
        let bottom = (ly + lh) - (wy + wh as i32) + my;
        return (top.max(0), right.max(0), bottom.max(0), left.max(0));
    }
    let container = output
        .map(|(_, (lw, lh))| (lw, lh))
        .unwrap_or((panel.0 as i32 * 2, panel.1 as i32 * 2));
    let (mx, my) = margin_px(placement.margin_x, placement.margin_y, container, panel);
    (my, mx, my, mx)
}

/// Pick the [`wl_output`](wl_output::WlOutput) whose logical geometry contains
/// the point `(px, py)` — used to place the overlay on the game's monitor.
fn pick_output(
    output_state: &OutputState,
    px: i32,
    py: i32,
) -> Option<wl_output::WlOutput> {
    for output in output_state.outputs() {
        let Some(info) = output_state.info(&output) else {
            continue;
        };
        if let (Some((lx, ly)), Some((lw, lh))) = (info.logical_position, info.logical_size) {
            if px >= lx && px < lx + lw && py >= ly && py < ly + lh {
                tracing::debug!("target ({px},{py}) → output at ({lx},{ly}) {lw}x{lh}");
                return Some(output);
            }
        }
    }
    None
}

impl LayerShellHandler for State {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        self.closed = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        // Honor a non-zero size hint from the compositor; otherwise keep ours.
        let (w, h) = configure.new_size;
        if w != 0 && h != 0 {
            self.width = w;
            self.height = h;
        }
        self.configured = true;
        self.draw();
    }
}

impl CompositorHandler for State {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_registry!(State);

smithay_client_toolkit::delegate_dispatch2!(State);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_maps_corner_strings() {
        assert_eq!(
            Placement::parse("bottom-left", 0.1, 0.2).anchor,
            Anchor::BOTTOM.union(Anchor::LEFT)
        );
        // unknown falls back to top-right, preserving margins.
        let p = Placement::parse("nonsense", 0.5, 0.7);
        assert_eq!(p.anchor, Anchor::TOP.union(Anchor::RIGHT));
        assert_eq!((p.margin_x, p.margin_y), (0.5, 0.7));
    }

    #[test]
    fn fullscreen_window_gives_bare_insets() {
        // Window exactly fills its output → margins are just the fraction-derived
        // insets, resolved against the 3440x1440 output and a 460x340 panel:
        // max_x = (3440-460)/2 = 1490, max_y = (1440-340)/2 = 550.
        let p = Placement::parse("top-right", 0.2, 0.1);
        let m = edge_margins(p, Some((0, 0, 3440, 1440)), Some(((0, 0), (3440, 1440))), (460, 340));
        assert_eq!(m, (55, 298, 55, 298)); // top, right, bottom, left
    }

    #[test]
    fn windowed_offset_folds_into_margins() {
        // A 100x60 panel placed inside a 1000x800 window at (100,50), itself
        // inside a 1920x1080 output: max_x = (1000-100)/2 = 450,
        // max_y = (800-60)/2 = 370.
        let p = Placement::parse("top-right", 0.2, 0.1);
        let (top, right, bottom, left) =
            edge_margins(p, Some((100, 50, 1000, 800)), Some(((0, 0), (1920, 1080))), (100, 60));
        assert_eq!(top, 50 + 37);
        assert_eq!(left, 100 + 90);
        assert_eq!(right, (1920 - 1100) + 90);
        assert_eq!(bottom, (1080 - 850) + 37);
    }

    #[test]
    fn unknown_geometry_falls_back_to_a_panel_sized_container() {
        // No window, no output → container defaults to double the panel's own
        // size: max_x = (200-100)/2 = 50, max_y = (120-60)/2 = 30.
        let p = Placement::parse("top-left", 0.4, 0.5);
        assert_eq!(edge_margins(p, None, None, (100, 60)), (15, 20, 15, 20));
    }

    #[test]
    fn margin_fractions_are_clamped_to_0_1() {
        // Stale/garbage fractions (e.g. a leftover absolute-pixel value from
        // before margins became fractions) must clamp rather than blow past
        // the container or go negative.
        let p = Placement::parse("top-right", 5.0, -1.0);
        let m = edge_margins(p, Some((0, 0, 2000, 1000)), Some(((0, 0), (2000, 1000))), (400, 200));
        assert_eq!(m, (0, 800, 0, 800));
    }
}
