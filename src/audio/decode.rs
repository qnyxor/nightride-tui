// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Symphonia decode loop driving the per-stream sample pump.
//!
//! Runs in its own `std::thread::spawn` thread (the cpal callback can't
//! cross an `await`, so the supervisor lives in tokio while the decoder
//! lives on a blocking thread). Communicates with the supervisor via the
//! event channel and with the audio sink via the pre-allocated
//! `SyncSender<i16>` sample queue.
//!
//! Reconnect behaviour driven by [`super::reconnect::Backoff`]:
//!
//! - [`DecodeOutcome::SoftEof`] — server-side clean EOF: reset the
//!   attempt counter, emit `Reconnecting`, retry immediately.
//! - [`DecodeOutcome::HardNetwork`] — transport drop: jittered
//!   exponential sleep, emit `Reconnecting`, retry.
//! - [`DecodeOutcome::HardCodec`] — terminal codec / probe failure: emit
//!   `Error`, stop.
//! - [`DecodeOutcome::Cancelled`] — supervisor flipped the stop flag:
//!   exit cleanly, no event.

use std::io;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::time::Instant;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tracing::{debug, error, info, warn};

use crate::error::NightrideError;

use super::http::{HttpStream, ICY_METADATA_HEADER, IcyDemuxReader};
use super::reconnect::{Backoff, ErrorClass, classify};
use super::{AudioEvent, ConnectionState};

/// Pre-roll buffer in samples (per channel-pair). 4096 stereo samples
/// ≈ 92 ms @ 44.1 kHz. Pre-loaded into the sink before the first audible
/// `play()` so the start does not stutter on slow networks.
pub(crate) const PREROLL_SAMPLES: usize = 4096;

/// Format-probe extension hint. MP3-only for v1.x; HLS sub-tree (deferred)
/// will route via a different probe path.
const PROBE_EXTENSION_HINT: &str = "mp3";

/// Emit an `AudioEvent` without ever blocking the decode thread.
///
/// `try_send` keeps the audio pipeline lock-free against a slow
/// supervisor: if the 64-slot event channel is momentarily full we
/// drop the event rather than stall the decode loop (the supervisor
/// will see the next state transition anyway). A *closed* channel,
/// however, means the supervisor is gone — caller must bail and let
/// the decode loop unwind as `DecodeOutcome::Cancelled`.
fn try_emit(evt_tx: &mpsc::Sender<AudioEvent>, event: AudioEvent) -> Result<(), ()> {
    match evt_tx.try_send(event) {
        Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
        Err(TrySendError::Closed(_)) => Err(()),
    }
}

/// Outcome of a single connect-and-decode attempt.
///
/// `SoftEof` and `HardNetwork` are recoverable; `HardCodec`
/// is terminal and `Cancelled` is the cooperative-shutdown signal.
pub(crate) enum DecodeOutcome {
    /// Supervisor cancelled the stream — exit cleanly without an event.
    Cancelled,
    /// Server-side clean EOF — retry immediately, attempt counter resets.
    SoftEof,
    /// Transport-layer fault — exponential backoff retry.
    HardNetwork(NightrideError),
    /// Terminal codec / probe / validation failure — stop, surface error.
    HardCodec(NightrideError),
}

