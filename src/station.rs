// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Station registry. Canonical list of nightride.fm streams shipped with
//! the binary.
//!
//! v0 keeps a flat const array; a `trait Station` is only introduced when a
//! second metadata mechanism or a second radio service appears
//! (no preemptive traits).
//!
//! Per the colour-token canon this module does NOT reference
//! `ratatui::style::Color` directly; each station carries a [`PaletteToken`]
//! and `theme.rs` is the only place that resolves tokens to concrete colours.

use url::Url;

use crate::NightrideError;

/// Theme token for the per-station accent. Resolved to a concrete
/// `ratatui::style::Color` only inside `theme.rs` (colour-token canon).
///
/// The `Brand*` variants are the curated palette; `Warm1` / `Warm2` exist for
/// the absorbed rekt + rektory channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaletteToken {
    /// Magenta-leaning brand for the flagship `nightride` channel.
    BrandMagenta,
    /// Cyan for chillsynth.
    BrandCyan,
    /// Blue for datawave.
    BrandBlue,
    /// Lighter blue for spacesynth.
    BrandLightBlue,
    /// Brand red — also the global accent for emphasised UI tokens.
    BrandRed,
    /// Dark grey for horrorsynth.
    BrandDarkGray,
    /// Yellow for ebsm.
    BrandYellow,
    /// Warm tone #1 (rekt — assigned in `theme.rs`).
    Warm1,
    /// Warm tone #2 (rektory — assigned in `theme.rs`).
    Warm2,
}

/// Static station entry. Each Nightride FM channel exposes two protocols:
///
/// * MP3 direct — `https://stream.nightride.fm/{slug}.mp3` (Icecast, 320 kbps).
/// * HLS — `https://stream.nightride.fm:8443/{slug}/{slug}.m3u8` (AAC adaptive,
///   deferred to a future release).
///
/// v2 MVP plays MP3 only; the `stream_hls` field is kept so a future toggle
/// can land without schema migration.
#[derive(Debug, Clone, Copy)]
pub struct Station {
    /// Stable slug used for lookups, log lines, and URL pattern.
    pub slug: &'static str,
    /// Display name shown on the now-playing header.
    pub display_name: &'static str,
    /// Genre tag rendered under the station name.
    pub genre: &'static str,
    /// Icecast MP3 endpoint opened by the audio pipeline (v2 MVP default).
    pub stream_mp3: &'static str,
    /// HLS master manifest endpoint, port 8443 (deferred to a future release).
    pub stream_hls: &'static str,
    /// Theme token resolved to an accent colour by `theme::accent_for`.
    pub accent: PaletteToken,
}

