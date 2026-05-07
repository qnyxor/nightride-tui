// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Font install helpers — embedded font data, SHA-256 verification, and
//! platform font-directory resolution.
//!
//! Two installable fonts are supported:
//!
//! - [`IOSEVKA`] — Iosevka Term Nerd Font Regular (TUI render face). Fetched
//!   on-demand from the pinned upstream `raw.githubusercontent.com` URL; not
//!   embedded in the binary.
//! - [`NIGHTRIDE_FM_MONO`] — Nightride FM Monospace (brand display face,
//!   authored by Z of Nightride FM). Embedded in the binary.
//!
//! The public surface for callers outside this module is
//! [`install_tui_font`] and [`install_nightride_font`].

use std::io::Read;
use std::path::PathBuf;
use std::process::Command as ProcCommand;

use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};

use crate::error::{NightrideError, Result};

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

/// SIL OFL 1.1 attribution + reserved-font-name notice for the bundled
/// Iosevka Term Nerd Font Regular. Travels with the .ttf when
/// `install-tui-font` writes it, satisfying OFL 1.1 §2.
pub(super) const IOSEVKA_LICENSE_BLOB: &[u8] =
    include_bytes!("../../assets/IosevkaTermNerdFont-Regular.LICENSE.txt");

/// Verbatim upstream README for the Nightride FM Monospace asset
/// (custom permissive grant from author Z). Travels with the .ttf
/// when `install-nightride-font` writes it.
pub(super) const NIGHTRIDE_FONT_LICENSE_BLOB: &[u8] =
    include_bytes!("../../assets/NightrideFMMonospace.LICENSE.txt");

/// Pinned upstream URL for the Iosevka Term Nerd Font Regular TTF.
/// Points at the official `ryanoasis/nerd-fonts` repo at the immutable
/// tag `v3.4.0`. Tag is canon — bytes verified live D20260504T11Z+0200
/// against the SHA pin below.
pub(super) const IOSEVKA_DOWNLOAD_URL: &str = "https://raw.githubusercontent.com/ryanoasis/nerd-fonts/v3.4.0/patched-fonts/IosevkaTerm/IosevkaTermNerdFont-Regular.ttf";

/// SHA-256 of the bytes served at `IOSEVKA_DOWNLOAD_URL`. Identical to
/// the embed pin shipped in v1.0.1; the bytes themselves are unchanged
/// across the embed -> fetch transition.
pub(super) const IOSEVKA_SHA256_PIN: &str =
    "d5116846a175ef4a988f61241dd3572d6a9dd3e09d4d168c67954b10783a7880";

/// SFNT magic for TrueType-flavoured (.ttf) fonts. Used as the first
/// pre-write integrity gate against any non-TTF bytes (HTML 404 pages,
/// rate-limit JSON, archive headers).
pub(super) const SFNT_MAGIC_TRUETYPE: [u8; 4] = [0x00, 0x01, 0x00, 0x00];

