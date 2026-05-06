// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Audio sub-tree: HTTP transport, ICY demux, symphonia decode, rodio
//! output sink, supervisor task, jittered-backoff reconnect machinery.
//!
//! ## State machine
//!
//! ```text
//! Idle ── Play ──→ Connecting ── stream ready ──→ Streaming
//!   ↑                ↓                                ↓
//!   └─ Stop ─────── Error ←── HardCodec/Cancelled ── Reconnecting
//!                    ↑                                ↑
//!                    └──── retry exhausted ──────────┘
//! ```
//!
//! ## Error class table (consumed by `reconnect::classify`)
//!
//! | Error variant                          | Class             |
//! |----------------------------------------|-------------------|
//! | `Io { source: UnexpectedEof }`          | `SoftEof`         |
//! | `Io { source: ... }` (other kinds)      | `HardNetwork`     |
//! | `Network { source: reqwest::Error }`    | `HardNetwork`     |
//! | `UpstreamUnavailable { detail }`        | `HardNetwork`     |
//! | `NetworkRejected { detail }`            | `HardCodec`       |
//! | `Decode { source: symphonia::Error }`   | `HardCodec`       |
//! | `Config` / `ConfigInvalid`              | `HardCodec`       |
//! | `Audio { ... }`                         | `HardCodec`       |
//! | `Validation` / `NotFound`               | `HardCodec`       |
//! | `Cancelled { ... }`                     | `HardCancelled`   |
//!
//! ## Volume invariant
//!
//! `rodio::Sink::set_volume` is the **only** volume mutation site. Custom
//! `Source::next` impls pass samples through verbatim. Documented in
//! `source.rs`. This eliminates the dual-application bug that was
//! introduced in commit `91d9855`.
//!
//! ## Cancellation hierarchy
//!
//! ```text
//! root token
//!  └── audio_token (supervisor)
//!       └── stream_token (per attached stream)
//! ```
//!
//! `root.cancel()` unwinds the supervisor + active stream + decode thread.
//! `stream_token.cancel()` unwinds only the current stream so a new
//! station can be attached without tearing the supervisor down.

use std::time::Instant;

pub(crate) mod decode;
pub(crate) mod hls;
pub(crate) mod http;
pub(crate) mod reconnect;
pub(crate) mod source;
pub(crate) mod supervisor;

pub(crate) use supervisor::supervisor;

use crate::config::TransportFormat;
use crate::station::Station;

/// Commands the UI sends to the audio supervisor.
#[derive(Debug, Clone)]
pub enum AudioCommand {
    /// Start playing the given station from scratch.
    Play(&'static Station),
    /// Set output volume in 0..=100 percent. Clamped on receipt.
    SetVolume(u8),
    /// Switch to a different station with the given input format.
    SetStation(&'static Station, TransportFormat),
}

/// Events the audio supervisor emits to the UI.
///
/// Three variants:
///
/// - [`AudioEvent::Metadata`] — a fresh ICY-metaint frame parsed into
///   [`crate::metadata::Metadata`]. Emitted whenever the upstream
///   stream rotates the `StreamTitle` value.
/// - [`AudioEvent::ConnectionState`] — supervisor lifecycle transition
///   (Idle → Connecting → Streaming → Reconnecting → Error).
/// - [`AudioEvent::Error`] — terminal failure surfaced to the UI for
///   display.
///
/// # Examples
///
/// ```
/// use nightride_tui::audio::{AudioEvent, ConnectionState};
/// use nightride_tui::metadata::Metadata;
///
/// let evt = AudioEvent::Metadata(Metadata {
///     artist: Some("Perturbator".to_string()),
///     title: Some("Sentient".to_string()),
///     raw: None,
/// });
/// assert!(matches!(evt, AudioEvent::Metadata(_)));
///
/// let state = AudioEvent::ConnectionState(ConnectionState::Idle);
/// assert!(matches!(state, AudioEvent::ConnectionState(ConnectionState::Idle)));
/// ```
#[derive(Debug, Clone)]
pub enum AudioEvent {
    /// Now-playing metadata snapshot extracted from the ICY stream.
    Metadata(crate::metadata::Metadata),
    /// Connection state change.
    ConnectionState(ConnectionState),
    /// Terminal failure surfaced to the UI for display.
    Error(String),
}

/// Lifecycle state of the active stream.
///
/// State machine: `Idle → Connecting → Streaming → Reconnecting →
/// {Streaming | Error}`. Cancel from any state returns to `Idle`.
///
/// # Examples
///
/// ```
/// use nightride_tui::audio::ConnectionState;
///
/// let idle = ConnectionState::Idle;
/// assert!(matches!(idle, ConnectionState::Idle));
///
/// let err = ConnectionState::Error {
///     station: "nightride",
///     detail: "stream timeout".to_string(),
/// };
/// match err {
///     ConnectionState::Error { station, .. } => assert_eq!(station, "nightride"),
///     _ => panic!("expected Error variant"),
/// }
/// ```
#[derive(Debug, Clone)]
pub enum ConnectionState {
    /// No stream active.
    Idle,
    /// Connecting to the upstream URL.
    Connecting {
        /// Slug of the station being connected.
        station: &'static str,
        /// Wall-clock instant at which the connection attempt started.
        started_at: Instant,
    },
    /// Stream connected and decoding.
    Streaming {
        /// Slug of the active station.
        station: &'static str,
        /// Wall-clock instant at which the stream began emitting samples.
        started_at: Instant,
    },
    /// Mid-stream failure; retry attempt `attempt`.
    Reconnecting {
        /// Slug of the affected station.
        station: &'static str,
        /// Retry counter (1-based for display).
        attempt: u32,
    },
    /// Retries terminated or a non-recoverable error surfaced.
    Error {
        /// Slug of the affected station.
        station: &'static str,
        /// Human-readable reason.
        detail: String,
    },
}
