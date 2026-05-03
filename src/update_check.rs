// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Background update-check: polls the GitHub releases API for a newer tag.
//!
//! This module is intentionally minimal — a single async function that makes
//! one HTTP request, parses the `tag_name` field, compares it to the running
//! binary version, and returns `Some(tag)` when an upgrade is available.
//! All failures are silenced; the indicator is best-effort and never blocks
//! startup or panics the process.
//!
//! # Version comparison
//!
//! [`is_newer`] implements a tiny inline semver comparator for `X.Y.Z` and
//! `vX.Y.Z` strings. The `semver` crate is intentionally **not** used to
//! keep the dependency footprint small.

use std::time::Duration;

/// GitHub releases API endpoint for this repository.
const RELEASES_LATEST_URL: &str =
    "https://api.github.com/repos/qnyxor/nightride-tui/releases/latest";

/// Network timeout for the update check. A generous but bounded limit so
/// slow connections don't stall startup for longer than the user would
/// notice the TUI appearing.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Compare two version strings without the `semver` crate.
///
/// Accepts both `vX.Y.Z` (with leading `v`) and `X.Y.Z` forms. Returns
/// `true` only when `remote` is strictly greater than `local`. Returns
/// `false` on any parse failure so a garbled release tag never produces
/// a false-positive upgrade prompt.
///
/// # Examples
///
/// ```
/// use nightride_tui::update_check::is_newer;
/// assert!(is_newer("1.0.1", "1.0.0"));
/// assert!(!is_newer("1.0.0", "1.0.0"));
/// assert!(!is_newer("0.9.9", "1.0.0"));
/// assert!(is_newer("v1.1.0", "1.0.5"));
/// assert!(!is_newer("garbage", "1.0.0"));
/// ```
#[must_use]
pub fn is_newer(remote: &str, local: &str) -> bool {
    parse_semver(remote)
        .zip(parse_semver(local))
        .is_some_and(|(r, l)| r > l)
}

/// Parse a `vX.Y.Z` or `X.Y.Z` string into a `(u64, u64, u64)` tuple.
/// Returns `None` on any parse failure.
fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = s.splitn(3, '.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    // The patch component may carry a pre-release suffix like `0-beta.1`;
    // we only care about the numeric prefix.
    let patch_str = parts.next()?;
    let patch_numeric: String = patch_str.chars().take_while(char::is_ascii_digit).collect();
    let patch = patch_numeric.parse::<u64>().ok()?;
    Some((major, minor, patch))
}

/// Extract the `tag_name` field from a GitHub releases API JSON response.
///
/// The response shape is stable and the field always appears near the top of
/// the object, so a lightweight string-scan is safer than pulling in a
/// serde_json dependency (which is not currently in `Cargo.toml`).
fn extract_tag_name(body: &str) -> Option<&str> {
    // Looking for: `"tag_name":"v1.2.3"` (with or without spaces around `:`).
    let key = "\"tag_name\"";
    let start = body.find(key)?;
    let after_key = &body[start + key.len()..];
    let colon = after_key.find(':')?;
    let after_colon = after_key[colon + 1..].trim_start();
    if !after_colon.starts_with('"') {
        return None;
    }
    let inner = &after_colon[1..];
    let end = inner.find('"')?;
    Some(&inner[..end])
}

/// Check whether a newer release exists on GitHub.
///
/// Makes a single GET to the GitHub releases API, parses the `tag_name`
/// field, and returns `Some(tag)` when the remote tag is strictly newer than
/// the running binary version. Returns `None` on any network or parse error.
///
/// This function **never panics** and **never returns an error** — all
/// failures are silenced so the caller can treat it as a fire-and-forget
/// background task.
///
/// # Examples
///
/// ```no_run
/// # async fn example() {
/// let client = reqwest::Client::builder()
///     .user_agent(format!("nightride-tui/{}", env!("CARGO_PKG_VERSION")))
///     .build()
///     .unwrap();
/// if let Some(tag) = nightride_tui::update_check::check_for_update(&client).await {
///     println!("new version available: {tag}");
/// }
/// # }
/// ```
pub async fn check_for_update(client: &reqwest::Client) -> Option<String> {
    let body = client
        .get(RELEASES_LATEST_URL)
        .timeout(TIMEOUT)
        .header(
            reqwest::header::USER_AGENT,
            format!("nightride-tui/{}", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;

    let tag = extract_tag_name(&body)?;
    let current = env!("CARGO_PKG_VERSION");
    if is_newer(tag, current) {
        Some(tag.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_tag_name, is_newer};

    #[test]
    fn is_newer_patch_bump() {
        assert!(is_newer("1.0.1", "1.0.0"));
    }

    #[test]
    fn is_newer_same_version_is_false() {
        assert!(!is_newer("1.0.0", "1.0.0"));
    }

    #[test]
    fn is_newer_older_patch_is_false() {
        assert!(!is_newer("0.9.9", "1.0.0"));
    }

    #[test]
    fn is_newer_with_v_prefix() {
        assert!(is_newer("v1.1.0", "1.0.5"));
    }

    #[test]
    fn is_newer_garbage_is_false() {
        assert!(!is_newer("garbage", "1.0.0"));
    }

    #[test]
    fn extract_tag_name_parses_github_response() {
        let body = r#"{"url":"https://api.github.com/repos/qnyxor/nightride-tui/releases/1","tag_name":"v1.2.3","name":"v1.2.3"}"#;
        assert_eq!(extract_tag_name(body), Some("v1.2.3"));
    }

    #[test]
    fn extract_tag_name_missing_field_returns_none() {
        let body = r#"{"url":"https://example.com","name":"something"}"#;
        assert!(extract_tag_name(body).is_none());
    }

    #[test]
    fn extract_tag_name_with_spaces_around_colon() {
        let body = r#"{"tag_name" : "v2.0.0","other":"x"}"#;
        assert_eq!(extract_tag_name(body), Some("v2.0.0"));
    }
}