/// Symphonia decode loop. Runs in its own thread until `stop_flag` flips
/// or a [`DecodeOutcome::HardCodec`] / cancellation surfaces.
#[allow(
    clippy::needless_pass_by_value,
    reason = "decode_loop runs in a thread::spawn context; owning args ensures 'static"
)]
pub(crate) fn decode_loop(
    url: String,
    sample_tx: SyncSender<i16>,
    stop_flag: Arc<AtomicBool>,
    evt_tx: mpsc::Sender<AudioEvent>,
    station_slug: &'static str,
    speaker_rate: OnceLock<u32>,
    ready_flag: Arc<AtomicBool>,
) {
    let mut backoff = Backoff::new(None);
    let mut attempt: u32 = 0;

    // Build the reqwest client ONCE outside the reconnect loop so the
    // TLS session cache + connection pool survive across attempts.
    // Timeouts protect against hanging upstreams; redirect policy is
    // explicit `none` because Icecast streams must not redirect.
    let client = match reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(err = %e, "audio::decode::client_build failed");
            return;
        }
    };

    loop {
        if stop_flag.load(Ordering::Acquire) {
            return;
        }

        let outcome = try_decode_once(
            &client,
            &url,
            &sample_tx,
            &stop_flag,
            &speaker_rate,
            &evt_tx,
            station_slug,
            &ready_flag,
        );

        match outcome {
            Ok(()) | Err(DecodeOutcome::Cancelled) => return,
            Err(DecodeOutcome::SoftEof) => {
                let _ = backoff.record_soft_eof();
                attempt = 0;
                warn!(station = station_slug, "stream EOF, immediate reconnect");
                if try_emit(
                    &evt_tx,
                    AudioEvent::ConnectionState(ConnectionState::Reconnecting {
                        station: station_slug,
                        attempt: attempt + 1,
                    }),
                )
                .is_err()
                {
                    return;
                }
            }
            Err(DecodeOutcome::HardNetwork(err)) => {
                let dur = backoff.record_hard();
                attempt = attempt.saturating_add(1);
                warn!(
                    station = station_slug,
                    attempt,
                    sleep_ms = u64::try_from(dur.as_millis()).unwrap_or(u64::MAX),
                    error = %err,
                    "transport fault; scheduling reconnect"
                );
                if try_emit(
                    &evt_tx,
                    AudioEvent::ConnectionState(ConnectionState::Reconnecting {
                        station: station_slug,
                        attempt,
                    }),
                )
                .is_err()
                {
                    return;
                }
                std::thread::sleep(dur);
            }
            Err(DecodeOutcome::HardCodec(err)) => {
                error!(station = station_slug, error = %err, "terminal decode failure");
                let _ = try_emit(
                    &evt_tx,
                    AudioEvent::ConnectionState(ConnectionState::Error {
                        station: station_slug,
                        detail: err.to_string(),
                    }),
                );
                // Terminal classifications short-circuit retry (defensive
                // re-check; matches `Backoff::is_terminal`).
                debug_assert!(
                    Backoff::is_terminal(classify(&err))
                        || matches!(classify(&err), ErrorClass::HardNetwork),
                    "HardCodec outcome carrying non-Codec error class"
                );
                return;
            }
        }
    }
}

/// Single connect-and-decode attempt: builds the HTTP transport, probes
/// the format, runs the symphonia packet loop until EOF / cancellation /
/// terminal error.
#[allow(
    clippy::too_many_arguments,
    reason = "internal helper threading 7 distinct lifetimes; struct grouping would obscure ownership"
)]
pub(crate) fn try_decode_once(
    client: &reqwest::blocking::Client,
    url: &str,
    sample_tx: &SyncSender<i16>,
    stop_flag: &Arc<AtomicBool>,
    speaker_rate: &OnceLock<u32>,
    evt_tx: &mpsc::Sender<AudioEvent>,
    station_slug: &'static str,
    ready_flag: &Arc<AtomicBool>,
) -> Result<(), DecodeOutcome> {
    // Step 1: HTTP GET with the pre-built client. Transport failures
    // classify as `HardNetwork` and route through reconnect.
    let response = client
        .get(url)
        .header(ICY_METADATA_HEADER.0, ICY_METADATA_HEADER.1)
        .send()
        .map_err(|e| {
            DecodeOutcome::HardNetwork(NightrideError::network("audio::decode::http_get", e))
        })?;

    // Step 1.5: validate the HTTP response BEFORE the symphonia probe.
    // Icecast mountpoints whose source has temporarily disconnected
    // return `404` with a small `report.xml` body; without this gate
    // those bytes would reach the format probe and trip a `HardCodec`
    // outcome (terminal, no retry). Reclassify both non-2xx and
    // non-audio bodies as `HardNetwork` so the supervisor backs off
    // and retries until the source reconnects.
    let status = response.status();
    if !status.is_success() {
        return Err(DecodeOutcome::HardNetwork(
            NightrideError::upstream_unavailable(
                "audio::decode::http_status",
                format!("status {} from {url}", status.as_u16()),
            ),
        ));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let ct_is_audio = content_type
        .as_deref()
        .is_some_and(|s| s.to_ascii_lowercase().starts_with("audio/"));
    if !ct_is_audio {
        let ct = content_type.as_deref().unwrap_or("(no content-type)");
        return Err(DecodeOutcome::HardNetwork(
            NightrideError::upstream_unavailable(
                "audio::decode::content_type",
                format!("expected audio/*, got {ct}"),
            ),
        ));
    }

    // Step 2: detect the icy-metaint header. Absent → upstream did not
    // honour the request; fall back to plain pass-through. Reasonable
    // Icecast configurations use intervals around 8 KB-16 KB; cap at
    // 64 KB to reject hostile servers that try to disable metadata
    // extraction by advertising an absurd interval.
    let icy_metaint = response
        .headers()
        .get("icy-metaint")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0 && n <= 65536);

    let http = match icy_metaint {
        Some(interval) => HttpStream::Icy(IcyDemuxReader::new(response, interval, evt_tx.clone())),
        None => HttpStream::Plain(response),
    };
    let mss = MediaSourceStream::new(Box::new(http), MediaSourceStreamOptions::default());

    // Step 3: probe the container format. Failure here is terminal.
    let probed = symphonia::default::get_probe()
        .format(
            &Hint::new().with_extension(PROBE_EXTENSION_HINT).to_owned(),
            mss,
            &FormatOptions {
                enable_gapless: true,
                ..FormatOptions::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(|e| DecodeOutcome::HardCodec(NightrideError::decode("audio::decode::probe", e)))?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.sample_rate.is_some())
        .ok_or_else(|| {
            DecodeOutcome::HardCodec(NightrideError::NotFound {
                op: "audio::decode::track",
                what: "no audio track in stream".to_string(),
            })
        })?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| {
            DecodeOutcome::HardCodec(NightrideError::decode("audio::decode::codec_init", e))
        })?;

    if try_emit(
        evt_tx,
        AudioEvent::ConnectionState(ConnectionState::Streaming {
            station: station_slug,
            started_at: Instant::now(),
        }),
    )
    .is_err()
    {
        return Err(DecodeOutcome::Cancelled);
    }

    // Step 4: drive the packet loop until EOF / cancel / fatal.
    drive_decode_loop(
        &mut *format,
        &mut *decoder,
        track_id,
        sample_tx,
        stop_flag,
        speaker_rate,
        ready_flag,
        station_slug,
    )
}

