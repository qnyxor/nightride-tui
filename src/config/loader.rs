// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! `CONFIG.md` loader + schema migrator.
//!
//! - [`load`] reads the user config (or the override path), strips the
//!   YAML frontmatter, parses it via `serde_norway`, projects the
//!   hierarchical [`super::RawConfig`] onto the flat [`super::Config`],
//!   and runs cross-field validation.
//! - [`ensure_schema`] writes the embedded template on first launch
//!   and merges missing keys into pre-existing files while preserving
//!   every existing leaf value.

use std::path::PathBuf;

use serde_norway::Value;

use crate::{NightrideError, Result};

use super::{
    Config, RawConfig, default_log_level, default_station, default_user_config_path,
    default_volume, validate,
};

/// Canonical schema embedded at build time. The repo's
/// `nightride-tui.md` is the source of truth — every key the binary
/// expects to see is documented there with a default and an inline
/// comment. The binary writes this template on first launch and
/// migrates pre-existing files when their schema falls behind.
const TEMPLATE: &str = include_str!("../../nightride-tui.md");

/// Frontmatter delimiter (YAML block fenced by `---` on its own line).
const FRONTMATTER_FENCE: &str = "---";

/// Load and validate the configuration file.
///
/// 1. Resolve the config path (CLI override → user config dir).
/// 2. Read UTF-8 contents.
/// 3. Strip the YAML frontmatter block.
/// 4. Deserialize via `serde_norway`.
/// 5. Run cross-field validation.
///
/// # Errors
/// Returns [`NightrideError::Io`] on filesystem failure,
/// [`NightrideError::Config`] / [`NightrideError::ConfigInvalid`] on
/// parse failure, and [`NightrideError::Validation`] on cross-field
/// failure.
pub fn load(override_path: Option<PathBuf>) -> Result<Config> {
    let path = override_path.unwrap_or_else(default_user_config_path);

    // CONFIG.md absent at runtime is a recoverable state — fall back to
    // built-in defaults so first-time users can run the binary without
    // shipping a config file.
    if !path.exists() {
        let cfg = Config::default();
        validate(&cfg)?;
        return Ok(cfg);
    }

    let raw = std::fs::read_to_string(&path).map_err(|err| NightrideError::Io {
        op: "config::load::read",
        source: err,
    })?;

    let frontmatter = extract_frontmatter(&raw)?;
    let raw_cfg: RawConfig = if frontmatter.trim().is_empty() {
        RawConfig::default()
    } else {
        serde_norway::from_str(frontmatter)
            .map_err(|err| NightrideError::config("config::load::parse", err))?
    };

    let cfg = project(raw_cfg);
    validate(&cfg)?;
    Ok(cfg)
}

/// Project the hierarchical [`RawConfig`] onto the flat [`Config`] the
/// rest of the library consumes. Each field falls back to its
/// canonical default when the disk value is missing or empty.
fn project(raw_cfg: RawConfig) -> Config {
    Config {
        default_station: raw_cfg
            .audio
            .default_station
            .filter(|s| !s.is_empty())
            .unwrap_or_else(default_station),
        default_volume: raw_cfg
            .audio
            .default_volume_percent
            .unwrap_or_else(default_volume),
        log_level: raw_cfg
            .app
            .log_level
            .filter(|s| !s.is_empty())
            .unwrap_or_else(default_log_level),
        log_dir: raw_cfg.app.log_dir,
    }
}

/// Extract the YAML block fenced by leading and trailing `---` lines.
///
/// Tolerates BOM-free UTF-8 only (per the contract). Returns the raw
/// YAML slice without the fences. If the file lacks frontmatter,
/// returns an empty slice so `serde_norway` deserializes the default
/// values.
pub(super) fn extract_frontmatter(raw: &str) -> Result<&str> {
    let trimmed = raw.trim_start_matches('\u{feff}');

    // Locate the opening fence, skipping any pre-frontmatter content
    // (HTML comment SPDX headers, blank lines). The fence is a `---`
    // standalone on its own line.
    let Some(opening) = find_fence_start(trimmed) else {
        // No frontmatter → defaults pass through.
        return Ok("");
    };

    // After the opening fence + its newline.
    let after_first_fence_idx = opening + FRONTMATTER_FENCE.len();
    let after_first_fence = trimmed[after_first_fence_idx..]
        .strip_prefix('\n')
        .unwrap_or(&trimmed[after_first_fence_idx..]);

    // Empty frontmatter: closing fence is the very next line.
    if after_first_fence.starts_with("---") {
        return Ok("");
    }

    match after_first_fence.find("\n---") {
        Some(pos) => Ok(&after_first_fence[..=pos]),
        None => Err(NightrideError::config_invalid(
            "config::extract_frontmatter",
            "unterminated frontmatter block",
        )),
    }
}

