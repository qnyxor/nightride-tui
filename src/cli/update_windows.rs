// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Native Windows self-update flow.
//!
//! POSIX hosts use the curl-piped `sh.nightride-tui.qnyxor.nexus`
//! installer; Windows has no `sh`, so this module implements the
//! same intent in pure Rust + a single `powershell.exe` shell-out
//! for zip extraction. Pipeline:
//!
//! 1. Query the GitHub Releases API for `tag_name` of `latest`.
//! 2. Skip if we are already on it.
//! 3. Download `nightride-tui-x86_64-pc-windows-msvc.zip` + matching
//!    `.sha256` sidecar to a `tempfile::tempdir()` scratch dir.
//! 4. Verify SHA-256.
//! 5. `powershell.exe -Command Expand-Archive ...` into the same dir.
//! 6. Locate the new `nightride-tui.exe` inside the extracted layout.
//! 7. Rename the running `current_exe()` to `<exe>.old` (Windows
//!    tolerates a rename on a running PE) and copy the new bytes
//!    over the original path.
//!
//! On any error the original `.exe` stays intact; the rename happens
//! last so a failed download or extract never leaves the user with a
//! half-installed binary. The `.old` file is left on disk — Windows
//! releases the lock when the running process exits and a future
//! launch can clean it up; we don't bother now.

use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command as ProcCommand;

use sha2::{Digest, Sha256};

use crate::error::{NightrideError, Result};

/// GitHub Releases API endpoint for the canonical `latest` resolution.
const RELEASES_API_URL: &str = "https://api.github.com/repos/qnyxor/nightride-tui/releases/latest";

/// Filename of the Windows release artifact. Must match the
/// `Package tarball` step in `.github/workflows/release.yml`.
const ARTIFACT_NAME: &str = "nightride-tui-x86_64-pc-windows-msvc.zip";

/// Filename of the executable inside the unpacked archive.
const EXE_NAME: &str = "nightride-tui.exe";

/// Extension appended to the running binary during the swap so the
/// new bytes can land at the original path.
const BACKUP_EXT: &str = "exe.old";

/// Native Windows update entry point. Routed from
/// `cli::run_update` when `cfg!(target_os = "windows")`.
pub(super) fn run() -> Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");
    let current_tag = format!("v{current_version}");

    println!();
    println!("[+] check    :: querying github releases api");

    let client = build_http_client()?;
    let latest_tag = fetch_latest_tag(&client)?;

    if latest_tag == current_tag {
        println!(
            "[+] check    :: nightride-tui {current_version} — already on latest, nothing to do."
        );
        println!();
        return Ok(());
    }

    println!(
        "[+] check    :: nightride-tui {current_version} → {}",
        latest_tag.trim_start_matches('v')
    );

    let scratch = tempfile::tempdir().map_err(|err| NightrideError::Io {
        op: "cli::update_windows::tempdir",
        source: err,
    })?;
    let scratch_path = scratch.path();

    let zip_path = scratch_path.join(ARTIFACT_NAME);
    let sha_path = scratch_path.join(format!("{ARTIFACT_NAME}.sha256"));

    println!("[+] download :: {ARTIFACT_NAME}");
    download(&client, &asset_url(&latest_tag, ARTIFACT_NAME), &zip_path)?;
    download(
        &client,
        &asset_url(&latest_tag, &format!("{ARTIFACT_NAME}.sha256")),
        &sha_path,
    )?;

    println!("[+] verify   :: sha-256");
    verify_sha256(&zip_path, &sha_path)?;

    println!("[+] extract  :: expand-archive");
    let extract_dir = scratch_path.join("extracted");
    extract_zip(&zip_path, &extract_dir)?;

    let new_exe = locate_exe(&extract_dir)?;

    println!("[+] swap     :: in-place via rename trick");
    swap_in_place(&new_exe)?;

    println!(
        "[ok] update  :: complete {current_version} → {}",
        latest_tag.trim_start_matches('v')
    );
    println!();
    println!("// shell-hint :: relaunch your terminal session if the new bytes are not picked up.");

    Ok(())
}

