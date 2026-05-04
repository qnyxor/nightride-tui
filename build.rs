// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Build script.
//!
//! 1. Verifies the embedded `assets/NightrideFMMonospace.ttf` against a
//!    pinned SHA-256 (brand-asset integrity) and emits `NIGHTRIDE_FONT_SHA256`
//!    for runtime verify in `cli::install_font`. The TUI does not render with
//!    this font; it ships only for `install-font` to drop into the user's
//!    system.
//! 2. Emits compile-time vergen metadata (`VERGEN_GIT_SHA`,
//!    `VERGEN_RUSTC_SEMVER`, `VERGEN_CARGO_TARGET_TRIPLE`).
//!
//! Iosevka Term Nerd Font Regular is no longer embedded. It is fetched
//! on-demand at `install-tui-font` time from a pinned upstream URL; see
//! `src/cli/fonts.rs` for the URL and SHA-256 pin.
//!
//! If `vergen` fails (e.g. release source zips that ship without `.git`)
//! we swallow the error so `option_env!` callers fall back to "unknown".

use sha2::{Digest, Sha256};

/// Pinned SHA-256 of `assets/NightrideFMMonospace.ttf`.
///
/// Source: `nrfm_font.zip` (Nightride FM community Discord, 2021-12-31)
/// authored by Z (z@nightride.fm). Verified at download time. The font
/// is bundled solely as a brand asset for `install-font`; runtime
/// rendering still uses Iosevka.
const NIGHTRIDE_FONT_SHA256_PIN: &str =
    "bed6a0135f53da7b3ccb4befe04376af31fe781acde9c2e9df4eca113bd7e0ec";

const NIGHTRIDE_FONT_PATH: &str = "assets/NightrideFMMonospace.ttf";

fn main() {
    println!("cargo:rerun-if-changed={NIGHTRIDE_FONT_PATH}");
    // Re-run when the embedded schema template changes; otherwise cargo
    // caches the include_str! payload from a stale revision.
    println!("cargo:rerun-if-changed=nightride-tui.md");
    verify_asset(
        NIGHTRIDE_FONT_PATH,
        NIGHTRIDE_FONT_SHA256_PIN,
        "NIGHTRIDE_FONT_SHA256",
        "NIGHTRIDE_FONT_PATH",
        "(re-extract from upstream nrfm_font.zip)",
        "NIGHTRIDE_FONT_SHA256_PIN",
        AssetKind::Font,
    );

    if let Err(err) = vergen::EmitBuilder::builder()
        .git_sha(false)
        .rustc_semver()
        .cargo_target_triple()
        .emit()
    {
        println!("cargo:warning=vergen emit failed ({err}); option_env! will fall back");
    }
}

enum AssetKind {
    Font,
}

fn verify_asset(
    path: &str,
    pin: &str,
    sha_env: &str,
    path_env: &str,
    refresh_hint: &str,
    pin_const: &str,
    kind: AssetKind,
) {
    let bytes = std::fs::read(path).unwrap_or_else(|err| {
        panic!("asset missing at {path}: {err}. Run `{refresh_hint}` to restore.")
    });

    match kind {
        AssetKind::Font => verify_font_magic(path, &bytes, refresh_hint),
    }

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    let computed = format!("{digest:x}");

    if computed != pin {
        panic!(
            "{path} SHA-256 mismatch.\n  expected: {pin}\n  computed: {computed}\n\
             The committed asset has been replaced or corrupted. Refresh via \
             `{refresh_hint}` and update {pin_const} in build.rs."
        );
    }

    println!("cargo:rustc-env={sha_env}={pin}");
    println!("cargo:rustc-env={path_env}={path}");
}

/// Reject anything whose first 4 bytes are not a known SFNT magic.
/// Belt-and-suspenders against the `make fetch-iosevka` failure mode where
/// upstream serves an HTML 404 page, which a SHA pin alone happily blesses.
/// Accepted: TrueType (`00 01 00 00`), OpenType/CFF (`OTTO`), classic Mac
/// (`true`), PostScript-flavoured (`typ1`).
fn verify_font_magic(path: &str, bytes: &[u8], refresh_hint: &str) {
    const MAGICS: &[&[u8]] = &[&[0x00, 0x01, 0x00, 0x00], b"OTTO", b"true", b"typ1"];
    let head = bytes.get(..4).unwrap_or(&[]);
    if !MAGICS.contains(&head) {
        panic!(
            "{path} is not a valid font file (first 4 bytes: {head:02x?}). \
             Likely an HTML error page or truncated download. \
             Refresh via `{refresh_hint}`."
        );
    }
}