/// Find the byte offset of the first standalone `---` fence. Skips
/// HTML comments (`<!-- ... -->`) and blank lines that may precede
/// the YAML frontmatter (e.g. SPDX header blocks).
fn find_fence_start(content: &str) -> Option<usize> {
    let mut offset = 0usize;
    let mut in_html_comment = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if in_html_comment {
            if trimmed.contains("-->") {
                in_html_comment = false;
            }
        } else if trimmed.starts_with("<!--") {
            // Single-line `<!-- ... -->` or start of multi-line block.
            if !trimmed.contains("-->") {
                in_html_comment = true;
            }
        } else if trimmed == FRONTMATTER_FENCE {
            return Some(offset);
        } else if !trimmed.is_empty() {
            // Non-fence, non-comment, non-blank → no frontmatter ahead.
            return None;
        }
        offset += line.len() + 1; // +1 for the '\n' that `lines()` strips.
    }
    None
}

/// Ensure `nightride-tui.md` exists on disk and that its YAML schema
/// includes every key the embedded template expects.
///
/// Behaviour:
///
/// - File absent → write the embedded template verbatim (first launch
///   creates the on-disk config from the bundled default).
/// - File present + schema matches → no write.
/// - File present + missing keys → merge the template's key tree into
///   the on-disk YAML, **preserving every existing leaf value** while
///   adding the missing keys with their template defaults. The merge
///   round-trips through `serde_norway::Value`, which discards inline
///   comments — accepted trade-off; the canonical comments live in
///   the template, so post-migration the file carries them.
///
/// # Errors
/// Returns [`NightrideError::Io`] for filesystem failure and
/// [`NightrideError::Config`] for malformed YAML on disk.
pub fn ensure_schema(path: &std::path::Path) -> Result<()> {
    if !path.exists() {
        write_template(path)?;
        return Ok(());
    }

    let disk = std::fs::read_to_string(path).map_err(|err| NightrideError::Io {
        op: "config::ensure_schema::read",
        source: err,
    })?;
    let disk_front = extract_frontmatter(&disk)?;
    let template_front = extract_frontmatter(TEMPLATE)?;

    let disk_value: Value = if disk_front.trim().is_empty() {
        Value::Mapping(serde_norway::Mapping::default())
    } else {
        serde_norway::from_str(disk_front)
            .map_err(|err| NightrideError::config("config::ensure_schema::parse_disk", err))?
    };
    let template_value: Value = serde_norway::from_str(template_front)
        .map_err(|err| NightrideError::config("config::ensure_schema::parse_template", err))?;

    let (merged, changed) = merge_missing_keys(disk_value, &template_value);
    if !changed {
        return Ok(());
    }

    let merged_yaml = serde_norway::to_string(&merged)
        .map_err(|err| NightrideError::config("config::ensure_schema::serialize", err))?;
    let body = template_body(TEMPLATE);
    let new_doc = format!("---\n{merged_yaml}---\n{body}");
    std::fs::write(path, new_doc).map_err(|err| NightrideError::Io {
        op: "config::ensure_schema::write_merged",
        source: err,
    })?;
    Ok(())
}

/// First-launch helper: create parent directory if needed and write
/// the embedded template verbatim.
fn write_template(path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|err| NightrideError::Io {
                op: "config::ensure_schema::mkdir",
                source: err,
            })?;
        }
    }
    std::fs::write(path, TEMPLATE).map_err(|err| NightrideError::Io {
        op: "config::ensure_schema::write_template",
        source: err,
    })?;
    Ok(())
}

