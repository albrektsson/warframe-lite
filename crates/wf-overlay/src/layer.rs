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
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
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

/// Where to anchor the overlay on screen.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub anchor: Anchor,
    pub margin_x: i32,
    pub margin_y: i32,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            anchor: Anchor::TOP.union(Anchor::RIGHT),
            margin_x: 24,
            margin_y: 24,
        }
    }
}

impl Placement {
    /// Build a placement from a config anchor string (`top-left`, `top-right`,
    /// `bottom-left`, `bottom-right`, `top`, `bottom`, `left`, `right`,
    /// `center`). Unrecognised values fall back to top-right.
    pub fn parse(anchor: &str, margin_x: i32, margin_y: i32) -> Self {
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
pub fn run(
    initial: Canvas,
    rx: Receiver<Canvas>,
    placement: Placement,
    window: Option<(i32, i32, u32, u32)>,
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

    let (top, right, bottom, left) = edge_margins(placement, window, chosen_geom);

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

    // Redraw timer: drain pending frames and repaint.
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
}

/// Margins (`top, right, bottom, left`) that place the panel at the placement's
/// x/y insets from the game window's corner. Given the window rect and its
/// output's logical geometry, each edge margin is the gap from that output edge to
/// the matching window edge plus the inset; the compositor only applies the
/// margins on the anchored edges, so whichever corner [`Placement`] anchors to, the
/// panel lands the requested inset inside the game window there. Falls back to the
/// bare insets when either rect is unknown (compositor-chosen output / fullscreen,
/// where window edges equal output edges).
fn edge_margins(
    placement: Placement,
    window: Option<(i32, i32, u32, u32)>,
    output: Option<((i32, i32), (i32, i32))>,
) -> (i32, i32, i32, i32) {
    let (mx, my) = (placement.margin_x, placement.margin_y);
    if let (Some((wx, wy, ww, wh)), Some(((lx, ly), (lw, lh)))) = (window, output) {
        let top = (wy - ly) + my;
        let left = (wx - lx) + mx;
        let right = (lx + lw) - (wx + ww as i32) + mx;
        let bottom = (ly + lh) - (wy + wh as i32) + my;
        return (top.max(0), right.max(0), bottom.max(0), left.max(0));
    }
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

delegate_compositor!(State);
delegate_output!(State);
delegate_shm!(State);
delegate_layer!(State);
delegate_registry!(State);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_maps_corner_strings() {
        assert_eq!(
            Placement::parse("bottom-left", 1, 2).anchor,
            Anchor::BOTTOM.union(Anchor::LEFT)
        );
        // unknown falls back to top-right, preserving margins.
        let p = Placement::parse("nonsense", 5, 7);
        assert_eq!(p.anchor, Anchor::TOP.union(Anchor::RIGHT));
        assert_eq!((p.margin_x, p.margin_y), (5, 7));
    }

    #[test]
    fn fullscreen_window_gives_bare_insets() {
        // Window exactly fills its output → margins are just the insets.
        let p = Placement::parse("top-right", 24, 12);
        let m = edge_margins(p, Some((0, 0, 3440, 1440)), Some(((0, 0), (3440, 1440))));
        assert_eq!(m, (12, 24, 12, 24)); // top, right, bottom, left
    }

    #[test]
    fn windowed_offset_folds_into_margins() {
        // A 1000x800 window at (100,50) inside a 1920x1080 output.
        let p = Placement::parse("top-right", 10, 10);
        let (top, right, bottom, left) =
            edge_margins(p, Some((100, 50, 1000, 800)), Some(((0, 0), (1920, 1080))));
        assert_eq!(top, 50 + 10);
        assert_eq!(left, 100 + 10);
        assert_eq!(right, (1920 - 1100) + 10);
        assert_eq!(bottom, (1080 - 850) + 10);
    }

    #[test]
    fn unknown_geometry_uses_insets() {
        let p = Placement::parse("top-left", 8, 4);
        assert_eq!(edge_margins(p, None, None), (4, 8, 4, 8));
    }
}
