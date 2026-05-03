// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Braille-canvas smooth-curve spectrum.
//!
//! Braille-resolution canvas that interpolates RMS amplitudes between
//! columns with a Catmull-Rom spline, then fills the area under the
//! curve. Each terminal cell holds 2×4 Braille sub-pixels, so a 3-row
//! band gives 12 sub-rows of vertical resolution — enough to draw a
//! smooth silhouette of the music's energy envelope.

use ratatui::layout::Rect;
use ratatui::symbols::Marker;
use ratatui::widgets::canvas::{Canvas, Points};

use super::super::app::App;

/// Sub-pixel rows trimmed off the TOP of the Braille spectrum band.
/// The Braille canvas exposes 4 vertical sub-pixels per cell, so a
/// 3-row band has 12 sub-y positions; trimming N from the top caps
/// the maximum amplitude reach to `(12 - N)` sub-pixels — leaves a
/// breathing strip between the loudest peak and the row above.
pub(super) const SPECTRUM_TOP_CROP_SUB: usize = 4;

/// Perceptual exponent applied to RMS amps before the spline. Music
/// RMS sits in 0.05-0.3, which a linear mapping crushes against the
/// bottom; `pow(0.4)` keeps the spline operating in the same
/// perceptual space the eye reads.
const SPECTRUM_PERCEPTUAL_EXPONENT: f32 = 0.4;

/// Overall intensity cap on the spectrum colour gradient. Caps the
/// loudest visible row at 0.5× alpha so the spectrum stays subordinate
/// to the song-line.
const SPECTRUM_INTENSITY_CAP: f32 = 0.5;

/// Bottom-row factor in the per-row gradient. Bottom row reads at
/// `INTENSITY_CAP × BOTTOM_ROW_FACTOR` alpha.
const SPECTRUM_BOTTOM_ROW_FACTOR: f32 = 0.3;

/// Range of the per-row gradient: top row reads at
/// `INTENSITY_CAP × (BOTTOM_ROW_FACTOR + GRADIENT_RANGE)` alpha.
const SPECTRUM_GRADIENT_RANGE: f32 = 0.7;