/// reqwest connect timeout for the on-demand font download.
pub(super) const DOWNLOAD_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// reqwest overall timeout for the on-demand font download.
pub(super) const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Discriminated source for an installable font. The install pipeline
/// dispatches on this enum to either pull bytes from an embedded blob
/// (compile-time known) or fetch them from a pinned upstream URL.
#[derive(Clone, Copy)]
pub(super) enum FontSource {
    /// Bytes embedded in the binary via `include_bytes!`. SHA-pinned at
    /// build time. No network involved.
    Embedded { blob: &'static [u8] },
    /// Bytes fetched on-demand from a pinned URL. Three pre-write
    /// integrity gates (HTTP, magic, SHA) defend the install path.
    Remote { url: &'static str },
}

/// Static descriptor for one font installed by `install-font`. Keeps
/// the install loop a single match-free pass and lets the credit lines
/// live next to the source descriptor they describe.
pub(super) struct InstallableFont {
    /// On-disk filename in the platform font directory.
    pub(super) file_name: &'static str,
    /// Source of font bytes — embedded blob or remote URL.
    pub(super) source: FontSource,
    /// Build-time pinned SHA-256 (embedded) or live pin (remote).
    pub(super) sha256: &'static str,
    /// Expected SFNT magic prefix for pre-write integrity check.
    pub(super) magic: &'static [u8],
    /// Display name surfaced to the user after install.
    pub(super) display_name: &'static str,
    /// Credit line printed after install ("by AUTHOR — LICENSE").
    pub(super) credit: &'static str,
    /// Optional companion license filename written alongside the font.
    /// Closes OFL 1.1 §2 / equivalent grants by keeping the license
    /// next to its font on the user's disk, not just inside the source
    /// tree.
    pub(super) license_file_name: Option<&'static str>,
    /// License text bundled with the font asset.
    pub(super) license_blob: Option<&'static [u8]>,
}

/// The TUI's runtime render face. `install-font` writes this one.
/// Bytes are not embedded in the binary — fetched on-demand from
/// the pinned upstream URL at install time, then verified via three
/// integrity gates (HTTP status, SFNT magic, SHA-256).
pub(super) const IOSEVKA: InstallableFont = InstallableFont {
    file_name: "IosevkaTermNerdFont-Regular.ttf",
    source: FontSource::Remote {
        url: IOSEVKA_DOWNLOAD_URL,
    },
    sha256: IOSEVKA_SHA256_PIN,
    magic: &SFNT_MAGIC_TRUETYPE,
    display_name: "Iosevka Term Nerd Font Regular",
    credit: "by Belleve Invis (Iosevka) + Ryan L McIntyre (Nerd Fonts) — SIL OFL 1.1",
    license_file_name: Some("IosevkaTermNerdFont-Regular.LICENSE.txt"),
    license_blob: Some(IOSEVKA_LICENSE_BLOB),
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
    source: FontSource::Embedded {
        blob: NIGHTRIDE_FONT_BLOB,
    },
    sha256: NIGHTRIDE_FONT_SHA256,
    magic: &SFNT_MAGIC_TRUETYPE,
    display_name: "Nightride FM Monospace",
    credit: "by Z (Nightride FM Discord, z@nightride.fm) — free for personal & commercial use",
    license_file_name: Some("NightrideFMMonospace.LICENSE.txt"),
    license_blob: Some(NIGHTRIDE_FONT_LICENSE_BLOB),
};

/// Roster preserved for invariant tests. Each entry has its own
/// dedicated `install-*` subcommand; this slice does NOT drive a
/// "install everything" flow anymore.
#[cfg(test)]
pub(super) const INSTALLABLE_FONTS: &[InstallableFont] = &[IOSEVKA, NIGHTRIDE_FM_MONO];

pub(super) const INSTALL_TUI_FONT_INTRO: &str =
    "Install Iosevka Term Nerd Font Regular into your user font directory.";

/// Post-install copy for macOS / Linux. The OS-side font cache picks
/// the file up automatically; the user only has to relaunch their
/// terminal and (optionally) select the new face.
pub(super) const INSTALL_TUI_FONT_NOTE: &str = "Relaunch your terminal. Iosevka Term Nerd Font is now installed.\n\
     Select it in terminal preferences to render the spectrum and UI chrome.\n\
     Other monospace fonts with Braille and Box Drawing support also work.\n\
     See the license file for details.";

/// Post-install copy for Windows. Drop-and-go does not register the
/// font with the OS — the user has to double-click the .ttf and pick
/// "Install for me only" before the terminal can see it. We pop File
/// Explorer onto the file via `try_open_font_folder` to make the
/// double-click a one-click affair.
pub(super) const INSTALL_TUI_FONT_NOTE_WINDOWS: &str = "The font has been copied. Double-click the .ttf file and choose\n\
     'Install for me only' to register it with Windows.\n\
     Then relaunch your terminal and select 'IosevkaTermNerdFont-Regular'\n\
     in its preferences. See the license file for details.";

pub(super) const INSTALL_NIGHTRIDE_FONT_INTRO: &str =
    "Install Nightride FM Monospace into your user font directory.";

/// Post-install copy for macOS / Linux.
pub(super) const INSTALL_NIGHTRIDE_FONT_NOTE: &str = "Relaunch your terminal. Nightride FM Monospace is now installed.\n\
     Use it for banners and screenshots only — do not select it for the\n\
     live TUI, as it lacks the glyphs needed to render the interface\n\
     correctly. See the license file for details.";

/// Post-install copy for Windows — same double-click step as the TUI
/// font, plus the brand-display caveat (do not select for the live
/// TUI; the face does not cover the glyph set needed to render the
/// interface).
pub(super) const INSTALL_NIGHTRIDE_FONT_NOTE_WINDOWS: &str = "The font has been copied. Double-click the .ttf file and choose\n\
     'Install for me only' to register it with Windows.\n\
     Use it for banners and screenshots only — do not select it for the\n\
     live TUI; it lacks the glyphs needed to render the interface.";

/// Install Iosevka Term Nerd Font Regular — the TUI render face —
/// into the platform font directory. Bytes are downloaded on demand
/// from the pinned upstream URL; three pre-write integrity gates
/// (HTTP status, SFNT magic, SHA-256) defend the install path.
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
/// Returns [`NightrideError::Io`] for filesystem failure,
/// [`NightrideError::Network`] for transport errors,
/// [`NightrideError::NetworkRejected`] for HTTP error status, or
/// [`NightrideError::Validation`] for SHA mismatch / unsupported
/// platform / missing $HOME.
pub fn install_tui_font() -> Result<()> {
    let dest = match install_one_font(&IOSEVKA) {
        Ok(path) => path,
        Err(err) => {
            // Fetch failures (DNS down, TLS reject, 4xx/5xx) leave the user
            // without the font and without a recovery path unless we surface
            // the upstream URL + SHA pin so they can complete the install
            // manually. Validation gate failures (magic / SHA mismatch) are
            // intentionally NOT covered here — those signal upstream drift
            // and the user must wait for a re-pinned release.
            if matches!(
                err,
                NightrideError::Network { .. } | NightrideError::NetworkRejected { .. }
            ) {
                eprintln!();
                eprintln!("could not fetch the font from upstream");
                eprintln!();
                eprintln!("manual download fallback:");
                eprintln!("  url:     {IOSEVKA_DOWNLOAD_URL}");
                eprintln!("  sha256:  {IOSEVKA_SHA256_PIN}");
                eprintln!("  place at: <your platform font dir>/IosevkaTermNerdFont-Regular.ttf");
            }
            return Err(err);
        }
    };
    println!();
    if cfg!(target_os = "windows") {
        println!("{INSTALL_TUI_FONT_NOTE_WINDOWS}");
    } else {
        println!("{INSTALL_TUI_FONT_NOTE}");
    }
    try_open_font_folder(&dest);
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
    let dest = install_one_font(&NIGHTRIDE_FM_MONO)?;
    println!();
    if cfg!(target_os = "windows") {
        println!("{INSTALL_NIGHTRIDE_FONT_NOTE_WINDOWS}");
    } else {
        println!("{INSTALL_NIGHTRIDE_FONT_NOTE}");
    }
    try_open_font_folder(&dest);
    Ok(())
}

/// Shared install path: dispatch on `FontSource`, verify integrity,
/// mkdir, atomic write via tempfile+persist, print credit, run
/// `fc-cache` on Linux. Used by [`install_tui_font`] and
/// [`install_nightride_font`] so the on-wire side-effect stays
/// symmetric.
///
/// Returns the absolute path the font was written to so callers can
/// pass it to `try_open_font_folder` for the Windows Explorer-pop
/// affordance.
fn install_one_font(font: &InstallableFont) -> Result<PathBuf> {
    let bytes: Vec<u8> = match font.source {
        FontSource::Embedded { blob } => {
            // Verify embedded blob at install time — build.rs already
            // checked at compile time, but defence-in-depth catches any
            // corruption that happened post-build (e.g. binary patching).
            verify_font_magic(blob, font.magic)?;
            verify_font_sha(blob, font.sha256)?;
            blob.to_vec()
        }
        FontSource::Remote { url } => {
            // download_to_bytes runs all three integrity gates internally
            // (HTTP status, magic, SHA) before returning bytes.
            download_to_bytes(url, font.magic, font.sha256)?
        }
    };

    let dir = platform_font_dir()?;
    std::fs::create_dir_all(&dir).map_err(|err| NightrideError::Io {
        op: "cli::install_font::mkdir",
        source: err,
    })?;

    let dest = dir.join(font.file_name);

    // Atomic write: tempfile in same dir + persist (rename(2)). Avoids
    // a partial file at dest if the process is interrupted mid-write.
    let mut tmp = tempfile::NamedTempFile::new_in(&dir).map_err(|err| NightrideError::Io {
        op: "cli::install_font::tempfile",
        source: err,
    })?;
    std::io::Write::write_all(&mut tmp, &bytes).map_err(|err| NightrideError::Io {
        op: "cli::install_font::write",
        source: err,
    })?;
    tmp.persist(&dest).map_err(|err| NightrideError::Io {
        op: "cli::install_font::persist",
        source: err.error,
    })?;

    println!("installed {} to {}", font.display_name, dest.display());
    println!("  size: {} bytes  sha256: {}", bytes.len(), font.sha256);
    println!("  {}", font.credit);

    if let (Some(name), Some(license_blob)) = (font.license_file_name, font.license_blob) {
        let license_dest = dir.join(name);
        // License sidecar — also atomic.
        let mut tmp_license =
            tempfile::NamedTempFile::new_in(&dir).map_err(|err| NightrideError::Io {
                op: "cli::install_font::tempfile_license",
                source: err,
            })?;
        std::io::Write::write_all(&mut tmp_license, license_blob).map_err(|err| {
            NightrideError::Io {
                op: "cli::install_font::write_license",
                source: err,
            }
        })?;
        tmp_license
            .persist(&license_dest)
            .map_err(|err| NightrideError::Io {
                op: "cli::install_font::persist_license",
                source: err.error,
            })?;
        println!("  license: {}", license_dest.display());
    }

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

    Ok(dest)
}

/// Download font bytes from `url`, streaming the body while showing a
/// progress bar. Three pre-write integrity gates are applied in order:
///
/// 1. **HTTP gate** — `error_for_status()` rejects non-2xx (catches HTML
///    404 pages, rate-limit responses, and CDN error pages before we
///    inspect body bytes).
/// 2. **Magic gate** — fires as soon as we have >= 4 bytes of body;
///    rejects any non-SFNT payload (HTML, JSON, ZIP) before we write a
///    byte to disk.
/// 3. **SHA gate** — full-body SHA-256 verified against the compile-time
///    pin before the caller is allowed to persist anything.
///
/// # Errors
/// Returns [`NightrideError::Network`] for transport errors,
/// [`NightrideError::NetworkRejected`] for HTTP error status,
/// [`NightrideError::Io`] for read failures, or
/// [`NightrideError::Validation`] for magic / SHA mismatch.
pub(super) fn download_to_bytes(
    url: &str,
    expected_magic: &[u8],
    expected_sha: &str,
) -> Result<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(crate::USER_AGENT)
        .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|err| NightrideError::network("cli::download_iosevka::client_build", err))?;

