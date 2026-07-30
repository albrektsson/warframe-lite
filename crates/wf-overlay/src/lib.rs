//! Overlay rendering and display for warframe-lite.
//!
//! * [`canvas`] — a tiny dependency-free RGBA drawing surface.
//! * [`render`] — draws the world-state panel onto a canvas.
//! * [`layer`] — shows a canvas as a Wayland `wlr-layer-shell` overlay.

pub mod canvas;
pub mod layer;
pub mod render;

pub use canvas::Canvas;
pub use render::{load_font, render_panel, render_reward_panel, RewardRow};
