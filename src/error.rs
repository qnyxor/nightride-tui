// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Domain error type for the NightRideTUI library.
//!
//! Single library-side enum derived via `thiserror::Error`. Each variant
//! carries an `op: &'static str` tag identifying the call site
//! (`module::function` form) so log lines correlate without heap allocation.
//! Wrappable variants preserve the upstream chain via `#[source]` on a typed
//! source field; synthesised variants (no upstream type — URL allowlist
//! rejection, malformed frontmatter) carry `detail: String` instead.
//!
//! Construction goes through the inherent helpers (`network`, `network_rejected`,
//! `decode`, `config`, `config_invalid`, `audio`, ...) so the `op` tag and the
//! source remain co-located at every call site.
//!
//! Error messages are lowercase with no trailing punctuation.

use thiserror::Error;

/// Project-wide `Result` alias.
pub type Result<T> = std::result::Result<T, NightrideError>;

/// Credential scrubber applied at error construction.
///
/// Synthesised `detail: String` fields can carry user-controlled URLs
/// or token-like values verbatim — a stream URL with embedded
/// `user:pass@`, an HTTP `Authorization: Bearer …` snippet surfaced
/// from a transport diagnostic. To keep secrets out of crash dumps,
/// log files, and on-screen error toasts, every detail-bearing
/// constructor pipes its input through [`scrub::redact_credentials`]:
///
/// - URL userinfo (`scheme://user:pass@host…`) → `scheme://***@host…`
/// - HTTP bearer headers (`Bearer XYZ`) → `Bearer ***`
///
/// Query-string token keys (`?token=…`, `?key=…`) need a real URL
/// parser to handle reliably and are intentionally out of scope here;
/// they remain a known carry-over for the next supply-chain pass.
mod scrub {
    use std::borrow::Cow;