/// Build a blocking reqwest client with a pinned User-Agent. The
/// crate's `reqwest` dependency is already configured with
/// `default-features = false`, `rustls-tls`, and `blocking` in
/// `Cargo.toml`, so this carries no system-OpenSSL dependency on
/// Windows.
fn build_http_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(crate::USER_AGENT)
        .build()
        .map_err(|err| {
            NightrideError::config_invalid(
                "cli::update_windows::client",
                format!("could not build http client: {err}"),
            )
        })
}

/// Hit the Releases API and pull `tag_name` from the JSON response.
/// We do not parse the whole payload — the schema has been stable
/// since 2018 and one substring scan keeps us off the `serde_json`
/// hot path for this single read.
fn fetch_latest_tag(client: &reqwest::blocking::Client) -> Result<String> {
    let response = client.get(RELEASES_API_URL).send().map_err(|err| {
        NightrideError::config_invalid(
            "cli::update_windows::api_send",
            format!("could not query GitHub releases api: {err}"),
        )
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(NightrideError::config_invalid(
            "cli::update_windows::api_status",
            format!("GitHub releases api returned http {status}"),
        ));
    }
    let body = response.text().map_err(|err| {
        NightrideError::config_invalid(
            "cli::update_windows::api_body",
            format!("could not read api response: {err}"),
        )
    })?;
    parse_tag_name(&body).ok_or_else(|| {
        NightrideError::config_invalid(
            "cli::update_windows::api_parse",
            "could not parse `tag_name` from GitHub api response",
        )
    })
}

/// Substring-scan parser for `tag_name` in the Releases API body.
fn parse_tag_name(body: &str) -> Option<String> {
    let key = "\"tag_name\"";
    let start = body.find(key)?;
    let after_key = &body[start + key.len()..];
    let colon = after_key.find(':')?;
    let after_colon = &after_key[colon + 1..];
    let quote = after_colon.find('"')?;
    let after_quote = &after_colon[quote + 1..];
    let end = after_quote.find('"')?;
    Some(after_quote[..end].to_string())
}

/// Compose the public download URL for an artifact released under
/// the resolved tag.
fn asset_url(tag: &str, filename: &str) -> String {
    format!("https://github.com/qnyxor/nightride-tui/releases/download/{tag}/{filename}")
}

/// Stream `url` to `dest`. Aborts on non-2xx HTTP. Uses the same
/// blocking client as the API call.
fn download(client: &reqwest::blocking::Client, url: &str, dest: &Path) -> Result<()> {
    let response = client.get(url).send().map_err(|err| {
        NightrideError::config_invalid(
            "cli::update_windows::download_send",
            format!("could not request {url}: {err}"),
        )
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(NightrideError::config_invalid(
            "cli::update_windows::download_status",
            format!("download of {url} returned http {status}"),
        ));
    }
    let bytes = response.bytes().map_err(|err| {
        NightrideError::config_invalid(
            "cli::update_windows::download_body",
            format!("could not read body of {url}: {err}"),
        )
    })?;
    let mut file = std::fs::File::create(dest).map_err(|err| NightrideError::Io {
        op: "cli::update_windows::download_create",
        source: err,
    })?;
    file.write_all(&bytes).map_err(|err| NightrideError::Io {
        op: "cli::update_windows::download_write",
        source: err,
    })?;
    Ok(())
}

/// Compute the SHA-256 of `zip_path` and compare against the digest
/// in the `.sha256` sidecar (canonical `<digest>  <filename>` two-
/// space layout; the parser also tolerates single-space and tab).
fn verify_sha256(zip_path: &Path, sha_path: &Path) -> Result<()> {
    let mut file = std::fs::File::open(zip_path).map_err(|err| NightrideError::Io {
        op: "cli::update_windows::sha_open_zip",
        source: err,
    })?;
    let mut hasher = Sha256::new();
    // Heap-allocate the read buffer; a 64 KiB stack array trips
    // clippy::large-stack-arrays at the -D warnings tier we run in CI.
    let mut buf = vec![0u8; 64 * 1024].into_boxed_slice();
    loop {
        let n = file.read(&mut buf).map_err(|err| NightrideError::Io {
            op: "cli::update_windows::sha_read",
            source: err,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let computed = format!("{:x}", hasher.finalize());

    let sidecar = std::fs::read_to_string(sha_path).map_err(|err| NightrideError::Io {
        op: "cli::update_windows::sha_read_sidecar",
        source: err,
    })?;
    let expected = sidecar
        .split_whitespace()
        .next()
        .ok_or_else(|| {
            NightrideError::config_invalid(
                "cli::update_windows::sha_parse",
                "checksum sidecar is empty",
            )
        })?
        .to_lowercase();

    if expected != computed {
        return Err(NightrideError::config_invalid(
            "cli::update_windows::sha_mismatch",
            format!("sha-256 mismatch: pinned {expected}, computed {computed}"),
        ));
    }
    Ok(())
}

/// Shell out to PowerShell's built-in `Expand-Archive` to unpack the
/// zip. Every supported Windows target (Win10 1809+, Win11) ships
/// PowerShell 5.1 in-box; no new install required. `-NoProfile` and
/// `-NonInteractive` keep the call deterministic across user shells.
fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest).map_err(|err| NightrideError::Io {
        op: "cli::update_windows::extract_mkdir",
        source: err,
    })?;
    let cmd = format!(
        "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
        zip_path.display(),
        dest.display()
    );
    let output = ProcCommand::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &cmd,
        ])
        .output()
        .map_err(|err| NightrideError::Io {
            op: "cli::update_windows::extract_spawn",
            source: err,
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(NightrideError::config_invalid(
            "cli::update_windows::extract_status",
            format!("Expand-Archive failed: {stderr}"),
        ));
    }
    Ok(())
}

/// Walk `dir` (depth ≤ 2) for `nightride-tui.exe`. Release archives
/// either ship the binary at the root or under a single
/// `nightride-tui-<target>/` folder, so a shallow scan covers both.
fn locate_exe(dir: &Path) -> Result<PathBuf> {
    fn scan(dir: &Path, depth: u8) -> std::io::Result<Option<PathBuf>> {
        if depth == 0 {
            return Ok(None);
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.file_name().and_then(|n| n.to_str()) == Some(EXE_NAME) {
                return Ok(Some(path));
            }
            if path.is_dir()
                && let Some(found) = scan(&path, depth - 1)?
            {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }

    scan(dir, 3)
        .map_err(|err| NightrideError::Io {
            op: "cli::update_windows::locate_walk",
            source: err,
        })?
        .ok_or_else(|| {
            NightrideError::config_invalid(
                "cli::update_windows::locate_missing",
                format!("'{EXE_NAME}' not found inside extracted archive"),
            )
        })
}

/// Replace the running binary's bytes with `new_exe` via the
/// canonical Windows rename-trick: rename the live `.exe` to a
/// `.exe.old` sibling (Windows allows `MOVEFILE` on an executing
/// binary), then copy the new bytes over the original path.
///
/// The `.old` file is left in place — the OS releases its lock when
/// the current process exits, but unlinking it from inside a process
/// that holds it open is not portable. Future launches can opt to
/// clean it up at startup; we do not bother now.
fn swap_in_place(new_exe: &Path) -> Result<()> {
    let current = std::env::current_exe().map_err(|err| NightrideError::Io {
        op: "cli::update_windows::current_exe",
        source: err,
    })?;
    let backup = current.with_extension(BACKUP_EXT);

    let _ = std::fs::remove_file(&backup);

    std::fs::rename(&current, &backup).map_err(|err| NightrideError::Io {
        op: "cli::update_windows::rename_current",
        source: err,
    })?;

    if let Err(err) = std::fs::copy(new_exe, &current) {
        let _ = std::fs::rename(&backup, &current);
        return Err(NightrideError::Io {
            op: "cli::update_windows::copy_new",
            source: err,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ARTIFACT_NAME, asset_url, parse_tag_name};

    #[test]
    fn parse_tag_name_extracts_value() {
        let body = r#"{"draft":false,"tag_name":"v1.1.1","name":"v1.1.1"}"#;
        assert_eq!(parse_tag_name(body), Some("v1.1.1".to_string()));
    }

    #[test]
    fn parse_tag_name_returns_none_when_missing() {
        let body = r#"{"name":"v1.1.1"}"#;
        assert_eq!(parse_tag_name(body), None);
    }

    #[test]
    fn asset_url_matches_release_layout() {
        let url = asset_url("v1.1.1", ARTIFACT_NAME);
        assert_eq!(
            url,
            "https://github.com/qnyxor/nightride-tui/releases/download/v1.1.1/\
             nightride-tui-x86_64-pc-windows-msvc.zip"
        );
    }
}
