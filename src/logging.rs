// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! File-only logging initializer.
//!
//! There is no console layer. The TUI cannot
//! interleave its render with stderr without smearing frames; debugging
//! happens by tailing the rolling daily file at
//! `${state_dir}/nightride/log/nightride.log.YYYY-MM-DD`.
//!
//! Rotation is daily; retention is 7 days, swept once at process start.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use directories::ProjectDirs;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::{NightrideError, Result};

/// Daily rotation file prefix. tracing-appender appends `.YYYY-MM-DD`.
const LOG_FILE_PREFIX: &str = "nightride.log";

/// Retention horizon — files older than this are removed at startup.
const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Build a filter directive that silences third-party crates.
fn build_filter_directive(user_level: &str) -> String {
    format!(
        "{user_level},nightride_tui={user_level},symphonia_format_isomp4=warn,symphonia_bundle_mp3=warn,symphonia_core=warn,hyper=warn,hyper_util=warn,reqwest=warn,rustls=warn,h2=warn"
    )
}

/// Initialize tracing with a single file layer.
///
/// `level` accepts any string `tracing_subscriber::EnvFilter` parses
/// (`off`, `error`, `warn`, `info`, `debug`, `trace`, or per-target
/// directives like `nightride_tui=debug,reqwest=warn`).
///
/// `log_dir` overrides the default state directory; when `None`, the
/// directory resolves to the platform state dir per `directories`.
///
/// Returns a [`WorkerGuard`] that the caller MUST keep alive for the
/// lifetime of the process — dropping it flushes the appender and stops
/// background writes.
///
/// # Errors
/// Returns [`NightrideError::Io`] if the log directory cannot be created
/// and [`NightrideError::Config`] if the level string fails to parse or
/// the subscriber registration fails.
pub fn init_logging(level: &str, log_dir: Option<PathBuf>) -> Result<WorkerGuard> {
    let dir = log_dir.unwrap_or_else(default_log_dir);
    std::fs::create_dir_all(&dir).map_err(|err| NightrideError::Io {
        op: "logging::init::create_dir",
        source: err,
    })?;

    sweep_old_files(&dir, RETENTION);

    let appender = rolling::daily(&dir, LOG_FILE_PREFIX);
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let filter_directive = build_filter_directive(level);
    let filter = EnvFilter::try_new(&filter_directive).map_err(|err| {
        NightrideError::config_invalid(
            "logging::init::parse_filter",
            format!("invalid log level {level:?}: {err}"),
        )
    })?;

    let file_layer = fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false);

    // `try_init` instead of `init` so a second call (e.g. from a test)
    // returns an error rather than panicking the host process.
    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .try_init()
        .map_err(|err| {
            NightrideError::config_invalid("logging::init::subscriber_init", err.to_string())
        })?;

    Ok(guard)
}

/// Resolve the canonical state directory for log output.
///
/// `directories` does not expose `state_dir`, so we use `data_dir`
/// (Linux: `~/.local/share/nightride/log`, macOS:
/// `~/Library/Application Support/nexus.qnyxor.nightride/log`).
/// Fallback is `/tmp/nightride/log` when `ProjectDirs` returns None
/// (stripped sandbox without `$HOME`).
fn default_log_dir() -> PathBuf {
    ProjectDirs::from("nexus", "qnyxor", "nightride").map_or_else(
        || PathBuf::from("/tmp/nightride/log"),
        |d| d.data_dir().join("log"),
    )
}

/// Best-effort cleanup of files older than `horizon`. Failures are
/// silently ignored — losing access to one stale log file is not worth
/// blocking process startup, and tracing is not up yet at this point.
fn sweep_old_files(dir: &Path, horizon: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age > horizon {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RETENTION, default_log_dir, sweep_old_files};
    use std::fs;
    use std::time::{Duration, SystemTime};

    #[test]
    fn default_dir_resolves() {
        let dir = default_log_dir();
        assert!(dir.ends_with("log"));
    }

    #[test]
    fn retention_is_seven_days() {
        assert_eq!(RETENTION, Duration::from_secs(7 * 24 * 60 * 60));
    }

    /// Sweep removes files older than the horizon and leaves fresh ones
    /// alone. Uses `File::set_modified` from std (1.75+) to back-date the
    /// stale fixture without pulling a `filetime` dep.
    #[test]
    fn sweep_removes_stale_files() {
        let tmp = tempdir();
        let stale = tmp.join("nightride.log.1999-01-01");
        let fresh = tmp.join("nightride.log.2099-12-31");
        fs::write(&stale, b"stale").unwrap();
        fs::write(&fresh, b"fresh").unwrap();

        let thirty_days_ago = SystemTime::now() - Duration::from_secs(30 * 24 * 60 * 60);
        let f = fs::File::options()
            .write(true)
            .open(&stale)
            .expect("open stale");
        f.set_modified(thirty_days_ago).expect("set_modified");
        drop(f);

        sweep_old_files(&tmp, RETENTION);

        assert!(!stale.exists(), "stale file must be removed");
        assert!(fresh.exists(), "fresh file must remain");
    }

    /// Inline tempdir helper so we don't pull `tempfile` into deps just
    /// for one test. Manual cleanup is acceptable in the test target.
    fn tempdir() -> std::path::PathBuf {
        let base = std::env::temp_dir();
        let unique = format!(
            "nightride-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        let dir = base.join(unique);
        fs::create_dir_all(&dir).expect("create tempdir");
        dir
    }

    #[test]
    fn filter_silences_third_party_at_info_level() {
        let directive = super::build_filter_directive("info");
        assert!(directive.contains("nightride_tui=info"));
        assert!(directive.contains("symphonia_format_isomp4=warn"));
        assert!(directive.contains("hyper=warn"));
        assert!(directive.contains("reqwest=warn"));
    }
}