    let mut resp = client
        .get(url)
        .send()
        .map_err(|err| NightrideError::network("cli::download_iosevka::send", err))?
        .error_for_status()
        .map_err(|err| {
            NightrideError::network_rejected(
                "cli::download_iosevka::http",
                format!("{url} -> {err}"),
            )
        })?;

    let total = resp.content_length().unwrap_or(0);
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template("  download [{bar:30}] {bytes}/{total_bytes} ({eta})")
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("##-"),
    );

    let capacity = usize::try_from(total).unwrap_or(0);
    let mut bytes: Vec<u8> = Vec::with_capacity(capacity);
    let mut buf = [0u8; 8192];
    let mut magic_checked = false;

    loop {
        let n = resp
            .read(&mut buf)
            .map_err(|err| NightrideError::io("cli::download_iosevka::read", err))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);

        // Magic-byte gate fires as soon as we have >= magic_len bytes of
        // body. Rejects non-SFNT payloads (HTML error pages, JSON) early,
        // before any disk write, without waiting for the full download.
        if !magic_checked && bytes.len() >= expected_magic.len() {
            verify_font_magic(&bytes, expected_magic)?;
            magic_checked = true;
        }

        pb.set_position(bytes.len() as u64);
    }
    pb.finish_and_clear();

    // SHA gate: verify whole-body hash matches the pin. Only after this
    // passes are we willing to hand the bytes to the caller for write.
    verify_font_sha(&bytes, expected_sha)?;

    Ok(bytes)
}

