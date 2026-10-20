//! Every size in rem, in steps of a quarter (0.25, 0.5, 0.75, 1 ...);
//! 1 rem the base margin (between cards, around the grids and the
//! player). A rem is the desktop's font size (GNOME's `font-name`) in
//! pixels, times the screen's scale, times the window's zoom: the sizes
//! are counted in the screen's pixels, the text drawn at its size, crisp.
//!
//! The zoom (Ctrl+= or Ctrl++ bigger, Ctrl+- smaller, Ctrl+0 back to
//! 100%) is kept across starts in `~/.local/state/mascarpone/zoom`.
//!
//! The cards have a fixed size. The Albums and Artists tabs are as wide
//! as the window, as many cards across as fit; the rest keeps to a
//! column as wide as three album cards and the gaps between them,
//! centred: two playlist cards fill it, and so do an artist's page (its
//! albums three across), the album's and playlist's pages, the queue
//! and the player.

use std::cell::Cell;
use std::path::PathBuf;

/// A cover's side, and an artist's portrait's in the grid.
pub const COVER: f32 = 12.0;
/// A card's padding.
pub const PAD: f32 = 0.5;
/// Between two cards.
pub const GAP: f32 = 1.0;
/// Cards across the column.
pub const COLUMNS: usize = 3;

const ZOOM_STEP: f32 = 0.1;
const ZOOM_MIN: f32 = 0.5;
const ZOOM_MAX: f32 = 2.0;

thread_local! {
    /// The desktop's font size in its pixels, read once.
    static BASE: f32 = desktop_font_px();
    static ZOOM: Cell<f32> = const { Cell::new(1.0) };
    /// The screen's pixels to its points.
    static SCALE: Cell<f32> = const { Cell::new(1.0) };
}

/// The desktop's font in pixels: its size in points at 96 dpi, times
/// the text scaling.
fn desktop_font_px() -> f32 {
    let gsettings = |key: &str| {
        std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", key])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_matches('\'').to_owned())
    };
    let name = gsettings("font-name").unwrap_or_else(|| "Sans 11".into());
    let size = name.split_whitespace().last().unwrap_or("11");
    let px = match size.strip_suffix("px") {
        Some(px) => px.parse::<f32>().ok(),
        None => size.parse::<f32>().ok().map(|pt| pt * 96.0 / 72.0),
    };
    let factor = gsettings("text-scaling-factor").and_then(|f| f.parse::<f32>().ok()).filter(|f| *f > 0.0).unwrap_or(1.0);
    px.filter(|p| *p > 0.0).unwrap_or(11.0 * 96.0 / 72.0) * factor
}

/// Reads the desktop's font and the zoom kept from last time.
pub fn init() {
    BASE.with(|_| ());
    let zoom = std::fs::read_to_string(zoom_path()).ok().and_then(|s| s.trim().parse::<f32>().ok()).unwrap_or(1.0);
    set_zoom(zoom);
}

/// The screen's scale, from the window: once a frame.
pub fn set_scale(scale: f32) {
    SCALE.with(|s| s.set(if scale > 0.0 { scale } else { 1.0 }));
}

pub fn scale() -> f32 {
    SCALE.with(Cell::get)
}

/// A rem in pixels, unrounded: for the font sizes.
pub fn px() -> f32 {
    BASE.with(|b| *b) * ZOOM.with(Cell::get) * scale()
}

/// `x` rem in pixels.
pub fn rem(x: f32) -> f32 {
    (x * px()).round()
}

/// One step bigger (1) or smaller (-1).
pub fn zoom(step: i32) {
    save(ZOOM.with(Cell::get) + step as f32 * ZOOM_STEP);
}

/// Back to 100%.
pub fn reset() {
    save(1.0);
}

fn save(zoom: f32) {
    set_zoom(zoom);
    let path = zoom_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, format!("{:.1}\n", ZOOM.with(Cell::get))) {
        eprintln!("{}: {e}", path.display());
    }
}

fn set_zoom(zoom: f32) {
    let zoom = ((zoom / ZOOM_STEP).round() * ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX);
    ZOOM.with(|z| z.set(zoom));
}

fn zoom_path() -> PathBuf {
    crate::dirs::state().join("mascarpone").join("zoom")
}

/// Half the gap: the margin around every card in a grid.
pub fn half_gap() -> f32 {
    rem(GAP / 2.0)
}

/// Between two cards.
pub fn gap() -> f32 {
    2.0 * half_gap()
}

/// A card's width: the cover and the padding.
pub fn card() -> f32 {
    rem(COVER) + 2.0 * rem(PAD)
}

/// The column: three cards and the two gaps between them.
pub fn column() -> f32 {
    COLUMNS as f32 * card() + (COLUMNS as f32 - 1.0) * gap()
}
