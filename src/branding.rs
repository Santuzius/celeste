//! Bundled brand assets and the helpers that turn them into
//! consumer-specific shapes (filesystem path for notify-rust, raw RGBA
//! pixmap for `iced::window::Icon`). Embedding the SVG with
//! `include_bytes!` keeps the brand icon usable from `cargo run` and
//! Flatpak-style sandboxes that don't have an XDG-installed copy to
//! fall back on.

use std::sync::OnceLock;

use iced::window;
use resvg::{tiny_skia, usvg};

/// SVG bundled with the binary so notifications and the iced window
/// can render the brand icon without depending on an XDG-themed copy
/// being installed.
pub const CELESTE_ICON_SVG: &[u8] = include_bytes!("../assets/celeste-icon.svg");

/// The brand icon as an iced svg handle (in-window logo), parsed once.
pub fn logo() -> iced::widget::svg::Handle {
    static CACHED: OnceLock<iced::widget::svg::Handle> = OnceLock::new();
    CACHED.get_or_init(|| iced::widget::svg::Handle::from_memory(CELESTE_ICON_SVG)).clone()
}

/// Pixel side length used when rasterising the brand icon for the
/// iced window. Compositors downscale freely; oversampling once at a
/// generous size beats repeatedly handing them a 32×32.
const WINDOW_ICON_SIZE: u32 = 256;

/// Materialise [`CELESTE_ICON_SVG`] to
/// `$XDG_CACHE_HOME/celeste/celeste-icon.svg` the first time it's
/// asked for, then hand back the absolute path. notify-rust accepts
/// either a freedesktop icon name or an absolute path; the path form
/// sidesteps icon-theme lookup entirely so the brand icon shows up
/// even on minimal sessions where the desktop file hasn't been
/// installed.
pub fn icon_file_path() -> Option<&'static str> {
    static CACHED: OnceLock<Option<String>> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            let cache_root = std::env::var_os("XDG_CACHE_HOME")
                .map(std::path::PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME")
                        .map(|h| std::path::PathBuf::from(h).join(".cache"))
                })?;
            let dir = cache_root.join("celeste");
            std::fs::create_dir_all(&dir).ok()?;
            let path = dir.join("celeste-icon.svg");
            // Re-write on every cold start: the embedded bytes are
            // the source of truth, and a stale copy after an upgrade
            // would otherwise stick around forever.
            std::fs::write(&path, CELESTE_ICON_SVG).ok()?;
            path.into_os_string().into_string().ok()
        })
        .as_deref()
}

/// Rasterise the brand icon to RGBA at [`WINDOW_ICON_SIZE`]² and wrap
/// it in an [`iced::window::Icon`] suitable for `window::Settings::icon`.
/// Cached so repeated calls (e.g. tray "Open Celeste" reopening the
/// window) don't re-rasterise. Returns `None` if `usvg`/`resvg` fails;
/// callers fall back to whatever icon the compositor picks.
pub fn window_icon() -> Option<window::Icon> {
    static CACHED: OnceLock<Option<window::Icon>> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            let opts = usvg::Options::default();
            let tree = usvg::Tree::from_data(CELESTE_ICON_SVG, &opts).ok()?;
            let mut pixmap = tiny_skia::Pixmap::new(WINDOW_ICON_SIZE, WINDOW_ICON_SIZE)?;
            let svg_size = tree.size();
            let scale_x = WINDOW_ICON_SIZE as f32 / svg_size.width();
            let scale_y = WINDOW_ICON_SIZE as f32 / svg_size.height();
            let transform = tiny_skia::Transform::from_scale(scale_x, scale_y);
            resvg::render(&tree, transform, &mut pixmap.as_mut());
            // `window::Icon::from_rgba` wants non-premultiplied RGBA;
            // tiny-skia hands us premultiplied, so divide out alpha.
            let rgba = depremultiply_rgba(pixmap.data());
            window::icon::from_rgba(rgba, WINDOW_ICON_SIZE, WINDOW_ICON_SIZE).ok()
        })
        .clone()
}

fn depremultiply_rgba(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len());
    for px in src.chunks_exact(4) {
        let (r, g, b, a) = (px[0], px[1], px[2], px[3]);
        let (r, g, b) = if a == 0 {
            (0, 0, 0)
        } else {
            let a_u = a as u32;
            (
                ((r as u32 * 255 + a_u / 2) / a_u).min(255) as u8,
                ((g as u32 * 255 + a_u / 2) / a_u).min(255) as u8,
                ((b as u32 * 255 + a_u / 2) / a_u).min(255) as u8,
            )
        };
        out.extend_from_slice(&[r, g, b, a]);
    }
    out
}