/// Verify that `bytes` begins with the expected SFNT magic prefix.
///
/// Returns [`NightrideError::Validation`] if the first `expected.len()`
/// bytes do not match `expected`. Fires before any disk write to reject
/// HTML 404 pages, rate-limit JSON, or archive headers that have
/// slipped past the HTTP status gate.
pub(super) fn verify_font_magic(bytes: &[u8], expected: &[u8]) -> Result<()> {
    let actual = &bytes[..expected.len().min(bytes.len())];
    if actual != expected {
        return Err(NightrideError::Validation {
            op: "cli::install_font::magic",
            field: "magic_bytes",
            detail: format!("got {actual:02x?} want {expected:02x?}"),
        });
    }
    Ok(())
}

/// Verify `bytes` against the pinned SHA-256 hex string `pin`.
///
/// Returns [`NightrideError::Validation`] on mismatch so the caller
/// can surface a clear diagnostic. The computed digest is included in
/// the detail string to ease re-pinning.
pub(super) fn verify_font_sha(bytes: &[u8], pin: &str) -> Result<()> {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let computed = format!("{:x}", hasher.finalize());
    if computed != pin {
        return Err(NightrideError::Validation {
            op: "cli::install_font::verify_sha",
            field: "font_blob",
            detail: format!("sha mismatch: pinned {pin}, computed {computed}"),
        });
    }
    Ok(())
}