/// Canonical default stations shipped with the binary.
///
/// The original 7 Nightride channels plus `rekt` + `rektory` = 9 total.
/// The actual Nightride.fm public registry caps at 9; earlier drafts
/// cited "11 stations" which was a miscount.
pub const DEFAULT_STATIONS: &[Station] = &[
    Station {
        slug: "nightride",
        display_name: "NIGHTRIDE FM",
        genre: "Synthwave Retrowave",
        stream_mp3: "https://stream.nightride.fm/nightride.mp3",
        stream_hls: "https://stream.nightride.fm:8443/nightride/nightride.m3u8",
        accent: PaletteToken::BrandMagenta,
    },
    Station {
        slug: "chillsynth",
        display_name: "CHILLSYNTH FM",
        genre: "Chillsynth Dreamwave",
        stream_mp3: "https://stream.nightride.fm/chillsynth.mp3",
        stream_hls: "https://stream.nightride.fm:8443/chillsynth/chillsynth.m3u8",
        accent: PaletteToken::BrandCyan,
    },
    Station {
        slug: "datawave",
        display_name: "DATAWAVE FM",
        genre: "Datawave Vaporwave",
        stream_mp3: "https://stream.nightride.fm/datawave.mp3",
        stream_hls: "https://stream.nightride.fm:8443/datawave/datawave.m3u8",
        accent: PaletteToken::BrandBlue,
    },
    Station {
        slug: "spacesynth",
        display_name: "SPACESYNTH FM",
        genre: "Spacesynth Cosmic",
        stream_mp3: "https://stream.nightride.fm/spacesynth.mp3",
        stream_hls: "https://stream.nightride.fm:8443/spacesynth/spacesynth.m3u8",
        accent: PaletteToken::BrandLightBlue,
    },
    Station {
        slug: "darksynth",
        display_name: "DARKSYNTH",
        genre: "Darksynth Cyberpunk",
        stream_mp3: "https://stream.nightride.fm/darksynth.mp3",
        stream_hls: "https://stream.nightride.fm:8443/darksynth/darksynth.m3u8",
        accent: PaletteToken::BrandRed,
    },
    Station {
        slug: "horrorsynth",
        display_name: "HORRORSYNTH",
        genre: "Horrorsynth Industrial",
        stream_mp3: "https://stream.nightride.fm/horrorsynth.mp3",
        stream_hls: "https://stream.nightride.fm:8443/horrorsynth/horrorsynth.m3u8",
        accent: PaletteToken::BrandDarkGray,
    },
    Station {
        slug: "ebsm",
        display_name: "EBSM",
        genre: "Electronic Body Dark Techno",
        stream_mp3: "https://stream.nightride.fm/ebsm.mp3",
        stream_hls: "https://stream.nightride.fm:8443/ebsm/ebsm.m3u8",
        accent: PaletteToken::BrandYellow,
    },
    Station {
        slug: "rekt",
        display_name: "REKT",
        genre: "Aggressive Synthwave",
        stream_mp3: "https://stream.nightride.fm/rekt.mp3",
        stream_hls: "https://stream.nightride.fm:8443/rekt/rekt.m3u8",
        accent: PaletteToken::Warm1,
    },
    Station {
        slug: "rektory",
        display_name: "REKTORY",
        genre: "Hardstyle Synth",
        stream_mp3: "https://stream.nightride.fm/rektory.mp3",
        stream_hls: "https://stream.nightride.fm:8443/rektory/rektory.m3u8",
        accent: PaletteToken::Warm2,
    },
];

/// Allowed host for any station URL — compile-time and CONFIG.md-loaded alike.
///
/// Defends against argument-injection (`--`-prefixed strings) and
/// host-substitution attacks if a future revision lets users override the
/// catalogue from CONFIG.md.
pub const ALLOWED_STREAM_HOST: &str = "stream.nightride.fm";

/// Validate a stream URL against the project allow-list.
///
/// 1. `scheme()` MUST equal `https`.
/// 2. `host_str()` MUST equal [`ALLOWED_STREAM_HOST`].
///
/// # Errors
/// Returns [`NightrideError::Validation`] when parsing fails or either guard
/// rejects the URL.
pub fn validate_stream_url(raw: &str) -> Result<Url, NightrideError> {
    let url = Url::parse(raw).map_err(|err| {
        NightrideError::validation(
            "station::validate_stream_url",
            "url",
            format!("parse error: {err}"),
        )
    })?;

    if url.scheme() != "https" {
        return Err(NightrideError::Validation {
            op: "station::validate_stream_url",
            field: "scheme",
            detail: format!("scheme not allowed: {}", url.scheme()),
        });
    }

    match url.host_str() {
        Some(host) if host == ALLOWED_STREAM_HOST => Ok(url),
        Some(host) => Err(NightrideError::Validation {
            op: "station::validate_stream_url",
            field: "host",
            detail: format!("host not allowed: {host}"),
        }),
        None => Err(NightrideError::Validation {
            op: "station::validate_stream_url",
            field: "host",
            detail: "missing host".to_string(),
        }),
    }
}

/// Validate the entire registry once at startup.
///
/// Called from `lib::run` before any audio task spawns; surfaces the first
/// malformed entry as a `Validation` error and refuses to start.
///
/// # Errors
/// Returns [`NightrideError::Validation`] on the first failing entry.
pub fn validate_registry() -> Result<(), NightrideError> {
    for station in DEFAULT_STATIONS {
        validate_stream_url(station.stream_mp3)?;
        validate_stream_url(station.stream_hls)?;
    }
    Ok(())
}

