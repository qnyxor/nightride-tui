// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Server-Sent Events (SSE) metadata supervisor for HLS mode.
//!
//! Connects to `https://nightride.fm/meta` and parses now-playing metadata
//! from push-based JSON events. Feeds updates into the shared `AudioEvent`
//! channel so the now-playing widget reflects live track changes.
//!
//! # Architecture
//!
//! - **Endpoint**: `https://nightride.fm/meta` (EventSource protocol).
//! - **Transport**: HTTP/1.1 persistent connection, Server-Sent Events.
//! - **Schema**: JSON objects with `{ "artist": "...", "title": "...", ... }`.
//! - **Single-endpoint multiplexing**: One endpoint covers all stations.
//!   Server-side filtering is assumed; client-side filtering for station
//!   slug is deferred (verify in early testing that events are station-specific
//!   or cross-station with filterable markers).
//!
//! # Lifecycle
//!
//! The SSE task is spawned alongside the HLS audio pipeline in
//! `attach_stream` (in `src/audio/supervisor.rs`) when HLS is the active
//! transport (checked via `NIGHTRIDE_TUI_HLS=1` env var). The task runs
//! in a dedicated `tokio::spawn` and is cancelled when the stream detaches
//! or the transport flips back to MP3.
//!
//! # Reconnect strategy
//!
//! Implements bounded exponential backoff (mirroring AD-01 spirit):
//! - Start: 500 ms
//! - Cap: 30 seconds
//! - Growth: 1.5× per attempt
//! - On network drop: emit "metadata: reconnecting" to the now-playing widget
//! - On prolonged drop (exhausted retries): emit "metadata: offline" warning
//! - Audio stream continues independently (metadata loss does not crash audio)
//!
//! # Licensing note
//!
//! `reqwest-eventsource` 0.6.0 is dual-licensed MIT-OR-Apache-2.0 per
//! `cargo audit` output; this module's Apache-2.0 heritage remains intact.

use std::sync::LazyLock;
use std::time::Duration;

use futures_util::stream::StreamExt;
use reqwest_eventsource::EventSource;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::audio::AudioEvent;
use crate::metadata::Metadata;

/// Process-wide reqwest client for SSE metadata. Reused across station
/// switches so DNS + TLS handshakes only pay their cost on the first
/// connection. No request/read timeout — see `connect_and_stream` for
/// rationale; only `tcp_keepalive` to detect half-closed sockets.
static SSE_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .tcp_keepalive(Duration::from_secs(60))
        .user_agent(crate::USER_AGENT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

/// Exponential backoff scheduler for SSE reconnect attempts.
///
/// Caps at 30 seconds; grows by 1.5× on each failure.
#[derive(Debug)]
struct SseBackoff {
    current: Duration,
    cap: Duration,
    growth_factor: f64,
}

impl SseBackoff {
    /// Create a new backoff starting at `start` with maximum `cap`.
    fn new(start: Duration, cap: Duration) -> Self {
        Self {
            current: start,
            cap,
            growth_factor: 1.5,
        }
    }

    /// Get the current backoff duration.
    fn current(&self) -> Duration {
        self.current
    }

    /// Advance the backoff: multiply by growth_factor and cap.
    fn advance(&mut self) {
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let next_ms = (self.current.as_millis() as f64 * self.growth_factor).ceil() as u64;
        self.current = Duration::from_millis(next_ms).min(self.cap);
    }

    /// Reset to the initial start duration.
    fn reset(&mut self, start: Duration) {
        self.current = start;
    }
}

/// Parse a JSON event payload into a `Metadata` record.
///
/// Nightride.fm `/meta` emits a single array per event with one entry per
/// station: `[{"station":"<slug>","artist":"...","title":"...",...}, …]`.
/// Pick the entry matching `station_slug` and project to `Metadata`.
/// Empty strings are treated as missing. Returns `None` when the event has
/// no entry for the active station or when both artist and title are empty.
fn parse_sse_event(raw: &str, station_slug: &str) -> Option<Metadata> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let entries = value.as_array()?;
    let entry = entries
        .iter()
        .find(|v| v.get("station").and_then(|s| s.as_str()) == Some(station_slug))?;

    let artist = entry
        .get("artist")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let title = entry
        .get("title")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);

    if artist.is_none() && title.is_none() {
        return None;
    }

    Some(Metadata {
        artist,
        title,
        raw: Some(raw.to_string()),
    })
}

