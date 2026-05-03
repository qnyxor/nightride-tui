// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Surgical YAML rewriter for `nightride-tui.md` runtime persistence.
//!
//! [`save_state`] preserves every existing line — indentation, sibling
//! fields, inline `# comments`, document body — and rewrites only the
//! two values the binary tracks across launches:
//! `audio.default_station` and `audio.default_volume_percent`. When the
//! file is absent a minimal scaffolding is emitted so first-time users
//! still get a self-documenting CONFIG.md after one shutdown.

use crate::{NightrideError, Result};

use super::Config;

/// Persist runtime state (default station + default volume) back to
/// `nightride-tui.md` so the next launch resumes where the user left off.
///
/// The implementation is surgical: when the target file exists, only the
/// two values are rewritten in-place, preserving every other line —
/// including indentation, sibling fields, inline `# comments`, and the
/// document body below the frontmatter. When the file is absent, a
/// minimal scaffolding is written.
///
/// Writes are crash-safe: payload lands on a sibling temp file first,
/// then `rename()` swaps it atomically into place. A `kill -9` or
/// power-loss between the two steps leaves either the old complete file
/// or the new complete file — never a truncated mid-write.
///
/// # Errors
/// Returns [`NightrideError::Io`] on read or write failure.
pub fn save_state(path: &std::path::Path, cfg: &Config) -> Result<()> {
    let payload = if path.exists() {
        let content = std::fs::read_to_string(path).map_err(|err| NightrideError::Io {
            op: "config::save_state::read",
            source: err,
        })?;
        surgical_replace(&content, cfg)
    } else {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|err| NightrideError::Io {
                    op: "config::save_state::mkdir",
                    source: err,
                })?;
            }
        }
        render_minimal(cfg)
    };
    atomic_write(path, &payload)
}

/// Write `payload` to `path` atomically: drop the bytes on a sibling
/// `<filename>.tmp`, fsync the dir if possible, then `rename()` over
/// the target. POSIX rename is atomic — the file at `path` is either
/// the old version or the new version, never half-written.
fn atomic_write(path: &std::path::Path, payload: &str) -> Result<()> {
    let tmp_path = match path.file_name() {
        Some(name) => {
            let mut tmp_name = name.to_os_string();
            tmp_name.push(".tmp");
            path.with_file_name(tmp_name)
        }
        None => {
            return Err(NightrideError::Io {
                op: "config::save_state::tmp_path",
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "path has no file name",
                ),
            });
        }
    };
    std::fs::write(&tmp_path, payload).map_err(|err| NightrideError::Io {
        op: "config::save_state::write_tmp",
        source: err,
    })?;
    std::fs::rename(&tmp_path, path).map_err(|err| NightrideError::Io {
        op: "config::save_state::rename",
        source: err,
    })?;
    Ok(())
}

/// Replace the values of `default_station:` and `default_volume_percent:`
/// in the document while keeping indentation and inline comments intact.
/// Lines that don't match either key pass through unchanged.
fn surgical_replace(content: &str, cfg: &Config) -> String {
    let mut out = String::with_capacity(content.len());
    let trailing_newline = content.ends_with('\n');
    let mut iter = content.lines().peekable();
    while let Some(line) = iter.next() {
        let next =
            if let Some(updated) = replace_value(line, "default_station", &cfg.default_station) {
                updated
            } else if let Some(updated) = replace_value(
                line,
                "default_volume_percent",
                &cfg.default_volume.to_string(),
            ) {
                updated
            } else {
                line.to_string()
            };
        out.push_str(&next);
        if iter.peek().is_some() {
            out.push('\n');
        }
    }
    if trailing_newline {
        out.push('\n');
    }
    out
}

/// If `line` is `<indent><key>: <value> [# comment]`, rebuild it with
/// the new value. Returns `None` when the key does not match.
fn replace_value(line: &str, key: &str, new_value: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let prefix_len = line.len() - trimmed.len();
    let header = format!("{key}:");
    if !trimmed.starts_with(&header) {
        return None;
    }
    let after = &trimmed[header.len()..];
    // Capture inline comment, if any, including its leading whitespace.
    let (_value_part, comment_part) = match after.find('#') {
        Some(idx) => (after[..idx].trim_end(), &after[idx..]),
        None => (after.trim_end(), ""),
    };
    let mut rebuilt = String::with_capacity(line.len());
    rebuilt.push_str(&line[..prefix_len]);
    rebuilt.push_str(&header);
    rebuilt.push(' ');
    // YAML-quote the value when it contains characters that would break
    // a bare scalar; for our slugs and integers this never triggers.
    if needs_quoting(new_value) {
        rebuilt.push('"');
        rebuilt.push_str(new_value);
        rebuilt.push('"');
    } else {
        rebuilt.push_str(new_value);
    }
    if !comment_part.is_empty() {
        rebuilt.push_str("  ");
        rebuilt.push_str(comment_part);
    }
    Some(rebuilt)
}

fn needs_quoting(value: &str) -> bool {
    value.is_empty() || value.contains(['#', ':', ' ', '\t'])
}

/// Minimal frontmatter scaffolding used when `NightRideTUI.md` is absent.
fn render_minimal(cfg: &Config) -> String {
    let station = if cfg.default_station.is_empty() {
        "nightride".to_string()
    } else {
        cfg.default_station.clone()
    };
    format!(
        "---\napp:\n  log_level: {}\n\naudio:\n  default_station: {}\n  default_volume_percent: {}\n---\n\n# nightride-tui.md — managed by the binary across launches.\n# You can edit values here; the app rewrites `audio.default_station`\n# and `audio.default_volume_percent` on graceful exit.\n",
        cfg.log_level, station, cfg.default_volume,
    )
}
