// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! NightRideTUI library crate.
//!
//! Single-binary Rust TUI for the nightride.fm radio family. The binary
//! at `src/main.rs` is a thin shell (~20 LOC) that owns the root
//! [`tokio_util::sync::CancellationToken`], parses CLI args, and
//! delegates to [`run`].
//!
//! # Module map
//! - [`audio`] — supervisor + symphonia decode loop + VisualizerSource.
//! - [`cli`]   — clap-derived args + Run / InstallFont / ListStations.
//! - [`config`] — CONFIG.md frontmatter loader + validation.
//! - [`error`] — domain `NightrideError` (thiserror).
//! - [`logging`] — file-only tracing-subscriber + 7-day retention.
//! - [`metadata`] — ICY parser + History ring.
//! - [`station`] — 9-station registry + ALLOWED_STREAM_HOST + helpers.
//! - [`theme`] — Theme + GlyphSet + colour-token resolver.
//! - [`ui`]    — Tui + App + canonical async event loop.
//!
//! # Channel topology (constructed in [`run`])
//!
//! ```text
//!  ui task  ── AudioCommand (32) ─→ audio supervisor
//!  ui task  ←── AudioEvent (64) ── audio supervisor
//!  ui task  ←── Vec<f32> (1, drop-on-full) ── VisualizerSource
//! ```
//!
//! # Cancellation hierarchy
//!
//! ```text
//!  root_token (main.rs)
//!    ├── audio_token (supervisor child)
//!    │     └── stream_token (per active stream)
//!    └── ui_token (event/render loop child)
//! ```

#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::doc_markdown)]

pub mod cli;
pub mod config;
pub mod error;
pub mod theme;
pub mod update_check;

// Internal modules are `pub(crate)`; gated to `pub` under `cfg(test)`
// or `feature = "test-export"` for integration test access.
//
// `audio`, `metadata`, and `ui` remain `pub` because the integration
// tests under `tests/` import concrete types from each (e.g.
// `nightride_tui::audio::ConnectionState`, `nightride_tui::metadata::Metadata`,
// `nightride_tui::ui::App`). `logging` and `station` are purely internal
// and therefore `pub(crate)`.
pub mod audio;
pub(crate) mod logging;
pub mod metadata;
pub mod station;
pub mod ui;

pub use error::{NightrideError, Result};

/// Single source of truth for the HTTP `User-Agent` header sent on every
/// `reqwest` client in this crate.
///
/// Identifies the calling application to upstream operators per RFC 9110 §10.1.5,
/// making logs and abuse reports audit-friendly without any runtime allocation.
pub const USER_AGENT: &str = concat!(
    env!("CARGO_PKG_NAME"),
    "/",
    env!("CARGO_PKG_VERSION"),
    " (+",
    env!("CARGO_PKG_REPOSITORY"),
    ")",
);

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::audio::{AudioCommand, AudioEvent};
use crate::cli::CliArgs;

/// Initial visualizer width hint. Replaced on the first render tick by
/// the actual terminal width minus the volume mini-bar reservation.
const INITIAL_VISUALIZER_WIDTH: usize = 80;

