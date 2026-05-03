// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Configuration loader for `CONFIG.md`.
//!
//! NightRideTUI reads exactly one configuration file: a Markdown document
//! whose YAML frontmatter describes runtime knobs and whose body documents
//! every key. This module owns the contract between disk and the rest of
//! the library.
//!
//! ## Contract
//!
//! 1. Locate `CONFIG.md` via a platform-aware path resolver with a CLI
//!    override flag taking priority.
//! 2. Read the file as UTF-8. Reject BOMs.
//! 3. Split frontmatter from body (`---` fences). Body is informational and
//!    discarded.
//! 4. Parse the frontmatter with `serde_norway` into [`Config`].
//! 5. Validate every field against the documented ranges.
//!
//! ## Module map
//!
//! - `loader` — `load`, `ensure_schema`, frontmatter parsing, template
//!   schema migration.
//! - `persist` — `save_state` and surgical YAML rewriting that
//!   preserves indentation + inline comments.

use std::path::PathBuf;

use directories::ProjectDirs;
use serde::Deserialize;

use crate::{NightrideError, Result, station};

mod loader;
mod persist;

pub use loader::{ensure_schema, load};
pub use persist::save_state;

/// Canonical user-dir location of `nightride-tui.md`. On macOS:
/// `~/Library/Application Support/nexus.qnyxor.nightride/nightride-tui.md`;
/// on Linux: `~/.config/nightride/nightride-tui.md`. Falls back to
/// `/tmp/nightride/nightride-tui.md` when `ProjectDirs` returns None
/// (sandboxed environment without `$HOME`).
///
/// Resolving here keeps the binary's mutable state OUT of the repo
/// working tree — running `cargo run` from the source checkout no
/// longer rewrites the embedded `TEMPLATE` file (which would
/// contaminate the build via `include_str!`).
#[must_use]
pub fn default_user_config_path() -> PathBuf {
    ProjectDirs::from("nexus", "qnyxor", "nightride").map_or_else(
        || PathBuf::from("/tmp/nightride/nightride-tui.md"),
        |d| d.config_dir().join("nightride-tui.md"),
    )
}

/// Top-level runtime configuration consumed by the v2 binary.
///
/// Public shape is intentionally flat — every field maps to an actual
/// behaviour wired in lib::run / audio / logging / ui. The CONFIG.md
/// frontmatter on disk is a hierarchical document with sections
/// (app, audio, network, theme, keymap) that predates v2; the loader
/// reads the hierarchical layout via `RawConfig` and projects only the
/// fields v2 actually consumes. Sections we do not implement yet
/// (network timeouts, theme mode, keymap overrides) are silently ignored
/// so the user's existing CONFIG.md keeps loading without errors.
#[derive(Debug, Clone)]
pub struct Config {
    /// Slug of the station played at startup. Validated against the registry.
    pub default_station: String,
    /// Initial volume (0..=100). Validated by `validate`.
    pub default_volume: u8,
    /// Tracing log level (`off`, `error`, `warn`, `info`, `debug`, `trace`).
    pub log_level: String,
    /// Override directory for log output. `None` resolves to the platform
    /// state dir via `directories` at logging init time.
    pub log_dir: Option<PathBuf>,
}

impl Default for Config {
    /// Defaults match the documented values in `CONFIG.md`.
    ///
    /// # Examples
    ///
    /// ```
    /// use nightride_tui::config::Config;
    ///
    /// let cfg = Config::default();
    /// assert_eq!(cfg.default_station, "nightride");
    /// assert_eq!(cfg.default_volume, 50);
    /// assert_eq!(cfg.log_level, "info");
    /// ```
    fn default() -> Self {
        Self {
            default_station: default_station(),
            default_volume: default_volume(),
            log_level: default_log_level(),
            log_dir: None,
        }
    }
}

/// Hierarchical mirror of the CONFIG.md frontmatter as it lives on disk.
/// Each section is `#[serde(default)]` so missing sections fall back to
/// empty `Section::default()` rather than failing the parse.
#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct RawConfig {
    #[serde(default)]
    pub(super) app: AppSection,
    #[serde(default)]
    pub(super) audio: AudioSection,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct AppSection {
    #[serde(default)]
    pub(super) log_level: Option<String>,
    #[serde(default)]
    pub(super) log_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct AudioSection {
    #[serde(default)]
    pub(super) default_station: Option<String>,
    #[serde(default)]
    pub(super) default_volume_percent: Option<u8>,
}

pub(super) fn default_station() -> String {
    "nightride".to_string()
}

pub(super) fn default_volume() -> u8 {
    50
}

pub(super) fn default_log_level() -> String {
    "info".to_string()
}

/// Cross-field validation. Surfaces the first failing constraint.
pub(super) fn validate(cfg: &Config) -> Result<()> {
    if cfg.default_volume > 100 {
        return Err(NightrideError::Validation {
            op: "config::validate",
            field: "default_volume",
            detail: format!("must be 0..=100, got {}", cfg.default_volume),
        });
    }

    if station::by_slug(&cfg.default_station).is_none() {
        return Err(NightrideError::Validation {
            op: "config::validate",
            field: "default_station",
            detail: format!("unknown station slug: {}", cfg.default_station),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Config, validate};

    #[test]
    fn default_config_is_valid() {
        let cfg = Config::default();
        validate(&cfg).expect("default config must validate");
    }

    #[test]
    fn rejects_volume_over_one_hundred() {
        let cfg = Config {
            default_volume: 200,
            ..Config::default()
        };
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn rejects_unknown_station() {
        let cfg = Config {
            default_station: "atlantis".to_string(),
            ..Config::default()
        };
        assert!(validate(&cfg).is_err());
    }
}