/// Recursively merge keys from `template` into `disk` while preserving
/// every existing leaf value on disk. Returns the merged value plus a
/// `changed` flag that's `true` iff at least one missing key was added.
fn merge_missing_keys(disk: Value, template: &Value) -> (Value, bool) {
    match (disk, template) {
        (Value::Mapping(mut disk_map), Value::Mapping(template_map)) => {
            let mut changed = false;
            for (k, tv) in template_map {
                if let Some(existing) = disk_map.get(k) {
                    // Recurse only when both sides are mappings.
                    if matches!((existing, tv), (Value::Mapping(_), Value::Mapping(_))) {
                        let owned = existing.clone();
                        let (merged, sub_changed) = merge_missing_keys(owned, tv);
                        if sub_changed {
                            disk_map.insert(k.clone(), merged);
                            changed = true;
                        }
                    }
                    // Otherwise keep the disk value verbatim.
                } else {
                    disk_map.insert(k.clone(), tv.clone());
                    changed = true;
                }
            }
            (Value::Mapping(disk_map), changed)
        }
        (disk_other, _) => (disk_other, false),
    }
}

/// Return the body of `doc` (everything after the closing `---` of the
/// frontmatter). Returns an empty string if `doc` has no frontmatter.
fn template_body(doc: &str) -> String {
    let after = doc
        .split_once("---\n")
        .and_then(|(_, rest)| rest.split_once("\n---"))
        .map(|(_, body)| body);
    match after {
        Some(b) => {
            let mut s = b.to_string();
            // Drop a single leading newline if any so we don't double up.
            if s.starts_with('\n') {
                s.remove(0);
            }
            s
        }
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_frontmatter, load};

    #[test]
    fn extract_frontmatter_handles_empty_doc() {
        assert_eq!(extract_frontmatter("# Just a body\n").unwrap(), "");
    }

    #[test]
    fn extract_frontmatter_returns_yaml_block() {
        let doc = "---\ndefault_volume: 30\n---\n# body\n";
        assert_eq!(extract_frontmatter(doc).unwrap(), "default_volume: 30\n");
    }

    /// Hierarchical CONFIG.md (matching the on-disk layout) parses
    /// audio.default_station + audio.default_volume_percent + app.log_level
    /// while silently ignoring sections v2 does not implement (network,
    /// theme, keymap).
    #[test]
    fn loads_hierarchical_config_md() {
        let dir = std::env::temp_dir().join(format!(
            "nightride-cfg-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("tempdir");
        let path = dir.join("CONFIG.md");
        std::fs::write(
            &path,
            "---\napp:\n  log_level: debug\naudio:\n  default_station: darksynth\n  default_volume_percent: 70\nnetwork:\n  connect_timeout_secs: 10\n---\n# body\n",
        )
        .expect("write fixture");

        let cfg = load(Some(path)).expect("hierarchical CONFIG.md loads");
        assert_eq!(cfg.default_station, "darksynth");
        assert_eq!(cfg.default_volume, 70);
        assert_eq!(cfg.log_level, "debug");
    }

    /// Empty `default_station: ""` falls back to the canonical default
    /// (matches the on-disk CONFIG.md shipped at v0.1).
    #[test]
    fn empty_default_station_falls_back() {
        let dir = std::env::temp_dir().join(format!(
            "nightride-cfg-test-empty-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("tempdir");
        let path = dir.join("CONFIG.md");
        std::fs::write(
            &path,
            "---\naudio:\n  default_station: \"\"\n  default_volume_percent: 40\n---\n",
        )
        .expect("write fixture");

        let cfg = load(Some(path)).expect("empty string falls back");
        assert_eq!(cfg.default_station, "nightride");
        assert_eq!(cfg.default_volume, 40);
    }

    #[test]
    fn extract_frontmatter_rejects_unterminated() {
        let doc = "---\ndefault_volume: 30\n# never closes\n";
        assert!(extract_frontmatter(doc).is_err());
    }

    #[test]
    fn missing_config_file_falls_back_to_defaults() {
        let cfg = load(Some(std::path::PathBuf::from("/nonexistent/CONFIG.md")))
            .expect("missing file falls back to defaults");
        assert_eq!(cfg.default_station, "nightride");
        assert_eq!(cfg.default_volume, 50);
    }
}
