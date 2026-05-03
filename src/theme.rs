// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Theme layer — design tokens, palette, and glyph set.
//!
//! This file is the **only** location in the crate that imports
//! `ratatui::style::Color` (colour-token canon). Every other module references
//! colours through [`PaletteToken`] (defined in `station.rs`) and the
//! resolution helpers in this module. The invariant is enforced at compile
//! time by the `clippy::disallowed_methods` lint configured in `clippy.toml`.

use ratatui::style::{Color, Modifier, Style};

use crate::station::{PaletteToken, Station};

/// Brand red — the project's identity colour. RGB sourced from the v1 mockup.
/// 8-colour fallback maps to `Color::Red`. Reachable from
/// `resolve(PaletteToken::BrandRed)` for the `ebsm` station accent.
const BRAND_RED_RGB: (u8, u8, u8) = (214, 52, 43);

/// Off-white "neutral" foreground used by primary informational text
/// (song title, song-elapsed counter). Hex `#D5D5D5` — v1.x mockup canon.
const TEXT_NEUTRAL_RGB: (u8, u8, u8) = (0xD5, 0xD5, 0xD5);

/// Dark-gray foreground used by de-emphasised text (timestamp head /
/// timezone tail, separators, chord label tails). Hex `#343436` — v1.x
/// mockup canon, replaces the `Modifier::DIM` terminal-defined shade.
const TEXT_DIM_RGB: (u8, u8, u8) = (0x34, 0x34, 0x36);

/// Mid-gray foreground for chord key glyphs (`← →`, `m`, `+`, `-`) — clearly
/// brighter than `TEXT_DIM_RGB` so the keystroke pops against the surrounding
/// label tail, yet a long way short of `TEXT_NEUTRAL_RGB` so it doesn't
/// compete with the song-line text. Hex `#7A7A7C`.
const TEXT_MID_RGB: (u8, u8, u8) = (0x7A, 0x7A, 0x7C);

/// Theme tokens consumed by `ui`.
///
/// The struct itself is stateless data — every method is `&self` and
/// produces fresh `Style` values per call. There is exactly one instance
/// of `Theme` in the running app, owned by `App` in `ui.rs`.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// Glyph set used by every render function.
    pub glyphs: GlyphSet,
    /// True when the terminal advertises 24-bit truecolor support; false
    /// triggers the 8-colour fallback path in colour-emitting helpers.
    pub truecolor: bool,
}

impl Theme {
    /// Build a theme matching the running terminal's colour capabilities.
    /// Detection is best-effort via the `COLORTERM` env var, the canonical
    /// signal for truecolor advertised by modern terminals.
    #[must_use]
    pub fn detect() -> Self {
        let truecolor = std::env::var("COLORTERM")
            .is_ok_and(|v| v.contains("truecolor") || v.contains("24bit"));
        Self {
            glyphs: GlyphSet::default(),
            truecolor,
        }
    }

    /// Default "dark gray" style for de-emphasised UI tokens — date
    /// head + timezone tail of the floating header, separators, and the
    /// chord chip label tails. Truecolor uses the canonical hex `#343436`
    /// so the shade matches the v1.x mockup pixel-for-pixel; the 8-colour
    /// fallback degrades to `Modifier::DIM` on terminal-default fg.
    #[must_use]
    pub fn dim_style(&self) -> Style {
        if self.truecolor {
            Style::default().fg(Color::Rgb(TEXT_DIM_RGB.0, TEXT_DIM_RGB.1, TEXT_DIM_RGB.2))
        } else {
            Style::default().add_modifier(Modifier::DIM)
        }
    }

    /// Mid-gray foreground style — sits between `dim_style` and
    /// `text_neutral_style`. Drives the chord-key highlight in the chord
    /// chip (`← →`, `m`, `+`, `-`): bright enough to pop against the dim
    /// label tail, dark enough to stay subordinate to the neutral content
    /// layer. Truecolor uses `#7A7A7C`; the 8-colour fallback degrades to
    /// `Color::Gray`.
    #[must_use]
    pub fn text_mid_style(&self) -> Style {
        if self.truecolor {
            Style::default().fg(Color::Rgb(TEXT_MID_RGB.0, TEXT_MID_RGB.1, TEXT_MID_RGB.2))
        } else {
            Style::default().fg(Color::Gray)
        }
    }