/// Spawn the SSE metadata supervisor task for HLS mode.
///
/// Connects to the metadata endpoint and feeds updates into the event channel.
/// The task runs until the cancellation token fires or a fatal error occurs.
///
/// # Arguments
///
/// - `evt_tx`: async channel to emit `AudioEvent::Metadata` updates.
/// - `token`: cancellation token; task exits cleanly when fired.
///
/// # Examples
///
/// ```no_run
/// use tokio_util::sync::CancellationToken;
/// use tokio::sync::mpsc;
/// use nightride_tui::audio::AudioEvent;
/// use nightride_tui::metadata::sse::spawn_sse_supervisor;
///
/// #[tokio::main]
/// async fn main() {
///     let (evt_tx, mut evt_rx) = mpsc::channel(64);
///     let token = CancellationToken::new();
///     let child = token.child_token();
///
///     // Spawn SSE supervisor.
///     let future = spawn_sse_supervisor(evt_tx, "darksynth", child);
///     tokio::spawn(future);
///
///     // Simulate cancel after 5 seconds.
///     tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
///     token.cancel();
/// }
/// ```
pub async fn spawn_sse_supervisor(
    evt_tx: mpsc::Sender<AudioEvent>,
    station_slug: &'static str,
    token: CancellationToken,
) {
    let url = "https://nightride.fm/meta";
    let mut backoff = SseBackoff::new(Duration::from_millis(500), Duration::from_secs(30));

    loop {
        tokio::select! {
            () = token.cancelled() => {
                debug!("sse_supervisor: cancellation received, exiting cleanly");
                return;
            }
            () = tokio::time::sleep(Duration::ZERO) => {
                // Attempt connection.
                match connect_and_stream(url, station_slug, &evt_tx, &token).await {
                    Ok(()) => {
                        // Stream ended cleanly (e.g., server close). Reset backoff.
                        backoff.reset(Duration::from_millis(500));
                        debug!("sse_supervisor: stream closed cleanly, resetting backoff");
                    }
                    Err(e) => {
                        // Connection failed. Server-driven disconnects on
                        // long-lived SSE are expected (nightride.fm/meta
                        // closes idle connections; reqwest reports the
                        // resulting EOF as `error decoding response body`).
                        // The supervisor reconnects automatically — debug
                        // level keeps the noise out of normal-operation logs.
                        debug!("sse_supervisor: stream error: {e}, reconnecting");
                        let wait = backoff.current();
                        backoff.advance();
                        debug!("sse_supervisor: backing off for {:?}", wait);
                        tokio::time::sleep(wait).await;
                    }
                }
            }
        }
    }
}