    /// Apply [`redact_url_userinfo`] then [`redact_bearer`] to `input`.
    /// Borrows when no credential-shaped substring is present.
    pub(super) fn redact_credentials(input: &str) -> Cow<'_, str> {
        let has_url = input.contains("://");
        let has_bearer = input.contains("Bearer ");
        if !has_url && !has_bearer {
            return Cow::Borrowed(input);
        }
        let url_pass: Cow<'_, str> = if has_url {
            redact_url_userinfo(input)
        } else {
            Cow::Borrowed(input)
        };
        if !has_bearer {
            return url_pass;
        }
        match redact_bearer(url_pass.as_ref()) {
            Cow::Owned(o) => Cow::Owned(o),
            Cow::Borrowed(_) => url_pass,
        }
    }

    /// Replace `user:pass@` (or any `userinfo@`) in URL authorities with
    /// `***@`. Returns `Cow::Borrowed` when no userinfo is found.
    fn redact_url_userinfo(input: &str) -> Cow<'_, str> {
        let mut parts = input.split("://");
        let Some(first) = parts.next() else {
            return Cow::Borrowed(input);
        };
        let mut out = String::with_capacity(input.len());
        out.push_str(first);
        let mut changed = false;
        for part in parts {
            out.push_str("://");
            let term = part
                .find(|c: char| {
                    c == '/' || c == '?' || c == '#' || c.is_whitespace() || c == '"' || c == '\''
                })
                .unwrap_or(part.len());
            let (authority, tail) = part.split_at(term);
            if let Some(at) = authority.rfind('@') {
                out.push_str("***");
                out.push_str(&authority[at..]);
                changed = true;
            } else {
                out.push_str(authority);
            }
            out.push_str(tail);
        }
        if changed {
            Cow::Owned(out)
        } else {
            Cow::Borrowed(input)
        }
    }

    /// Replace each `Bearer XYZ` with `Bearer ***` (token bounded by
    /// whitespace or quote). Returns `Cow::Borrowed` when no bearer
    /// header is found.
    fn redact_bearer(input: &str) -> Cow<'_, str> {
        const NEEDLE: &str = "Bearer ";
        let mut out = String::with_capacity(input.len());
        let mut rest = input;
        let mut changed = false;
        while let Some(pos) = rest.find(NEEDLE) {
            out.push_str(&rest[..pos]);
            out.push_str("Bearer ***");
            let after = &rest[pos + NEEDLE.len()..];
            let term = after
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
                .unwrap_or(after.len());
            rest = &after[term..];
            changed = true;
        }
        out.push_str(rest);
        if changed {
            Cow::Owned(out)
        } else {
            Cow::Borrowed(input)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::redact_credentials;

        #[test]
        fn plain_text_unchanged() {
            let s = "no credentials here";
            assert_eq!(redact_credentials(s), s);
            // Borrow-fast-path: no allocation.
            assert!(matches!(
                redact_credentials(s),
                std::borrow::Cow::Borrowed(_)
            ));
        }

        #[test]
        fn url_userinfo_redacted() {
            assert_eq!(
                redact_credentials("https://user:pass@stream.example.com/foo"),
                "https://***@stream.example.com/foo"
            );
        }

        #[test]
        fn url_without_userinfo_untouched() {
            let s = "host: https://stream.example.com/path";
            assert_eq!(redact_credentials(s), s);
        }

        #[test]
        fn bearer_token_redacted() {
            assert_eq!(
                redact_credentials("auth: Bearer abc123xyz here"),
                "auth: Bearer *** here"
            );
        }

        #[test]
        fn url_and_bearer_redacted_together() {
            let raw = "fetch https://u:p@h/x failed: Bearer secret was used";
            assert_eq!(
                redact_credentials(raw),
                "fetch https://***@h/x failed: Bearer *** was used"
            );
        }

        #[test]
        fn multiple_urls_all_redacted() {
            assert_eq!(
                redact_credentials("a https://u1:p1@h1/ b https://u2:p2@h2/"),
                "a https://***@h1/ b https://***@h2/"
            );
        }

        #[test]
        fn bearer_at_end_of_input() {
            assert_eq!(redact_credentials("Bearer abc"), "Bearer ***");
        }

        #[test]
        fn host_only_url_untouched() {
            assert_eq!(redact_credentials("https://host"), "https://host");
        }
    }
}

/// Pipe a detail string through the credential scrubber. Helper so every
/// detail-bearing constructor stays a single line.
fn scrub_detail<S: Into<String>>(s: S) -> String {
    scrub::redact_credentials(&s.into()).into_owned()
}