    /// Off-white "neutral" foreground style for primary informational
    /// text that is not the per-station accent. Used by:
    ///
    /// - the artist name on the now-playing line,
    /// - the connection-status body inside `[ STATION // status ]`.
    ///
    /// Bright enough to read as the dominant content layer, distinct
    /// from `dim_style` (dark gray `#343436`) and from `accent_*`
    /// (per-station brand). Truecolor uses canonical `#D5D5D5`; the
    /// 8-colour fallback degrades to `Color::White`.
    #[must_use]
    pub fn text_neutral_style(&self) -> Style {
        if self.truecolor {
            Style::default().fg(Color::Rgb(
                TEXT_NEUTRAL_RGB.0,
                TEXT_NEUTRAL_RGB.1,
                TEXT_NEUTRAL_RGB.2,
            ))
        } else {
            Style::default().fg(Color::White)
        }
    }

    /// Brand-red foreground colour. Truecolor when supported, 8-colour
    /// fallback otherwise. Reachable via `resolve(PaletteToken::BrandRed)`
    /// for the `ebsm` station accent.
    #[must_use]
    pub fn brand_red(&self) -> Color {
        if self.truecolor {
            Color::Rgb(BRAND_RED_RGB.0, BRAND_RED_RGB.1, BRAND_RED_RGB.2)
        } else {
            Color::Red
        }
    }

    /// Scale a colour toward black by `alpha` (0.0 = hidden, 1.0 = full).
    /// Truecolor path multiplies each RGB channel; 8-colour fallback uses a
    /// hard cutoff at 0.5 since named colours have no programmable shade.
    /// `Color::Reset` and other non-RGB sentinels pass through unchanged at
    /// alpha ≥ 0.5 and collapse to `Color::Reset` below.
    #[must_use]
    pub fn fade_color(&self, color: Color, alpha: f32) -> Color {
        let a = alpha.clamp(0.0, 1.0);
        if let Color::Rgb(r, g, b) = color {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "alpha-scaled u8 channel, value already in [0,255]"
            )]
            return Color::Rgb(
                (f32::from(r) * a) as u8,
                (f32::from(g) * a) as u8,
                (f32::from(b) * a) as u8,
            );
        }
        if a < 0.5 { Color::Reset } else { color }
    }

    /// Resolve a [`PaletteToken`] to a concrete `Color`. All accent callers
    /// reach colour via [`Self::accent_for`] which delegates here.
    #[must_use]
    pub fn resolve(&self, token: PaletteToken) -> Color {
        match token {
            PaletteToken::BrandMagenta if self.truecolor => Color::Rgb(232, 80, 196),
            PaletteToken::BrandMagenta => Color::Magenta,
            PaletteToken::BrandCyan if self.truecolor => Color::Rgb(72, 200, 230),
            PaletteToken::BrandCyan => Color::Cyan,
            PaletteToken::BrandBlue if self.truecolor => Color::Rgb(60, 110, 220),
            PaletteToken::BrandBlue => Color::Blue,
            PaletteToken::BrandLightBlue if self.truecolor => Color::Rgb(120, 180, 235),
            PaletteToken::BrandLightBlue => Color::LightBlue,
            PaletteToken::BrandRed => self.brand_red(),
            PaletteToken::BrandDarkGray if self.truecolor => Color::Rgb(120, 120, 130),
            PaletteToken::BrandDarkGray => Color::DarkGray,
            PaletteToken::BrandYellow if self.truecolor => Color::Rgb(232, 196, 80),
            PaletteToken::BrandYellow => Color::Yellow,
            PaletteToken::Warm1 if self.truecolor => Color::Rgb(232, 124, 60),
            PaletteToken::Warm1 => Color::LightRed,
            PaletteToken::Warm2 if self.truecolor => Color::Rgb(180, 100, 80),
            PaletteToken::Warm2 => Color::Red,
        }
    }

    /// Resolve the active station's accent token to a concrete colour.
    /// Convenience wrapper used by render functions that already hold a
    /// `&Station` reference.
    #[must_use]
    pub fn accent_for(&self, station: &Station) -> Color {
        self.resolve(station.accent)
    }

    /// Station accent bold style at full visibility. The entire UI breathes
    /// with the station — the v1.x mockup contract.
    #[must_use]
    pub fn accent_style(&self, station: &Station) -> Style {
        Style::default()
            .fg(self.accent_for(station))
            .add_modifier(Modifier::BOLD)
    }

    /// Dim variant of the station accent. `factor` is the linear scale
    /// toward black (1.0 = full accent, 0.0 = black). Used for the
    /// empty-tick body of the volume chip.
    #[must_use]
    pub fn accent_dim(&self, station: &Station, factor: f32) -> Color {
        self.fade_color(self.accent_for(station), factor)
    }

    /// Foreground style for the station-accent dim variant — drives the
    /// inactive ticks of the volume mini-bar.
    #[must_use]
    pub fn accent_dim_style(&self, station: &Station, factor: f32) -> Style {
        Style::default().fg(self.accent_dim(station, factor))
    }
}

