// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Font install helpers — embedded font data, SHA-256 verification, and
//! platform font-directory resolution.
//!
//! Two installable fonts are bundled:
//!
//! - [`IOSEVKA`] — Iosevka Term Nerd Font Regular (TUI render face).
//! - [`NIGHTRIDE_FM_MONO`] — Nightride FM Monospace (brand display face,
//!   authored by Z of Nightride FM).
//!
//! The public surface for callers outside this module is
//! [`install_tui_font`] and [`install_nightride_font`].

use std::path::PathBuf;
use std::process::Command as ProcCommand;

use sha2::{Digest, Sha256};

use crate::error::{NightrideError, Result};

/// Embedded Iosevka Term Nerd Font Regular. SHA-256 verified at build
/// time by `build.rs`; this `include_bytes!` produces the same bytes
/// the binary committed to the repo.
pub(super) const IOSEVKA_BLOB: &[u8] =
    include_bytes!("../../assets/IosevkaTermNerdFont-Regular.ttf");

/// Pinned SHA-256, surfaced from `build.rs` via the `IOSEVKA_SHA256`
/// env var. `option_env!` falls back to "unknown" if a release source
/// zip ships without the build-script context (defensive only — every
/// regular build sets this).
pub(super) const IOSEVKA_SHA256: &str = match option_env!("IOSEVKA_SHA256") {
    Some(s) => s,
    None => "unknown",
};

/// Embedded Nightride FM Monospace TTF. Brand asset, not used at
/// runtime — only written to the user's font dir by `install-font`.
/// SHA-256 verified at build time by `build.rs`.
pub(super) const NIGHTRIDE_FONT_BLOB: &[u8] =
    include_bytes!("../../assets/NightrideFMMonospace.ttf");

/// Pinned SHA-256 of the bundled Nightride FM Monospace asset, surfaced
/// from `build.rs` via the `NIGHTRIDE_FONT_SHA256` env var.
pub(super) const NIGHTRIDE_FONT_SHA256: &str = match option_env!("NIGHTRIDE_FONT_SHA256") {
    Some(s) => s,
    None => "unknown",
};

/// Static descriptor for one font installed by `install-font`. Keeps
/// the install loop a single match-free pass and lets the credit lines
/// live next to the bytes they describe.
pub(super) struct InstallableFont {
    /// On-disk filename in the platform font directory.
    pub(super) file_name: &'static str,
    /// Embedded payload from `assets/`.
    pub(super) blob: &'static [u8],
    /// Build-time pinned SHA-256 from `build.rs`.
    pub(super) sha256: &'static str,
    /// Display name surfaced to the user after install.
    pub(super) display_name: &'static str,
    /// Credit line printed after install ("by AUTHOR — LICENSE").
    pub(super) credit: &'static str,
}

/// The TUI's runtime render face. `install-font` writes this one.
pub(super) const IOSEVKA: InstallableFont = InstallableFont {
    file_name: "IosevkaTermNerdFont-Regular.ttf",
    blob: IOSEVKA_BLOB,
    sha256: IOSEVKA_SHA256,
    display_name: "Iosevka Term Nerd Font Regular",
    credit: "by Belleve Invis (Iosevka) + Ryan L McIntyre (Nerd Fonts) — SIL OFL 1.1",
};

/// The Nightride FM brand display face. `install-nightride-font` writes
/// this one. Authored by Z (creator of Nightride FM, reachable on the
/// community Discord at <https://discord.gg/synthwave> and via
/// `z@nightride.fm`) per the upstream `assets/NightrideFMMonospace.LICENSE.txt`
/// and the font's own `name` table (`Copyright (c) 2021, Nightride FM
/// by Z`). Released "100% free for personal and commercial use" by the
/// author.
pub(super) const NIGHTRIDE_FM_MONO: InstallableFont = InstallableFont {
    file_name: "NightrideFMMonospace.ttf",
    blob: NIGHTRIDE_FONT_BLOB,
    sha256: NIGHTRIDE_FONT_SHA256,
    display_name: "Nightride FM Monospace",
    credit: "by Z (Nightride FM Discord, z@nightride.fm) — free for personal & commercial use",
};

/// Roster preserved for invariant tests. Each entry has its own
/// dedicated `install-*` subcommand; this slice does NOT drive a
/// "install everything" flow anymore.
#[cfg(test)]
pub(super) const INSTALLABLE_FONTS: &[InstallableFont] = &[IOSEVKA, NIGHTRIDE_FM_MONO];

pub(super) const INSTALL_TUI_FONT_INTRO: &str =
    "Install Iosevka Term Nerd Font Regular into your user font directory.";
pub(super) const INSTALL_TUI_FONT_NOTE: &str = "This is the recommended runtime font for \x1b[1mnightride-tui\x1b[0m because it covers Braille, Box Drawing, and Block Elements used by the visualizer and chrome.\n\nTerminals do not auto-reload fonts. Re-launch your terminal and (optionally) select 'IosevkaTermNerdFont-Regular' in its preferences. Any mono with briale + box-drawing also works - see README Embedded assets for the supported glyph ranges.\nAuthored by Belleve Invis (Iosevka) and Ryan L McIntyre (Nerd Fonts) under SIL OFL 1.1.\n---\n<https://github.com/be5invis/iosevka> | <https://nerdfonts.com>";

pub(super) const INSTALL_NIGHTRIDE_FONT_INTRO: &str =
    "Install Nightride FM Monospace into your user font directory.";