/// Domain error enum for every fallible operation in the library.
#[derive(Debug, Error)]
pub enum NightrideError {
    /// Filesystem, process I/O, or terminal-backend failure (crossterm wraps
    /// `std::io::Error` for raw-mode and alt-screen entry/exit).
    #[error("{op}: i/o error")]
    Io {
        /// Operation tag (for example `"config::load"`, `"ui::tui::enter"`).
        op: &'static str,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Network failure with an upstream `reqwest` source: DNS, TCP, TLS,
    /// HTTP status, or transport drop. The `#[source]` chain walks via
    /// `Display` so log triage sees the underlying transport reason.
    #[error("{op}: network error")]
    Network {
        /// Operation tag (for example `"audio::http::connect"`).
        op: &'static str,
        /// Underlying transport error.
        #[source]
        source: reqwest::Error,
    },

    /// Network-layer rejection without an upstream error: URL allowlist
    /// failure, scheme/host validation, malformed station registry entry.
    /// Synthesised string detail because no library type maps cleanly.
    #[error("{op}: network rejected: {detail}")]
    NetworkRejected {
        /// Operation tag.
        op: &'static str,
        /// Reason the request was rejected before transport.
        detail: String,
    },

    /// Upstream returned a transient unavailability — non-2xx HTTP
    /// status, or a 2xx response whose body / content-type is not
    /// playable audio (Icecast 404 with a `report.xml` body when a
    /// mountpoint's source disconnects, HTML error page, empty body).
    /// Treated as recoverable — the supervisor retries with backoff
    /// until the upstream source reconnects to the Icecast mountpoint.
    /// Distinct from [`NightrideError::NetworkRejected`] (terminal,
    /// allowlist / validation rejection) and from
    /// [`NightrideError::Network`] (transport-level error from
    /// reqwest).
    #[error("{op}: upstream unavailable: {detail}")]
    UpstreamUnavailable {
        /// Operation tag.
        op: &'static str,
        /// Status code, content-type mismatch, or other body diagnostic.
        detail: String,
    },

    /// Audio decoder failure (codec selection, malformed packet, EOF while
    /// not requested). Recoverable variants are handled internally; only
    /// terminal failures bubble through this variant. The `#[source]`
    /// preserves the symphonia error chain.
    #[error("{op}: decode error")]
    Decode {
        /// Operation tag (for example `"audio::decode::probe"`).
        op: &'static str,
        /// Underlying decoder error.
        #[source]
        source: symphonia::core::errors::Error,
    },

    /// YAML frontmatter parse or serialise failure. The `#[source]`
    /// preserves the underlying parser position + reason.
    #[error("{op}: config error")]
    Config {
        /// Operation tag (for example `"config::loader::parse_disk"`).
        op: &'static str,
        /// Underlying YAML error.
        #[source]
        source: serde_norway::Error,
    },

    /// Configuration semantic failure without a YAML parser source:
    /// malformed frontmatter delimiter, log-level filter parse, subscriber
    /// init, schema mismatch.
    #[error("{op}: config invalid: {detail}")]
    ConfigInvalid {
        /// Operation tag.
        op: &'static str,
        /// Synthesised reason surfaced to the operator.
        detail: String,
    },

    /// Validation failure on a known field with a known constraint
    /// (volume out of range, host not in allow-list, station slug unknown).
    #[error("{op}: validation failed on {field}: {detail}")]
    Validation {
        /// Operation tag (for example `"station::validate_registry"`).
        op: &'static str,
        /// Field name that failed validation.
        field: &'static str,
        /// Reason the value was rejected.
        detail: String,
    },

    /// Audio output device failure: no default device, unsupported sample
    /// format, `rodio::Sink` construction failure.
    ///
    /// `detail` carries the stringified upstream reason.
    #[error("{op}: audio error: {detail}")]
    Audio {
        /// Operation tag (for example `"audio::supervisor::open_sink"`).
        op: &'static str,
        /// Reason surfaced to the UI.
        detail: String,
    },

    /// Lookup target absent (station slug not in registry, log dir not
    /// resolvable).
    #[error("{op}: not found: {what}")]
    NotFound {
        /// Operation tag (for example `"station::by_slug"`).
        op: &'static str,
        /// Description of the missing entity.
        what: String,
    },

    /// Operation cancelled cooperatively (root token cancelled, task
    /// shutdown requested mid-flight).
    #[error("{op}: cancelled")]
    Cancelled {
        /// Operation tag (for example `"audio::stream_task"`).
        op: &'static str,
    },
}

impl NightrideError {
    /// Wrap a `reqwest::Error` with an operation tag.
    ///
    /// Prefer this over the `Network { op, source }` literal so the chain
    /// stays preserved consistently across call sites.
    #[must_use]
    pub fn network(op: &'static str, source: reqwest::Error) -> Self {
        Self::Network { op, source }
    }

    /// Build a synthesised network rejection (URL allowlist, scheme guard).
    /// Detail is run through the credential scrubber.
    #[must_use]
    pub fn network_rejected(op: &'static str, detail: impl Into<String>) -> Self {
        Self::NetworkRejected {
            op,
            detail: scrub_detail(detail),
        }
    }