/// Centralised glyph table used by every renderer.
///
/// All Unicode literals consumed by the UI live here. Widgets MUST NOT
/// scatter `\u{...}` literals.
#[derive(Debug, Clone, Copy)]
pub struct GlyphSet {
    /// Single thin vertical bar used by the v1.0.1 volume pill — every
    /// cell of the bar series uses the SAME glyph; fill vs empty is
    /// distinguished by colour, not by glyph swap.
    pub volume_bar: &'static str,
    /// Separator between station name, title, and artist on the now-playing
    /// line.
    pub now_separator: &'static str,
    /// Tuning placeholder shown when no metadata has arrived yet.
    pub tuning_placeholder: &'static str,
    /// Mute label rendered in place of the volume number.
    pub mute_label: &'static str,
}

impl Default for GlyphSet {
    fn default() -> Self {
        Self {
            volume_bar: "|",
            now_separator: "/",
            tuning_placeholder: "tuning…",
            mute_label: "MUTE",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Theme;
    use crate::station::{DEFAULT_STATIONS, PaletteToken};

    #[test]
    fn detect_returns_a_theme() {
        let _ = Theme::detect();
    }

    /// Truecolor flag flips the rendering path between `Color::Rgb` and the
    /// 8-colour fallback. This guards both branches.
    #[test]
    fn fallback_resolves_to_named_colors_without_truecolor() {
        let theme = Theme {
            glyphs: super::GlyphSet::default(),
            truecolor: false,
        };
        assert_eq!(theme.brand_red(), ratatui::style::Color::Red);
        assert_eq!(
            theme.resolve(PaletteToken::BrandCyan),
            ratatui::style::Color::Cyan
        );
        assert_eq!(
            theme.resolve(PaletteToken::Warm1),
            ratatui::style::Color::LightRed
        );
    }

    #[test]
    fn truecolor_resolves_to_rgb() {
        let theme = Theme {
            glyphs: super::GlyphSet::default(),
            truecolor: true,
        };
        match theme.brand_red() {
            ratatui::style::Color::Rgb(214, 52, 43) => (),
            other => panic!("expected brand red RGB, got {other:?}"),
        }
    }

    /// Every station in the registry resolves to a concrete colour through
    /// the theme — guards against an unhandled `PaletteToken` variant being
    /// added without a `resolve` arm.
    #[test]
    fn every_station_accent_resolves() {
        let theme = Theme {
            glyphs: super::GlyphSet::default(),
            truecolor: true,
        };
        for station in DEFAULT_STATIONS {
            let _ = theme.accent_for(station);
        }
    }
}
