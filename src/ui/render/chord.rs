// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Top-right chord chip painted on the dedicated chord row above the
//! song panel.
//!
//! Reading order: `← → stations | m mute | + - volume | t type`.
//! Key letters (`← →`, `m`, `+`, `-`, `t`) render in `text_mid_style`
//! so the actionable glyphs stand out; the label tails (`tations`,
//! `ute`, `olume`, `ype`) render in `dim_style`.
//!
//! Adaptive collapse: when the right-side budget can't fit the full
//! form, segments drop right-to-left (type → volume → mute → nothing).
//! The TUI never renders a clipped fragment. Always-on chrome — does
//! NOT gate on `panel_visibility` so the navigation hint remains
//! visible during idle / reconnecting.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::super::app::App;
use super::single_row_rect;

/// Inset cells reserved on each side of the chord chip.
const CHORD_INSET: u16 = 2;

/// Minimum row width below which the chord renders nothing rather than
/// a clipped fragment.
const CHORD_MIN_WIDTH: u16 = 6;

/// Render the chord chip.
pub(super) fn render(frame: &mut ratatui::Frame, chord_row: Rect, app: &App) {
    if chord_row.height == 0 || chord_row.width < CHORD_MIN_WIDTH {
        return;
    }
    let dim = app.theme.dim_style();
    let mid = app.theme.text_mid_style();

    let avail = usize::from(chord_row.width.saturating_sub(CHORD_INSET));

    // `← → stations | m mute | + - volume`
    //  mid: ← →          mid: m       mid: + -
    //  dim: stations |    dim: ute |   dim: volume
    let seg_stations: Vec<Span<'static>> =
        vec![Span::styled("← → ", mid), Span::styled("stations", dim)];
    let seg_stations_w: usize = 12; // "← → " (4) + "stations" (8)

    let sep: Vec<Span<'static>> = vec![Span::styled(" | ", dim)];
    let sep_w: usize = 3;

    let seg_mute: Vec<Span<'static>> = vec![Span::styled("m", mid), Span::styled("ute", dim)];
    let seg_mute_w: usize = 4;

    let seg_volume: Vec<Span<'static>> =
        vec![Span::styled("+ - ", mid), Span::styled("volume", dim)];
    let seg_volume_w: usize = 10;

    let seg_type: Vec<Span<'static>> = vec![Span::styled("t", mid), Span::styled("ype", dim)];
    let seg_type_w: usize = 4;

    let full_w = seg_stations_w + sep_w + seg_mute_w + sep_w + seg_volume_w + sep_w + seg_type_w;
    let xl_w = seg_stations_w + sep_w + seg_mute_w + sep_w + seg_volume_w;
    let mid_w = seg_stations_w + sep_w + seg_mute_w;

    let (spans, used) = if avail >= full_w {
        let mut s = seg_stations;
        s.extend(sep.clone());
        s.extend(seg_mute);
        s.extend(sep.clone());
        s.extend(seg_volume);
        s.extend(sep);
        s.extend(seg_type);
        (s, full_w)
    } else if avail >= xl_w {
        let mut s = seg_stations;
        s.extend(sep.clone());
        s.extend(seg_mute);
        s.extend(sep);
        s.extend(seg_volume);
        (s, xl_w)
    } else if avail >= mid_w {
        let mut s = seg_stations;
        s.extend(sep);
        s.extend(seg_mute);
        (s, mid_w)
    } else if avail >= seg_stations_w {
        (seg_stations, seg_stations_w)
    } else {
        return;
    };

    let used_u16 = u16::try_from(used).unwrap_or(0);
    let x = chord_row
        .x
        .saturating_add(chord_row.width)
        .saturating_sub(used_u16)
        .saturating_sub(CHORD_INSET);
    let area = single_row_rect(x, chord_row.y, used_u16);
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