/// Resolve the platform font directory. Refuses unsupported platforms
/// up front rather than writing to a half-correct location.
///
/// - macOS: `~/Library/Fonts`
/// - Linux: `~/.local/share/fonts`
/// - Windows: `%LOCALAPPDATA%\Microsoft\Windows\Fonts` — the per-user
///   fonts directory introduced in Windows 10 1809. Writable without
///   admin and indexed by Windows for the current user. Some terminal
///   emulators still need the user to double-click the .ttf and pick
///   "Install for me only" to register it in HKCU; the post-install
///   hint surfaces that step.
fn platform_font_dir() -> Result<PathBuf> {
    if cfg!(target_os = "windows") {
        // directories 6.0 BaseDirs::data_local_dir() resolves
        // %LOCALAPPDATA% via the canonical Known Folder ID
        // (FOLDERID_LocalAppData). Avoids the POSIX-only $HOME path.
        let base = directories::BaseDirs::new().ok_or_else(|| NightrideError::Validation {
            op: "cli::install_font::base_dirs",
            field: "data_local_dir",
            detail: "could not resolve %LOCALAPPDATA%; user profile missing?".to_string(),
        })?;
        return Ok(base
            .data_local_dir()
            .join("Microsoft")
            .join("Windows")
            .join("Fonts"));
    }

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
            detail: "install-font supports macOS, Linux, and Windows only".to_string(),
        })
    }
}

/// Pop File Explorer focused on the freshly-written `.ttf` so the
/// operator can double-click to register the font with the Windows
/// font subsystem (terminals on Windows do not auto-pick up files
/// dropped into `%LOCALAPPDATA%\Microsoft\Windows\Fonts\`).
///
/// Best-effort. Skipped when:
/// - host is not Windows (no-op),
/// - stdout is not a TTY (scripted invocation; do not pop UI).
///
/// `explorer.exe /select,<path>` opens the parent folder with the
/// target file pre-selected, ready for a double-click. Spawn errors
/// (missing PATH, headless session) are swallowed — the post-install
/// note already explained where the file lives.
fn try_open_font_folder(font_path: &std::path::Path) {
    use std::io::IsTerminal;
    if !cfg!(target_os = "windows") {
        return;
    }
    if !std::io::stdout().is_terminal() {
        return;
    }
    let _ = ProcCommand::new("explorer.exe")
        .arg(format!("/select,{}", font_path.display()))
        .spawn();
}
