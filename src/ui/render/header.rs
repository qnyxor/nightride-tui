// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Floating header band on the song-band's top row. Four fields,
//! separated by 2 spaces, painted left-to-right starting 2 cells in
//! from the panel left edge:
//!
//! 1. Volume percentage — `mid` tone when audible (`50 %`), full
//!    accent when muted (`MUTE`). Replaces the retired volume pill on
//!    row 3 so the spectrum can claim the entire bottom row.
//! 2. Live wall-clock D-timestamp — head + tail dim, `T{HHMMSS}`
//!    fragment in the active station accent (dimmed).
//! 3. App uptime since launch (`H:MM:SS`, monotonic). Accent (dimmed).
//! 4. Current song elapsed (`H:MM:SS`). Neutral mid-gray. Hidden
//!    until the first `Streaming` event.
//!
//! Visibility gated on `App::panel_visibility` so the band fades
//! in/out together with the rest of the now-playing group.

use std::time::Duration;

use chrono::{DateTime, Local};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::super::app::App;
use super::{fade_style_fg, single_row_rect, vertical_center_y};

/// Linear scale toward black applied to the per-station accent before
/// the visibility-gate fade. Drops the timer trio one step below the
/// now-playing line + spectrum so the counters read as ambient chrome
/// instead of competing with the song content.
const TIMER_ACCENT_DIM_FACTOR: f32 = 0.6;

/// Dim factor applied to the empty-tail bars of the volume pill.
/// Keeps them tinted in-key with the surface but reads as background.
const EMPTY_BAR_DIM: f32 = 0.25;

/// Total bar cells in the pill. The label is a fixed 4-cell envelope
/// (`5  %` / `10 %` / `100%` / `MUTE`) and the bar series is fixed so
/// the timer trio that follows stays at a stable x across volume
/// changes.
const VOLUME_BAR_CELLS: usize = 10;

/// Build the styled spans for the volume pill: `<label> <filled
/// bars><empty bars>`. `alpha` is the panel-visibility gate (0..=1)
/// applied to every foreground colour so the pill fades in lockstep
/// with the rest of the now-playing group. Returns an empty vec when
/// the panel is hidden.
fn volume_spans(app: &App, alpha: f32) -> Vec<Span<'static>> {
    if app.panel_hidden() {
        return Vec::new();
    }
    let glyphs = &app.theme.glyphs;
    // Fixed-width 4-cell label: number left-aligned in a 3-char field
    // then `%`. `5  %` / `10 %` / `100%` / `MUTE`.
    let label = match app.volume {
        0 => glyphs.mute_label.to_string(),
        v => format!("{v:<3}%"),
    };

    let filled = (usize::from(app.volume) * VOLUME_BAR_CELLS) / 100;
    let empty = VOLUME_BAR_CELLS.saturating_sub(filled);
    let bar_glyph = glyphs.volume_bar;
    let filled_bar: String = bar_glyph.repeat(filled);
    let empty_bar: String = bar_glyph.repeat(empty);

    let label_style_base: Style = if app.volume == 0 || app.volume == 100 {
        // Both extremes promote the label to the full station accent
        // — mute so a silent stream is unmissable, max so the user
        // notices the volume is pinned at the ceiling. Mid-band
        // values stay in the ambient mid-gray.
        app.theme.accent_style(app.displayed_station)
    } else {
        // Match the chord-key tone (mid-gray) so the volume label
        // reads as ambient chrome, not as a primary content layer.
        app.theme.text_mid_style()
    };
    let filled_style_base = app.theme.accent_style(app.displayed_station);
    let empty_style_base = app
        .theme
        .accent_dim_style(app.displayed_station, EMPTY_BAR_DIM);
    let label_style = fade_style_fg(&app.theme, label_style_base, alpha);
    let filled_style = fade_style_fg(&app.theme, filled_style_base, alpha);
    let empty_style = fade_style_fg(&app.theme, empty_style_base, alpha);

    vec![
        Span::styled(label, label_style),
        Span::styled(filled_bar, filled_style),
        Span::styled(empty_bar, empty_style),
    ]
}

/// Inset from the song-band's left edge for the floating header trio.
/// Zero: flush-left inside the `SIDE_PADDING` envelope.
const HEADER_INSET: u16 = 0;

/// Minimum tail margin: how many cells past the timestamp + uptime +
/// song-time block must remain inside the panel for the header to
/// render at all.
const HEADER_MIN_TAIL: u16 = 4;