/// Library entry point for the `Run` subcommand.
///
/// Wires the channel topology, spawns the audio supervisor + UI tasks,
/// and waits for either to exit. The first to exit cancels `root` so
/// the other unwinds cleanly.
///
/// # Errors
/// Returns [`NightrideError::Validation`] for registry / config
/// validation, [`NightrideError::Io`] for terminal init, and
/// [`NightrideError::Cancelled`] for any task-channel close while
/// the supervisor is still expected.
pub async fn run(args: CliArgs, root: CancellationToken) -> Result<()> {
    // 1. Defence in depth: validate the registry against ALLOWED_STREAM_HOST
    //    before any task spawns.
    station::validate_registry()?;

    // 2. First-launch creation + schema migration. The binary embeds the
    //    canonical `nightride-tui.md` via `include_str!`; if the user has
    //    no file we write the template; if their file is missing keys we
    //    add them without overwriting any leaf the user already set.
    let cfg_path = config::default_user_config_path();
    config::ensure_schema(&cfg_path)?;

    // 3. Load config + apply CLI overrides. The config-path and
    // log-level CLI flags were retired — power users edit
    // `nightride-tui.md` directly or set `RUST_LOG` to override
    // logging at runtime via tracing's standard env-var path.
    let mut cfg = config::load(Some(cfg_path.clone()))?;
    if let Some(slug) = args.station.as_deref() {
        cfg.default_station = slug.to_string();
    }

    // 4. Init logging. WorkerGuard MUST outlive every spawned task —
    //    we hold it in the local stack until run returns.
    //    Suffix the configured `log_level` with per-target overrides
    //    pinning every noisy third-party crate to `warn`. Symphonia in
    //    particular emits `info!` on every fMP4 atom it parses, which
    //    drowns our own `info!` lines — at one segment every 5 s that
    //    is several lines per second of unwanted output. Likewise
    //    reqwest / hyper / h2 narrate every connection lifecycle event
    //    at info. The user's `log_level` (default `info`) still drives
    //    our own crate; the suffix only attenuates the dependencies.
    let log_level = format!(
        "{},symphonia=warn,symphonia_format_isomp4=warn,symphonia_codec_aac=warn,\
         symphonia_bundle_mp3=warn,symphonia_metadata=warn,reqwest=warn,reqwest_eventsource=warn,\
         hyper=warn,hyper_util=warn,h2=warn,rustls=warn",
        cfg.log_level,
    );
    let _log_guard = logging::init_logging(&log_level, cfg.log_dir.clone())?;
    info!(
        version = env!("CARGO_PKG_VERSION"),
        station = %cfg.default_station,
        volume = cfg.default_volume,
        "nightride starting"
    );

    // Background HLS pre-warm: opens the TLS connection to the HLS
    // host and caches the init segment for the default station before
    // the operator interacts. Saves ~350 ms on the first attach. Fire-
    // and-forget, failure is non-fatal.
    if let Some(default_station) = crate::station::by_slug(&cfg.default_station) {
        audio::hls::prewarm(default_station);
    }

    // 5. Channel topology per design § Channel topology.
    let (cmd_tx, cmd_rx) = mpsc::channel::<AudioCommand>(32);
    let (evt_tx, evt_rx) = mpsc::channel::<AudioEvent>(64);
    let (amp_tx, amp_rx) = mpsc::channel::<Vec<f32>>(1);
    let (save_tx, save_rx) = mpsc::channel::<ui::FinalState>(8);
    let visualizer_width = Arc::new(AtomicUsize::new(INITIAL_VISUALIZER_WIDTH));

    // 6. Cancellation hierarchy. Audio + UI + save-debouncer are siblings;
    //    all descend from `root`.
    let audio_token = root.child_token();
    let ui_token = root.child_token();
    let save_token = root.child_token();

    // 7. Spawn save-debouncer: writes the latest state to disk after
    //    ~500 ms of quiet. Avoids one disk write per scroll-wheel tick.
    let save_path = cfg_path.clone();
    let save_base = cfg.clone();
    tokio::spawn(async move {
        save_debouncer(
            save_rx,
            save_path,
            save_base,
            std::time::Duration::from_millis(500),
            save_token,
        )
        .await;
    });

    // 8. Spawn audio supervisor.
    let mut audio_handle = tokio::spawn({
        let width = visualizer_width.clone();
        async move {
            audio::supervisor(cmd_rx, evt_tx, amp_tx, width, audio_token).await;
        }
    });

    // 9. Spawn UI loop.
    let ui_cfg = cfg.clone();
    let ui_width = visualizer_width.clone();
    let mut ui_handle = tokio::spawn(async move {
        ui::run(ui_cfg, ui_width, cmd_tx, evt_rx, amp_rx, save_tx, ui_token).await
    });

    // 10. Wait for either task to finish; the first one cancels root so
    //    the other unwinds cleanly. UI exit is the canonical path
    //    (user pressed q / Ctrl+C); audio exit is unexpected.
    let mut final_state: Option<ui::FinalState> = None;
    let ui_outcome: Result<()> = tokio::select! {
        ui_res = &mut ui_handle => {
            info!("ui task exited; cancelling root");
            root.cancel();
            let _ = audio_handle.await;
            match ui_res {
                Ok(Ok(state)) => {
                    final_state = Some(state);
                    Ok(())
                }
                Ok(Err(err)) => Err(err),
                Err(err) => {
                    warn!(err = %err, "ui join error");
                    Err(NightrideError::Cancelled { op: "lib::run::ui_join" })
                }
            }
        }
        audio_res = &mut audio_handle => {
            warn!("audio task exited unexpectedly; cancelling root");
            root.cancel();
            let _ = ui_handle.await;
            match audio_res {
                Ok(()) => Ok(()),
                Err(err) => {
                    warn!(err = %err, "audio join error");
                    Err(NightrideError::Cancelled { op: "lib::run::audio_join" })
                }
            }
        }
    };

    // 11. Persist final state (best-effort — never poisons the exit).
    //    Save-on-modify lands as a follow-up: the UI emits a save tick
    //    on every Action that mutates state and a debouncer task writes
    //    after 500 ms of quiet. The closing snapshot is enough to honour
    //    the session-cookie persistence contract on graceful exit: next
    //    launch resumes with the last station + volume.
    if let Some(state) = final_state {
        let mut to_save = cfg.clone();
        to_save.default_station = state.station_slug;
        to_save.default_volume = state.volume;
        if let Err(err) = config::save_state(&cfg_path, &to_save) {
            warn!(err = %err, path = %cfg_path.display(), "save_state failed");
        }
    }

    info!("nightride exiting cleanly");
    ui_outcome
}

