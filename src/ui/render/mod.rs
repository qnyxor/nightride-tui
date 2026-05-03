// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Render layer.
//!
//! Splits the frame area into:
//!
//! - **Top chord row** (1 row) — `← → stations | mute | + - volume`
//!   chip, right-aligned. Always-on chrome.
//! - **Song band** (3 rows, borderless, fades with `panel_visibility`):
//!   - row 0 — `[ STATION // Artist / Title ]` marquee.
//!   - row 1 — header band: `50 % ||||||||||  Dwallclock  uptime  song-elapsed`.
//!   - row 2 — spectrum baseline (Braille canvas, bars rise into row 2).
//!
//! Render functions receive `&App` and a target [`Rect`]. Layout
//! helpers consolidate the recurring single-row Rect builds and
//! visibility gates so each render fn stays focused on its surface.

use std::sync::atomic::Ordering;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;

use super::app::App;
use crate::theme::Theme;

mod chord;
mod header;
mod now_playing;
mod spectrum;

pub use header::{format_d_timestamp, format_d_timestamp_red_fragment, format_elapsed_hms};

/// Outer left/right padding (in cells) so no surface glues to the
/// terminal edges. Mirrors the v1.x mockup breathing room.
const SIDE_PADDING: u16 = 2;

/// Reserved height (cells) for the volume + spectrum band slot below
/// the song band. The volume label has migrated up to the timer row,
/// so this slot now hosts the spectrum alone.
const VOLUME_SPECTRUM_BAND_HEIGHT: u16 = 2;

/// Right-side breathing margin so the spectrum tail does not glue to
/// the terminal edge.
const SPECTRUM_RIGHT_MARGIN: u16 = 2;

/// Inset (cells) from the song-band's left edge for the volume +
/// spectrum band. Zero: both surfaces flush-left inside the
/// `SIDE_PADDING` envelope.
const BAND_LEFT_INSET: u16 = 0;

/// Vertical span (cells) of the spectrum canvas. Two rows give the
/// Braille canvas 8 sub-pixels of vertical resolution; combined with
/// `spectrum::SPECTRUM_TOP_CROP_SUB = 4`, the bars only ever render
/// in the bottom row of the rect — the top row exists solely to give
/// the canvas its full vertical sub-pixel budget. The rect anchors
/// at `band_y - 1` so the visible bottom row sits flush with the row
/// reserved for the spectrum baseline.
const SPECTRUM_HEIGHT: u16 = 2;

/// Minimum frame dimensions required for any meaningful layout. Below
/// this `render_main` short-circuits — Canvas / Block widgets panic on
/// rects that overflow the framebuffer.
const MIN_FRAME_HEIGHT: u16 = 4;
const MIN_FRAME_WIDTH: u16 = 16;

/// Visualizer width slack: subtract from `area.width` to compute the
/// number of RMS columns the audio supervisor should produce. Mirrors
/// the spectrum-margin envelope (left inset + right margin) that does
/// not contain spectrum cells.
const VISUALIZER_RIGHT_SLACK: u16 = BAND_LEFT_INSET + SPECTRUM_RIGHT_MARGIN;