/// Render the floating header band.
pub(super) fn render(frame: &mut ratatui::Frame, panel_area: Rect, app: &App) {
    if panel_area.width < 8 || panel_area.height < 1 {
        return;
    }
    if app.panel_hidden() {
        return;
    }
    let alpha = app.panel_visibility;
    let now = Local::now();
    let live_full = format_d_timestamp(now);
    let live_red = format_d_timestamp_red_fragment(now);
    let uptime = format_elapsed_hms(app.app_launched_at.elapsed());
    let song_elapsed = app
        .stream_started_at
        .map(|s| {
            let secs = (now - s).num_seconds().max(0);
            #[allow(
                clippy::cast_sign_loss,
                reason = "max(0) above guarantees non-negative seconds"
            )]
            format_elapsed_hms(Duration::from_secs(secs as u64))
        })
        .unwrap_or_default();

    // Apply the panel visibility alpha to every fg in the band so the
    // header fades in lockstep with the song panel below it. The band
    // uses a NON-bold, dimmed accent so timer digits read as ambient
    // chrome — bold or full-saturation accent would push them above
    // the song line below in the visual hierarchy.
    let dim = fade_style_fg(&app.theme, app.theme.dim_style(), alpha);
    // Song-elapsed reads in the neutral mid-gray tone so every neutral
    // field in the band sits at the same attenuated weight as ambient
    // chrome.
    let neutral = fade_style_fg(&app.theme, app.theme.text_mid_style(), alpha);
    let accent_thin = app
        .theme
        .accent_dim_style(app.displayed_station, TIMER_ACCENT_DIM_FACTOR);
    let accent = fade_style_fg(&app.theme, accent_thin, alpha);

    // Field 1: volume pill (label + filled / empty bar split). Same x
    // as the now-playing `[` on row 1 below — the pill anchors the
    // band's left edge. One space separates the bar tail from the
    // wallclock that follows.
    let mut spans: Vec<Span<'static>> = volume_spans(app, alpha);
    if !spans.is_empty() {
        spans.push(Span::raw(" "));
    }
    spans.extend(timestamp_red_fragment_spans(
        &live_full, &live_red, dim, accent,
    ));
    if !uptime.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(uptime, accent));
    }
    // Song-elapsed shows only while audio is actually streaming; during
    // tuning / reconnecting / error / idle the field collapses so the
    // band reads volume / wallclock / uptime without a stale counter.
    let is_streaming = matches!(
        app.connection,
        crate::audio::ConnectionState::Streaming { .. }
    );
    if is_streaming && !song_elapsed.is_empty() {
        spans.push(Span::raw("  "));
        // Song elapsed reads in the neutral mid-gray tone — distinct
        // from the dim date head/tz and from the accent uptime, so the
        // band resolves left-to-right as volume / wallclock / uptime /
        // song time.
        spans.push(Span::styled(song_elapsed, neutral));
    }

    let need_minimum = u16::try_from(live_red.len()).unwrap_or(7);
    if panel_area.width < need_minimum + HEADER_MIN_TAIL {
        return;
    }

    // Float on the song-band's CENTRE row (y = vertical_center_y),
    // sandwiched between the now-playing marquee on row 0 and the
    // spectrum baseline on row 2. Flush-left inside the side-padding
    // envelope.
    let header_x = panel_area.x.saturating_add(HEADER_INSET);
    let header_width = panel_area
        .width
        .saturating_sub(HEADER_INSET.saturating_mul(2));
    let header_area = single_row_rect(header_x, vertical_center_y(panel_area), header_width);

    let para = Paragraph::new(Line::from(spans));
    frame.render_widget(para, header_area);
}

/// Split `full` into three spans around `red_frag` so the fragment can
/// carry its own style. Returns the original string as a single dim
/// span when the fragment is absent (defensive fallback for
/// malformed input).
fn timestamp_red_fragment_spans(
    full: &str,
    red_frag: &str,
    dim: Style,
    red: Style,
) -> Vec<Span<'static>> {
    if let Some(pos) = full.find(red_frag) {
        let (head, tail) = full.split_at(pos);
        let after = &tail[red_frag.len()..];
        return vec![
            Span::styled(head.to_string(), dim),
            Span::styled(red_frag.to_string(), red),
            Span::styled(after.to_string(), dim),
        ];
    }
    vec![Span::styled(full.to_string(), dim)]
}

/// Format a `Duration` as `H:MM:SS` with hours unpadded (so a 948-hour
/// uptime renders as `948:22:29`, not `00948:22:29`).
#[must_use]
pub fn format_elapsed_hms(d: Duration) -> String {
    let total = d.as_secs();
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    format!("{h}:{m:02}:{s:02}")
}

/// Format a chrono `DateTime<Local>` as `D{YYYYMMDDTHHmmss}Z{±HHMM}`.
///
/// # Examples
///
/// ```
/// use chrono::TimeZone;
/// use nightride_tui::ui::format_d_timestamp;
///
/// let dt = chrono::Local
///     .with_ymd_and_hms(2026, 4, 28, 9, 21, 7)
///     .single()
///     .expect("valid datetime");
/// let s = format_d_timestamp(dt);
/// assert_eq!(s.len(), 22);
/// assert!(s.starts_with("D20260428T092107Z"));
/// ```
#[must_use]
pub fn format_d_timestamp(dt: DateTime<Local>) -> String {
    let main = dt.format("D%Y%m%dT%H%M%S");
    let tz_full = dt.format("%z").to_string(); // ±HHMM
    format!("{main}Z{tz_full}")
}

/// Extract the `T-HHMMSS` fragment from a D-format timestamp.
#[must_use]
pub fn format_d_timestamp_red_fragment(dt: DateTime<Local>) -> String {
    dt.format("T%H%M%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::{format_d_timestamp, format_d_timestamp_red_fragment};
    use chrono::TimeZone;

    #[test]
    fn timestamp_format_shape() {
        let dt = chrono::Local
            .with_ymd_and_hms(2026, 4, 28, 9, 21, 7)
            .single()
            .expect("valid datetime");
        let s = format_d_timestamp(dt);
        // Shape: `D{8}T{6}Z{±4}` = 22 chars (e.g. D20260428T092107Z+0200).
        assert_eq!(s.len(), 22);
        assert!(s.starts_with("D20260428T092107Z"));
        let red = format_d_timestamp_red_fragment(dt);
        assert_eq!(red, "T092107");
    }
}