pub(super) const INSTALL_NIGHTRIDE_FONT_NOTE: &str = "This font is for branding and artwork. It does not cover the full glyph set needed by the live TUI renderer.\n\n'Nightride FM Monospace' is the brand display face — pick it for banners, art or screenshots, not for the TUI itself (it covers ASCII + Latin-1 only, no box-drawing / braille / Nerd-Font glyphs).\nAuthored by Z, creator, and owner of Nightride FM <discord.gg/synthwave>\n---\n<https://www.patreon.com/posts/official-fm-font-60533997>";

/// Install Iosevka Term Nerd Font Regular — the bundled TUI render
/// face — into the platform font directory.
///
/// Convenience only: the TUI does not require this specific font.
/// Any modern monospace face that covers Block Elements
/// (U+2580–259F), Braille (U+2800–28FF), Box Drawing (U+2500–257F)
/// and basic arrows renders the player correctly — that's most
/// macOS / JetBrains / Fira / Hack / Cascadia / Berkeley families.
/// The bundled Iosevka Term Nerd Font Regular is shipped because
/// it's a known-good baseline for users whose default mono lacks
/// braille (e.g. legacy Courier variants), not because we depend
/// on Nerd-Font private-use glyphs (we do not).
///
/// # Errors
/// Returns [`NightrideError::Io`] for filesystem failure, or
/// [`NightrideError::Validation`] for SHA mismatch / unsupported
/// platform / missing $HOME.
pub fn install_tui_font() -> Result<()> {
    install_one_font(&IOSEVKA)?;
    println!();
    println!("{INSTALL_TUI_FONT_NOTE}");
    Ok(())
}

/// Install Nightride FM Monospace — the brand display face authored by
/// Z (creator of Nightride FM) — into the platform font directory.
///
/// Decorative only. The TUI renders with Iosevka; this font ships
/// because the Nightride community distributes it freely and bundling
/// it lets users grab the matching display face in one step. The font
/// covers Basic Latin + Latin-1 Supplement only, so it cannot replace
/// Iosevka for the TUI itself.
///
/// Author + grant come from the embedded
/// `assets/NightrideFMMonospace.LICENSE.txt` (verbatim upstream
/// `README.txt`) and the font's own `name` table
/// (`Copyright (c) 2021, Nightride FM by Z`).
///
/// # Errors
/// Returns [`NightrideError::Io`] for filesystem failure, or
/// [`NightrideError::Validation`] for SHA mismatch / unsupported
/// platform / missing $HOME.
pub fn install_nightride_font() -> Result<()> {
    install_one_font(&NIGHTRIDE_FM_MONO)?;
    println!();
    println!("{INSTALL_NIGHTRIDE_FONT_NOTE}");
    Ok(())
}

/// Shared install path: SHA-verify, mkdir, write, print credit, run
/// `fc-cache` on Linux. Used by [`install_tui_font`] and
/// [`install_nightride_font`] so the on-wire side-effect stays
/// symmetric.
fn install_one_font(font: &InstallableFont) -> Result<()> {
    verify_font_sha(font)?;
    let dir = platform_font_dir()?;
    std::fs::create_dir_all(&dir).map_err(|err| NightrideError::Io {
        op: "cli::install_font::mkdir",
        source: err,
    })?;

    let dest = dir.join(font.file_name);
    std::fs::write(&dest, font.blob).map_err(|err| NightrideError::Io {
        op: "cli::install_font::write",
        source: err,
    })?;

    println!("installed {} to {}", font.display_name, dest.display());
    println!("  size: {} bytes  sha256: {}", font.blob.len(), font.sha256);
    println!("  {}", font.credit);

    // Best-effort fc-cache refresh on Linux. macOS auto-rescans.
    if cfg!(target_os = "linux") {
        match ProcCommand::new("fc-cache").arg("-fv").output() {
            Ok(out) if out.status.success() => {
                println!("fc-cache -fv: ok");
            }
            Ok(out) => {
                println!(
                    "fc-cache -fv: exit {} (continuing — install still wrote the file)",
                    out.status
                );
            }
            Err(err) => {
                println!("fc-cache not available ({err}); install still wrote the file");
            }
        }
    }

    Ok(())
}

/// Verify a single embedded font blob against its build-time pin.
pub(super) fn verify_font_sha(font: &InstallableFont) -> Result<()> {
    let mut hasher = Sha256::new();
    hasher.update(font.blob);
    let computed = format!("{:x}", hasher.finalize());
    if computed != font.sha256 {
        return Err(NightrideError::Validation {
            op: "cli::install_font::verify_sha",
            field: "font_blob",
            detail: format!(
                "embedded {} sha mismatch: pinned {}, computed {computed}",
                font.file_name, font.sha256
            ),
        });
    }
    Ok(())
}

/// Resolve the platform font directory. Refuses unsupported platforms
/// up front rather than writing to a half-correct location.
fn platform_font_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| NightrideError::Validation {
        op: "cli::install_font::home",
        field: "HOME",
        detail: "$HOME is unset; cannot resolve user font directory".to_string(),
    })?;
    let home = PathBuf::from(home);

    if cfg!(target_os = "macos") {
        Ok(home.join("Library").join("Fonts"))
    } else if cfg!(target_os = "linux") {
        Ok(home.join(".local").join("share").join("fonts"))
    } else {
        Err(NightrideError::Validation {
            op: "cli::install_font::platform",
            field: "target_os",
            detail: "install-font supports macOS and Linux only".to_string(),
        })
    }
}