/// Top-level render dispatcher. Splits the area into chord row + song
/// panel + volume/spectrum band and delegates to the per-surface
/// render fns. Re-exported via [`render_main_test_export`] for
/// integration tests.
pub(crate) fn render_main(frame: &mut ratatui::Frame, app: &App) {
    let frame_area = frame.area();

    // Defensive: a terminal mid-resize can drop to single-digit
    // dimensions for one or two frames. Skip rendering instead of
    // letting Canvas / Block widgets panic on rects that overflow
    // the framebuffer. 4 rows + 16 cols is the minimum where any
    // layout makes sense.
    if frame_area.height < MIN_FRAME_HEIGHT || frame_area.width < MIN_FRAME_WIDTH {
        return;
    }

    // Outer left/right breathing room so no surface glues to the
    // terminal edges.
    let padded = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(SIDE_PADDING),
            Constraint::Min(0),
            Constraint::Length(SIDE_PADDING),
        ])
        .split(frame_area);
    let area = padded[1];

    // Update the visualizer width tap so audio.rs computes the right
    // number of RMS columns next batch.
    app.visualizer_width.store(
        usize::from(area.width.saturating_sub(VISUALIZER_RIGHT_SLACK)).max(1),
        Ordering::Relaxed,
    );

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            // Top chord row: dedicated 1-row band ABOVE the song band
            // hosts the right-aligned key-binding chip so it does not
            // compete with any song-band content.
            Constraint::Length(1),
            // Borderless song band: 3 rows. Row 0 carries the
            // now-playing `[ STATION // Artist / Title ]` marquee,
            // row 1 (centre) carries the header band (volume +
            // wallclock + uptime + song-elapsed), row 2 anchors the
            // spectrum baseline.
            Constraint::Length(3),
            // Spectrum-only band below the song band. One row of
            // budget so peak-amplitude bars rise into row `band_y`
            // (the song-band's bottom row) without ever touching the
            // header band above.
            Constraint::Length(VOLUME_SPECTRUM_BAND_HEIGHT.saturating_sub(1)),
            Constraint::Min(0), // spare
        ])
        .split(area);

    // Order: chord row first, now-playing on the song band's TOP row,
    // header on its centre row. The two never collide because
    // `now_playing` renders on `area.y` and `header` renders on
    // `vertical_center_y(area)` directly.
    chord::render(frame, layout[0], app);
    now_playing::render(frame, layout[1], app);
    header::render(frame, layout[1], app);

    // Clip every explicit Rect against the frame bounds so a mid-
    // resize never hands a widget an out-of-bounds area. Canvas
    // (Braille) is particularly sensitive — its sub-pixel grid
    // assumes the area sits inside the buffer.
    //
    // Anchor row: the first row right under the now-playing line.
    // The song band is 3 rows; the now-playing line sits on its
    // centre row, so the row immediately below is the band's bottom
    // row — that is where the spectrum baseline anchors.
    let band_y = layout[1]
        .y
        .saturating_add(layout[1].height)
        .saturating_sub(1);

    let band_x = layout[1].x.saturating_add(BAND_LEFT_INSET);
    let band_w = layout[1].width.saturating_sub(BAND_LEFT_INSET);

    // Spectrum spans from `band_x` (same x as the volume label and
    // the now-playing `[`) to the right edge minus a breathing margin,
    // so the bars sweep nearly the full width of the row.
    let spectrum_x = band_x;
    let spectrum_w = band_w.saturating_sub(SPECTRUM_RIGHT_MARGIN);
    let spectrum_rect = Rect {
        x: spectrum_x,
        // Anchor one row above the volume baseline so the canvas's
        // bottom row aligns with `band_y` while the (cropped, never
        // drawn) top row sits behind the now-playing line on row
        // `band_y - 1`. The crop guarantees no overpaint.
        y: band_y.saturating_sub(1),
        width: spectrum_w,
        height: SPECTRUM_HEIGHT,
    }
    .intersection(frame_area);
    if spectrum_rect.height > 0 && spectrum_rect.width > 0 {
        spectrum::render(frame, spectrum_rect, app);
    }
}

/// Test-only re-export of [`render_main`]. Gated behind the
/// `test-export` Cargo feature so production release builds carry zero
/// leakage; integration snapshot tests under `tests/` opt in via
/// `required-features = ["test-export"]` in `Cargo.toml`.
#[cfg(any(test, feature = "test-export"))]
pub fn render_main_test_export(frame: &mut ratatui::Frame, app: &App) {
    render_main(frame, app);
}

/// Build a single-row [`Rect`] at `(x, y)` of `width`. Centralises the
/// idiom that recurs across every render fn that paints text on a
/// reserved row.
pub(super) fn single_row_rect(x: u16, y: u16, width: u16) -> Rect {
    Rect {
        x,
        y,
        width,
        height: 1,
    }
}

/// Compute the y coordinate of the visual centre row of `rect`. Used
/// to vertically centre 1-row content inside an N-row band without
/// repeating the `(height - 1) / 2` math at every call site.
pub(super) fn vertical_center_y(rect: Rect) -> u16 {
    rect.y.saturating_add(rect.height.saturating_sub(1) / 2)
}

/// Apply a visibility-gate `alpha` to the foreground colour of `style`,
/// leaving every other field (modifier set, bg, etc.) untouched. Used
/// to attenuate the tri-coloured song-line so it fades in/out cleanly
/// as the panel visibility gate transitions.
pub(super) fn fade_style_fg(theme: &Theme, style: Style, alpha: f32) -> Style {
    if let Some(fg) = style.fg {
        return style.fg(theme.fade_color(fg, alpha));
    }
    style
}