/// Inner packet loop. Returns the same [`DecodeOutcome`] surface as
/// [`try_decode_once`]; split out so the connect / probe and packet pump
/// each stay under the per-function line cap independently.
#[allow(
    clippy::too_many_arguments,
    reason = "shares supervisor / decoder / channel lifetimes with try_decode_once"
)]
fn drive_decode_loop(
    format: &mut dyn symphonia::core::formats::FormatReader,
    decoder: &mut dyn symphonia::core::codecs::Decoder,
    track_id: u32,
    sample_tx: &SyncSender<i16>,
    stop_flag: &Arc<AtomicBool>,
    speaker_rate: &OnceLock<u32>,
    ready_flag: &Arc<AtomicBool>,
    station_slug: &'static str,
) -> Result<(), DecodeOutcome> {
    let mut preroll_done = false;
    let mut samples_pushed: usize = 0;

    loop {
        if stop_flag.load(Ordering::Acquire) {
            return Err(DecodeOutcome::Cancelled);
        }
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(io_err))
                if io_err.kind() == io::ErrorKind::UnexpectedEof =>
            {
                return Err(DecodeOutcome::SoftEof);
            }
            Err(SymphoniaError::IoError(io_err)) => {
                // Non-EOF io is a transport drop — classify as network.
                return Err(DecodeOutcome::HardNetwork(NightrideError::io(
                    "audio::decode::next_packet",
                    io_err,
                )));
            }
            Err(SymphoniaError::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(e) => {
                return Err(DecodeOutcome::HardCodec(NightrideError::decode(
                    "audio::decode::next_packet",
                    e,
                )));
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(audio_buf) => {
                let spec = *audio_buf.spec();
                let _ = speaker_rate.set(spec.rate);
                let mut buffer = SampleBuffer::<i16>::new(audio_buf.capacity() as u64, spec);
                buffer.copy_interleaved_ref(audio_buf);
                for &s in buffer.samples() {
                    if sample_tx.send(s).is_err() {
                        return Err(DecodeOutcome::Cancelled);
                    }
                    samples_pushed += 1;
                    if !preroll_done && samples_pushed >= PREROLL_SAMPLES {
                        preroll_done = true;
                        // Signal the supervisor: it can lift the mute it
                        // installed at SetStation time. Release ordering
                        // pairs with the supervisor's Acquire load.
                        ready_flag.store(true, Ordering::Release);
                        info!(station = station_slug, "pre-roll done");
                    }
                }
            }
            Err(SymphoniaError::DecodeError(e)) => {
                debug!(station = station_slug, "skip bad packet: {e}");
            }
            Err(SymphoniaError::ResetRequired) => {
                decoder.reset();
            }
            Err(e) => {
                error!(station = station_slug, "decode fatal: {e}");
                return Err(DecodeOutcome::HardCodec(NightrideError::decode(
                    "audio::decode::packet_decode",
                    e,
                )));
            }
        }
    }
}