/// Lookup a station by slug.
#[must_use]
pub fn by_slug(slug: &str) -> Option<&'static Station> {
    DEFAULT_STATIONS.iter().find(|s| s.slug == slug)
}

/// Return the next station in registry order, wrapping at the end.
///
/// Used by `next-station` keybinding. If `current` is somehow not in the
/// registry (defensive guard), returns the first station.
#[must_use]
pub fn next(current: &Station) -> &'static Station {
    let idx = DEFAULT_STATIONS
        .iter()
        .position(|s| s.slug == current.slug)
        .unwrap_or(0);
    let next_idx = (idx + 1) % DEFAULT_STATIONS.len();
    &DEFAULT_STATIONS[next_idx]
}

/// Return the previous station in registry order, wrapping at the start.
#[must_use]
pub fn prev(current: &Station) -> &'static Station {
    let idx = DEFAULT_STATIONS
        .iter()
        .position(|s| s.slug == current.slug)
        .unwrap_or(0);
    let n = DEFAULT_STATIONS.len();
    let prev_idx = if idx == 0 { n - 1 } else { idx - 1 };
    &DEFAULT_STATIONS[prev_idx]
}

#[cfg(test)]
mod tests {
    use super::{
        ALLOWED_STREAM_HOST, DEFAULT_STATIONS, by_slug, next, prev, validate_registry,
        validate_stream_url,
    };

    /// 7 original Nightride channels + rekt + rektory = 9 total.
    #[test]
    fn default_stations_count_is_nine() {
        assert_eq!(DEFAULT_STATIONS.len(), 9);
    }

    #[test]
    fn slugs_are_lowercase_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for station in DEFAULT_STATIONS {
            assert_eq!(
                station.slug.to_lowercase(),
                station.slug,
                "slug {} must be lowercase",
                station.slug
            );
            assert!(seen.insert(station.slug), "duplicate slug {}", station.slug);
        }
    }

    /// The registry passes its own validator at startup.
    #[test]
    fn registry_self_validates() {
        validate_registry().expect("registry must self-validate at startup");
    }

    #[test]
    fn accepts_default_stations() {
        for station in DEFAULT_STATIONS {
            validate_stream_url(station.stream_mp3)
                .expect("default mp3 url must pass the allow-list");
        }
    }

    #[test]
    fn rejects_plaintext_scheme() {
        let url = format!("http://{ALLOWED_STREAM_HOST}/nightride.mp3");
        assert!(validate_stream_url(&url).is_err());
    }

    #[test]
    fn rejects_foreign_host() {
        assert!(validate_stream_url("https://evil.example.com/nightride.mp3").is_err());
    }

    #[test]
    fn rejects_garbage_input() {
        assert!(validate_stream_url("--inject").is_err());
        assert!(validate_stream_url("").is_err());
    }

    #[test]
    fn by_slug_finds_known_stations() {
        assert!(by_slug("nightride").is_some());
        assert!(by_slug("rektory").is_some());
        assert!(by_slug("nonexistent").is_none());
    }

    /// Next wraps from last (rektory) back to first (nightride).
    #[test]
    fn next_wraps_at_end() {
        let last = by_slug("rektory").unwrap();
        let first = next(last);
        assert_eq!(first.slug, "nightride");
    }

    /// Prev wraps from first (nightride) back to last (rektory).
    #[test]
    fn prev_wraps_at_start() {
        let first = by_slug("nightride").unwrap();
        let last = prev(first);
        assert_eq!(last.slug, "rektory");
    }

    /// Round-trip `next` ∘ `prev` == identity for every station in
    /// the registry.
    #[test]
    fn next_then_prev_roundtrips() {
        for station in DEFAULT_STATIONS {
            let round = prev(next(station));
            assert_eq!(round.slug, station.slug);
        }
    }
}
