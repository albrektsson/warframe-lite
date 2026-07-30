//! X11 screen capture of the Warframe window.
//!
//! Under Steam + Proton on a Wayland desktop, Warframe renders through
//! **Xwayland**, so it is an ordinary X11 window that a pure-Rust X11 client
//! ([`x11rb`]) can locate and read pixels from — no `xdg-desktop-portal` prompt,
//! no compositor-specific screenshot API. This is the capture path for the relic
//! reward picker.

use anyhow::{Context, Result};
use image::RgbaImage;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt, ImageFormat, Window,
};
use x11rb::rust_connection::RustConnection;

/// A captured frame plus where it sits on the root window.
pub struct Capture {
    pub image: RgbaImage,
    /// Absolute position of the captured region on the X root window.
    pub root_x: i16,
    pub root_y: i16,
}

/// A rectangular sub-region of a window, in window-local coordinates.
#[derive(Debug, Clone, Copy)]
pub struct Region {
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
}

/// Connect to the X server named by `$DISPLAY`.
pub fn connect() -> Result<(RustConnection, usize)> {
    let (conn, screen) = x11rb::connect(None).context("connecting to X server ($DISPLAY)")?;
    Ok((conn, screen))
}

/// Find the top-level Warframe window by matching its title (case-insensitive
/// substring). Searches the window tree breadth-first with a small depth cap.
pub fn find_window(conn: &RustConnection, root: Window, title_substr: &str) -> Result<Option<Window>> {
    let net_wm_name = conn.intern_atom(false, b"_NET_WM_NAME")?.reply()?.atom;
    let needle = title_substr.to_lowercase();

    // BFS with depth cap: the game window is a near-root child in practice.
    let mut frontier = vec![root];
    for _depth in 0..5 {
        let mut next = Vec::new();
        for win in frontier {
            let tree = conn.query_tree(win)?.reply()?;
            for &child in &tree.children {
                if window_title_matches(conn, child, net_wm_name, &needle)? {
                    return Ok(Some(child));
                }
                next.push(child);
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Ok(None)
}

/// Whether `win`'s `_NET_WM_NAME` or `WM_NAME` contains `needle` (already lowercased).
fn window_title_matches(
    conn: &RustConnection,
    win: Window,
    net_wm_name: u32,
    needle: &str,
) -> Result<bool> {
    for prop in [net_wm_name, u32::from(AtomEnum::WM_NAME)] {
        let reply = conn
            .get_property(false, win, prop, AtomEnum::ANY, 0, 1024)?
            .reply()?;
        if reply.value.is_empty() {
            continue;
        }
        let name = String::from_utf8_lossy(&reply.value).to_lowercase();
        if name.contains(needle) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Capture a sub-region of `win`. Pass `None` to capture the whole window.
pub fn capture(conn: &RustConnection, win: Window, region: Option<Region>) -> Result<Capture> {
    let geom = conn.get_geometry(win)?.reply()?;
    let region = region.unwrap_or(Region {
        x: 0,
        y: 0,
        width: geom.width,
        height: geom.height,
    });

    // Absolute position on the root window (for later overlay alignment).
    let root = conn.setup().roots[0].root;
    let abs = conn
        .translate_coordinates(win, root, region.x, region.y)?
        .reply()?;

    let img = conn
        .get_image(
            ImageFormat::Z_PIXMAP,
            win,
            region.x,
            region.y,
            region.width,
            region.height,
            !0, // all planes
        )?
        .reply()
        .context("GetImage failed (region may exceed max request size)")?;

    let image = decode_zpixmap(conn, &img.data, region.width, region.height)?;
    Ok(Capture {
        image,
        root_x: abs.dst_x,
        root_y: abs.dst_y,
    })
}

/// Convert an X `ZPixmap` byte buffer into an [`RgbaImage`].
///
/// Assumes the common Xwayland case: 32 bits-per-pixel, little-endian
/// (LSBFirst) byte order, giving `[B, G, R, X]` per pixel.
fn decode_zpixmap(conn: &RustConnection, data: &[u8], width: u16, height: u16) -> Result<RgbaImage> {
    let (w, h) = (width as usize, height as usize);
    let bpp = 4; // 32 bits-per-pixel
    let expected = w * h * bpp;
    anyhow::ensure!(
        data.len() >= expected,
        "unexpected image size: got {} bytes, need {} ({}x{} @ {}bpp)",
        data.len(),
        expected,
        w,
        h,
        bpp * 8
    );
    let lsb_first = conn.setup().image_byte_order == x11rb::protocol::xproto::ImageOrder::LSB_FIRST;

    let mut out = RgbaImage::new(width as u32, height as u32);
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * bpp;
            let (b, g, r) = if lsb_first {
                (data[i], data[i + 1], data[i + 2])
            } else {
                // MSBFirst: [X, R, G, B]
                (data[i + 3], data[i + 2], data[i + 1])
            };
            out.put_pixel(x as u32, y as u32, image::Rgba([r, g, b, 255]));
        }
    }
    Ok(out)
}

/// Locate the Warframe window and return its rectangle on the X root
/// (`x, y, width, height`) without reading pixels — cheap, used to decide which
/// monitor the overlay should appear on.
pub fn warframe_geometry() -> Result<(i32, i32, u32, u32)> {
    let (conn, screen) = connect()?;
    let root = conn.setup().roots[screen].root;
    let win = find_window(&conn, root, "Warframe")?
        .context("Warframe window not found (is the game running under Xwayland?)")?;
    let geom = conn.get_geometry(win)?.reply()?;
    let abs = conn.translate_coordinates(win, root, 0, 0)?.reply()?;
    Ok((abs.dst_x as i32, abs.dst_y as i32, geom.width as u32, geom.height as u32))
}

/// Convenience: connect, locate the Warframe window, and capture `region`
/// (or the whole window if `None`).
pub fn capture_warframe(region: Option<Region>) -> Result<Capture> {
    let (conn, screen) = connect()?;
    let root = conn.setup().roots[screen].root;
    let win = find_window(&conn, root, "Warframe")?
        .context("Warframe window not found (is the game running under Xwayland?)")?;
    tracing::info!("found Warframe window {win:#x}");
    capture(&conn, win, region)
}