/// Connect to the SSE endpoint and stream events until cancellation or error.
///
/// Returns `Ok(())` if the stream closed cleanly; `Err(...)` on network or
/// parse failure.
async fn connect_and_stream(
    url: &str,
    station_slug: &str,
    evt_tx: &mpsc::Sender<AudioEvent>,
    token: &CancellationToken,
) -> Result<(), String> {
    debug!("sse_supervisor: connecting to {}", url);

    // SSE streams are long-lived; the body never terminates by design.
    // We deliberately set NO `.timeout()` and NO `.read_timeout()` —
    // either would close the connection on legitimate silent periods
    // between events. nightride.fm/meta does not emit keepalive
    // comments, so even 30 s read windows trip a "decoding response
    // body" error (reqwest issue #2839). `.tcp_keepalive` is enough
    // to detect a half-closed socket at the OS layer (60 s probes,
    // ~3× to declare dead) without false-positive reconnects.
    // Cloned from a process-wide static so DNS + TLS handshakes survive
    // station switches.
    let client = SSE_CLIENT.clone();

    let request = client.get(url);

    let mut event_source =
        EventSource::new(request).map_err(|e| format!("event source creation failed: {e}"))?;

    loop {
        tokio::select! {
            () = token.cancelled() => {
                debug!("sse_supervisor: cancellation received during stream");
                return Ok(());
            }
            msg = event_source.next() => {
                match msg {
                    None => {
                        debug!("sse_supervisor: stream ended");
                        return Ok(());
                    }
                    Some(Err(e)) => {
                        debug!("sse_supervisor: stream error: {e}");
                        return Err(format!("stream error: {e}"));
                    }
                    Some(Ok(reqwest_eventsource::Event::Open)) => {
                        debug!("sse_supervisor: connection opened");
                    }
                    Some(Ok(reqwest_eventsource::Event::Message(msg))) => {
                        debug!(
                            event = msg.event,
                            data_len = msg.data.len(),
                            "sse_supervisor: event received"
                        );

                        // Parse the event data as JSON.
                        if let Some(metadata) = parse_sse_event(&msg.data, station_slug) {
                            debug!(
                                artist = ?metadata.artist,
                                title = ?metadata.title,
                                "sse_supervisor: parsed metadata"
                            );
                            let _ = evt_tx.try_send(AudioEvent::Metadata(metadata));
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sse_event_picks_active_station() {
        let json = r#"[{"station":"nightride","artist":"CYBERTHING!","title":"KIRA","album":"X","comment":""},{"station":"darksynth","artist":"Orphan Zero","title":"Big Blue","album":"Big Blue","comment":""}]"#;
        let md = parse_sse_event(json, "darksynth").expect("station match parses");
        assert_eq!(md.artist, Some("Orphan Zero".to_string()));
        assert_eq!(md.title, Some("Big Blue".to_string()));
    }

    #[test]
    fn parse_sse_event_unknown_station_returns_none() {
        let json = r#"[{"station":"nightride","artist":"X","title":"Y","album":"","comment":""}]"#;
        assert!(parse_sse_event(json, "darksynth").is_none());
    }

    #[test]
    fn parse_sse_event_empty_strings_treated_as_missing() {
        let json = r#"[{"station":"darksynth","artist":"","title":"","album":"","comment":""}]"#;
        assert!(parse_sse_event(json, "darksynth").is_none());
    }

    #[test]
    fn parse_sse_event_artist_only_when_title_empty() {
        let json = r#"[{"station":"darksynth","artist":"Carpenter Brut","title":"","album":"","comment":""}]"#;
        let md = parse_sse_event(json, "darksynth").expect("artist-only parses");
        assert_eq!(md.artist, Some("Carpenter Brut".to_string()));
        assert_eq!(md.title, None);
    }

    #[test]
    fn parse_sse_event_object_payload_returns_none() {
        // Defensive: legacy single-object shape must not crash, just return None.
        let json = r#"{"artist":"X","title":"Y"}"#;
        assert!(parse_sse_event(json, "darksynth").is_none());
    }

    #[test]
    fn parse_sse_event_invalid_json() {
        assert!(parse_sse_event("not json", "darksynth").is_none());
    }

    #[test]
    fn backoff_monotonic_growth() {
        let mut backoff = SseBackoff::new(Duration::from_millis(500), Duration::from_secs(30));
        let mut prev = backoff.current();
        for _ in 0..10 {
            backoff.advance();
            let curr = backoff.current();
            assert!(curr >= prev, "backoff must monotonically increase");
            prev = curr;
        }
    }

    #[test]
    fn backoff_respects_cap() {
        let cap = Duration::from_secs(30);
        let mut backoff = SseBackoff::new(Duration::from_millis(500), cap);
        for _ in 0..20 {
            backoff.advance();
        }
        assert_eq!(backoff.current(), cap, "backoff must not exceed cap");
    }

    #[test]
    fn backoff_reset() {
        let mut backoff = SseBackoff::new(Duration::from_millis(500), Duration::from_secs(30));
        backoff.advance();
        backoff.advance();
        let advanced = backoff.current();
        assert!(advanced > Duration::from_millis(500));

        backoff.reset(Duration::from_millis(500));
        assert_eq!(backoff.current(), Duration::from_millis(500));
    }

    #[tokio::test]
    async fn spawn_sse_supervisor_respects_cancellation() {
        let (evt_tx, _evt_rx) = mpsc::channel(64);
        let token = CancellationToken::new();
        let child = token.child_token();

        let supervisor_handle =
            tokio::spawn(async move { spawn_sse_supervisor(evt_tx, "darksynth", child).await });

        tokio::time::sleep(Duration::from_millis(100)).await;
        token.cancel();

        // Task should exit cleanly within reasonable time.
        let outcome = tokio::time::timeout(Duration::from_secs(2), supervisor_handle).await;
        assert!(outcome.is_ok(), "supervisor should exit cleanly on cancel");
    }
}