    /// Build a transient upstream-unavailable error (HTTP 4xx/5xx, or a
    /// 2xx response whose body is not audio). Detail is run through the
    /// credential scrubber. Classified as `HardNetwork` so the
    /// supervisor retries with backoff.
    #[must_use]
    pub fn upstream_unavailable(op: &'static str, detail: impl Into<String>) -> Self {
        Self::UpstreamUnavailable {
            op,
            detail: scrub_detail(detail),
        }
    }

    /// Wrap a `symphonia::core::errors::Error` with an operation tag.
    #[must_use]
    pub fn decode(op: &'static str, source: symphonia::core::errors::Error) -> Self {
        Self::Decode { op, source }
    }

    /// Wrap a `serde_norway::Error` with an operation tag.
    #[must_use]
    pub fn config(op: &'static str, source: serde_norway::Error) -> Self {
        Self::Config { op, source }
    }

    /// Build a synthesised config error (no YAML parser source).
    /// Detail is run through the credential scrubber.
    #[must_use]
    pub fn config_invalid(op: &'static str, detail: impl Into<String>) -> Self {
        Self::ConfigInvalid {
            op,
            detail: scrub_detail(detail),
        }
    }

    /// Build an audio-output error from a stringified reason.
    /// Detail is run through the credential scrubber.
    #[must_use]
    pub fn audio(op: &'static str, detail: impl Into<String>) -> Self {
        Self::Audio {
            op,
            detail: scrub_detail(detail),
        }
    }

    /// Wrap a `std::io::Error` with an operation tag.
    #[must_use]
    pub fn io(op: &'static str, source: std::io::Error) -> Self {
        Self::Io { op, source }
    }

    /// Build a validation failure from a scrubbed detail string.
    /// Prefer this helper for any validation site whose detail may
    /// echo a user-provided URL or token.
    #[must_use]
    pub fn validation(op: &'static str, field: &'static str, detail: impl Into<String>) -> Self {
        Self::Validation {
            op,
            field,
            detail: scrub_detail(detail),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NightrideError;

    /// Error display is lowercase, no trailing punctuation, op-prefixed.
    #[test]
    fn display_format_is_op_prefixed() {
        let err = NightrideError::Validation {
            op: "test::display",
            field: "volume",
            detail: "out of range".to_string(),
        };
        let rendered = format!("{err}");
        assert!(rendered.starts_with("test::display:"));
        assert!(rendered.contains("validation failed on volume"));
        assert!(rendered.contains("out of range"));
    }

    /// Cancelled variant carries no detail beyond the op tag — matches the
    /// canonical "graceful shutdown" semantics of `CancellationToken`.
    #[test]
    fn cancelled_variant_minimal() {
        let err = NightrideError::Cancelled { op: "test::cancel" };
        assert_eq!(format!("{err}"), "test::cancel: cancelled");
    }

    /// `config_invalid` helper carries the op tag and synthesised detail.
    #[test]
    fn config_invalid_helper_preserves_op_and_detail() {
        let err = NightrideError::config_invalid("test::invalid", "bad fence");
        let rendered = format!("{err}");
        assert!(rendered.starts_with("test::invalid:"));
        assert!(rendered.contains("config invalid"));
        assert!(rendered.contains("bad fence"));
    }

    /// `network_rejected` helper carries op tag and synthesised detail.
    #[test]
    fn network_rejected_helper_preserves_op_and_detail() {
        let err = NightrideError::network_rejected("test::reject", "host not allowed");
        let rendered = format!("{err}");
        assert!(rendered.starts_with("test::reject:"));
        assert!(rendered.contains("network rejected"));
        assert!(rendered.contains("host not allowed"));
    }

    /// `audio` helper retains the synthesised-detail shape.
    #[test]
    fn audio_helper_carries_detail() {
        let err = NightrideError::audio("test::audio", "no default device");
        let rendered = format!("{err}");
        assert!(rendered.starts_with("test::audio:"));
        assert!(rendered.contains("audio error"));
        assert!(rendered.contains("no default device"));
    }
}
