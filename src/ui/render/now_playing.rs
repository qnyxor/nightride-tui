// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Borderless now-playing line — `[STATION//Artist/Title]` with the
//! brackets + station + artist + `//` + `/` separators painted in the
//! active station accent and the title in the neutral light tone.
//! The retired rounded border has been replaced by the literal
//! brackets, which travel with the marquee. Per-character style
//! preservation across the rune-correct marquee scroll.
//!
//! Visibility is gated on `App::panel_visibility`: the line only renders
//! while audio is actually streaming, fading in/out around station switches.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::super::app::App;
use super::{fade_style_fg, single_row_rect};

/// Default capacity for the styled-line buffer. ~64 chars covers the
/// median song-line length in one allocation.
const STYLED_LINE_DEFAULT_CAP: usize = 64;

/// Width reserved for the update notice `- new version -` (17 chars)
/// plus one trailing space gap before the marquee starts.
const UPDATE_NOTICE_WIDTH: u16 = 18;

/// The update-available notice text painted at the left edge of the
/// now-playing row when a newer release exists.
const UPDATE_NOTICE_TEXT: &str = "- new version -";

/// Minimum terminal width required to render the update notice alongside
/// any meaningful marquee content. Narrower terminals suppress the notice
/// entirely to avoid crowding.
const UPDATE_NOTICE_MIN_WIDTH: u16 = 30;

/// Render the now-playing line.
pub(super) fn render(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    if app.panel_hidden() {
        return;
    }
    let alpha = app.panel_visibility;

    // Borderless: the now-playing line sits on the song-band's TOP
    // row. The header band (volume + timers) drops to the centre row
    // and the spectrum claims the bottom row — the marquee leads the
    // visual hierarchy from the top of the screen.

    // --- Update-available notice ---
    // When a newer release is available AND the terminal is wide enough,
    // render `- new version -` at the far-left of row 0 in the station
    // accent colour, then shrink the marquee rect so the two never overlap.
    let (marquee_x_offset, marquee_width) =
        if app.update_available.is_some() && area.width >= UPDATE_NOTICE_MIN_WIDTH {
            let accent = app.theme.accent_style(app.displayed_station);
            let faded = fade_style_fg(&app.theme, accent, alpha);
            let notice_row = single_row_rect(area.x, area.y, UPDATE_NOTICE_WIDTH.min(area.width));
            let notice = Paragraph::new(Line::from(Span::styled(UPDATE_NOTICE_TEXT, faded)));
            frame.render_widget(notice, notice_row);
            (
                UPDATE_NOTICE_WIDTH,
                area.width.saturating_sub(UPDATE_NOTICE_WIDTH),
            )
        } else {
            (0, area.width)
        };

    // Build the per-character styled view of the now-playing line.
    // We carry a Style alongside every char so the marquee rotation
    // can preserve segment colours through wraparound.
    let segments = app.now_playing_segments();
    let mut styled: Vec<(char, Style)> = Vec::with_capacity(STYLED_LINE_DEFAULT_CAP);
    for (text, style) in segments {
        for ch in text.chars() {
            styled.push((ch, style));
        }
    }

    let inner_w = usize::from(marquee_width);
    let visible: Vec<(char, Style)> = if styled.len() <= inner_w {
        styled
    } else {
        // Rune-correct rotation: rotate the styled vector by
        // `marquee_offset`, then take the first `marquee_width` cells.
        let offset = app.marquee_offset.min(styled.len());
        styled[offset..]
            .iter()
            .chain(styled[..offset].iter())
            .take(inner_w)
            .copied()
            .collect()
    };

    // Coalesce consecutive same-style chars into single spans, then
    // attenuate every fg by `alpha` so the panel breathes through the
    // visibility gate.
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut cur_buf = String::new();
    let mut cur_style: Option<Style> = None;
    for (ch, style) in visible {
        let faded = fade_style_fg(&app.theme, style, alpha);
        if cur_style != Some(faded) {
            if let Some(s) = cur_style {
                spans.push(Span::styled(std::mem::take(&mut cur_buf), s));
            }
            cur_style = Some(faded);
        }
        cur_buf.push(ch);
    }
    if let Some(s) = cur_style {
        spans.push(Span::styled(cur_buf, s));
    }

    // Marquee is rendered at `area.x + marquee_x_offset` to avoid
    // overlapping the update notice when it is present.
    let marquee_row = single_row_rect(
        area.x.saturating_add(marquee_x_offset),
        area.y,
        marquee_width,
    );
    let title = Paragraph::new(Line::from(spans)).wrap(Wrap { trim: false });
    frame.render_widget(title, marquee_row);
}