/// Save-debouncer task. Receives [`ui::FinalState`] snapshots from the
/// UI; coalesces bursts (volume scroll, station spam) by waiting
/// `debounce` of quiet before writing. On root cancellation, flushes
/// the pending snapshot if any.
async fn save_debouncer(
    mut rx: mpsc::Receiver<ui::FinalState>,
    cfg_path: std::path::PathBuf,
    base_cfg: config::Config,
    debounce: std::time::Duration,
    token: CancellationToken,
) {
    let mut latest: Option<ui::FinalState> = None;
    loop {
        tokio::select! {
            () = token.cancelled() => {
                if let Some(state) = latest.take() {
                    flush_save(&cfg_path, &base_cfg, &state);
                }
                break;
            }
            maybe = rx.recv() => {
                let Some(state) = maybe else {
                    if let Some(state) = latest.take() {
                        flush_save(&cfg_path, &base_cfg, &state);
                    }
                    break;
                };
                latest = Some(state);
                // Drain further events for `debounce` before writing.
                loop {
                    tokio::select! {
                        () = tokio::time::sleep(debounce) => break,
                        next = rx.recv() => {
                            match next {
                                Some(s) => latest = Some(s),
                                None => break,
                            }
                        }
                    }
                }
                if let Some(state) = latest.as_ref() {
                    flush_save(&cfg_path, &base_cfg, state);
                }
            }
        }
    }
}

fn flush_save(path: &std::path::Path, base: &config::Config, state: &ui::FinalState) {
    let mut cfg = base.clone();
    cfg.default_station.clone_from(&state.station_slug);
    cfg.default_volume = state.volume;
    if let Err(err) = config::save_state(path, &cfg) {
        warn!(err = %err, path = %path.display(), "save_debouncer write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::USER_AGENT;

    /// Permanent invariant guard: USER_AGENT must resolve to the canonical
    /// `<name>/<version> (+<repo>)` form. Fails the build if someone bumps
    /// the version in Cargo.toml without noticing this assertion — the correct
    /// fix is to update the expected string here alongside the version bump.
    #[test]
    fn user_agent_resolves_correctly() {
        assert_eq!(
            USER_AGENT,
            "nightride-tui/1.1.0 (+https://github.com/qnyxor/nightride-tui)",
        );
    }
}