/// Render the Braille spectrum.
#[allow(
    clippy::too_many_lines,
    reason = "canvas paint closure captures usable_top, row_colors, and the \
              perceptual array as one unit so the per-sub-pixel loop stays \
              cache-coherent; splitting adds coupling without clarifying data flow"
)]
pub(super) fn render(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    if app.panel_hidden() || area.width == 0 || area.height == 0 {
        return;
    }
    let alpha = app.panel_visibility;
    let cols = usize::from(area.width).max(1);
    let zero_fallback;
    let amps: &[f32] = if let Some(v) = app.last_amp.as_ref() {
        v.as_slice()
    } else {
        zero_fallback = vec![0.0_f32; cols];
        &zero_fallback
    };
    let accent_color = app.theme.accent_for(app.displayed_station);

    // Pre-bake the perceptual curve.
    let perceptual: Vec<f32> = amps
        .iter()
        .map(|a| a.clamp(0.0, 1.0).powf(SPECTRUM_PERCEPTUAL_EXPONENT))
        .collect();

    // Canvas coordinate system — width in cells × 2 (Braille horiz
    // sub-pixels), height in cells × 4 (Braille vertical sub-pixels).
    // Y origin is at the BOTTOM with y_bounds[0]=0; filling from
    // the TOP downward draws the silhouette as a stalactite hanging
    // from the area's ceiling.
    let max_sub_x = f64::from(area.width) * 2.0;
    let max_sub_y = f64::from(area.height) * 4.0;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "max_sub_y = area.height * 4; area.height ≤ frame height — fits usize"
    )]
    let max_sub_y_i = max_sub_y as usize;
    let usable_top = max_sub_y_i.saturating_sub(SPECTRUM_TOP_CROP_SUB);

    // Per-vertical-row gradient: each Braille sub-row gets its own
    // colour — top row at `INTENSITY_CAP × (BOTTOM_ROW_FACTOR +
    // GRADIENT_RANGE)`, bottom row at `INTENSITY_CAP ×
    // BOTTOM_ROW_FACTOR`. Gives the spectrum visual depth without
    // letting it compete with the song line for attention.
    let mut row_colors: Vec<ratatui::style::Color> = Vec::with_capacity(usable_top.max(1));
    if usable_top == 0 {
        row_colors.push(
            app.theme
                .fade_color(accent_color, alpha * SPECTRUM_INTENSITY_CAP),
        );
    } else {
        for sy in 0..usable_top {
            #[allow(
                clippy::cast_precision_loss,
                reason = "sy ≤ usable_top ≤ frame_h*4 ≈ 240; well below 2^24 f32 precision boundary"
            )]
            let pos = (sy as f32) / ((usable_top - 1).max(1) as f32);
            // pos = 0 at the bottom row, 1 at the top row.
            let factor = SPECTRUM_BOTTOM_ROW_FACTOR + SPECTRUM_GRADIENT_RANGE * pos;
            row_colors.push(
                app.theme
                    .fade_color(accent_color, alpha * SPECTRUM_INTENSITY_CAP * factor),
            );
        }
    }

    let canvas = Canvas::default()
        .marker(Marker::Braille)
        .x_bounds([0.0, max_sub_x])
        .y_bounds([0.0, max_sub_y])
        .paint(move |ctx| {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "max_sub_x = area.width * 2; area.width ≤ frame width — fits usize"
            )]
            let n_sub_x = (max_sub_x as usize).max(1);
            if usable_top == 0 {
                return;
            }
            // One coord vec per sub-row; each row carries its own
            // colour from `row_colors`.
            let mut rows: Vec<Vec<(f64, f64)>> = (0..usable_top).map(|_| Vec::new()).collect();
            #[allow(
                clippy::cast_precision_loss,
                reason = "usable_top ≤ max_sub_y ≈ 240; precision loss only > 2^53"
            )]
            let usable_top_f = usable_top as f64;
            for sx in 0..n_sub_x {
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "sub-pixel index ≤ 2× cell width; precision loss only > 2^53"
                )]
                let frac = sx as f64 / 2.0;
                let level = catmull_rom_at(&perceptual, frac).clamp(0.0, 1.0);
                let peak_sub_f = f64::from(level) * usable_top_f;
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "peak_sub_f ∈ [0, usable_top]; usable_top ≤ 240 — fits usize"
                )]
                let peak_sub = (peak_sub_f.round() as usize).min(usable_top);
                if peak_sub == 0 {
                    continue;
                }
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "sub-pixel index ≤ 2× cell width; precision loss only > 2^53"
                )]
                let x = sx as f64;
                // Top-down fill anchored at `usable_top - 1`, so the
                // top `SPECTRUM_TOP_CROP_SUB` rows always stay empty
                // and the wave hangs from `usable_top - 1` downward.
                for k in 0..peak_sub {
                    let sy = usable_top - 1 - k;
                    #[allow(
                        clippy::cast_precision_loss,
                        reason = "sy ≤ max_sub_y ≈ 240; precision loss only > 2^53"
                    )]
                    let y = sy as f64;
                    rows[sy].push((x, y));
                }
            }
            for (sy, coords) in rows.iter().enumerate() {
                if coords.is_empty() {
                    continue;
                }
                ctx.draw(&Points {
                    coords,
                    color: row_colors[sy],
                });
            }
        });
    frame.render_widget(canvas, area);
}

/// Catmull-Rom interpolation across `samples` at fractional index
/// `frac`. Smooth C¹-continuous spline through the sample points;
/// degenerates to a straight line when consecutive samples are
/// equal. Out-of-range indices clamp to the nearest endpoint so the
/// curve stays anchored at the borders of the spectrum band.
fn catmull_rom_at(samples: &[f32], frac: f64) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    #[allow(
        clippy::cast_possible_wrap,
        reason = "samples.len() bounded by frame width — never approaches i64::MAX"
    )]
    let last = samples.len() as i64 - 1;
    #[allow(
        clippy::cast_possible_truncation,
        reason = "frac ∈ [0, samples.len()] — fits i64 trivially"
    )]
    let i = frac.floor() as i64;
    #[allow(
        clippy::cast_possible_truncation,
        reason = "fractional residual in [0,1] — narrow to f32 for the spline math"
    )]
    let t = (frac - frac.floor()) as f32;
    let pick = |idx: i64| -> f32 {
        let clamped = idx.clamp(0, last);
        #[allow(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "clamp(0, last) keeps idx non-negative and within samples.len()"
        )]
        let u = clamped as usize;
        samples[u]
    };
    let p0 = pick(i - 1);
    let p1 = pick(i);
    let p2 = pick(i + 1);
    let p3 = pick(i + 2);
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}
