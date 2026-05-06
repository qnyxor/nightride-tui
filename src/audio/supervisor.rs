// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Audio supervisor task.
//!
//! Owns the rodio output stream + sink and the active stream task
//! lifetime. Receives [`AudioCommand`] over an mpsc channel and drives
//! the canonical Play / Stop / SetVolume / SetStation lifecycle. Stream
//! decode runs on a dedicated `std::thread::spawn` thread (see
//! [`super::decode`]) bridged via an `AtomicBool` stop flag.
//!
//! ## Constants
//!
//! - [`DEFAULT_FADE_OUT`] — linear fade-out duration on Stop / cancel.
//! - [`MUTE_DRAIN_GUARD`] — sleep budget after `set_volume(0)` so the
//!   cpal hardware ring drains the trailing samples at zero gain before
//!   the new source is appended.
//! - [`READY_POLL_INTERVAL`] — supervisor tick to read the
//!   `ready_flag` set by the decode thread post-pre-roll.
//! - [`FADE_STEPS`] — linear fade resolution for [`fade_and_clear`].
//! - [`SAMPLE_QUEUE_CAP`] — bounded queue between decoder and rodio
//!   output; ~46 ms @ 44.1 kHz stereo (matches pre-roll envelope).

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::sync_channel;
use std::time::Duration;

use rodio::Source;
use rodio::source::UniformSourceIterator;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::config::TransportFormat;
use crate::error::NightrideError;
use crate::station::Station;

use super::decode::decode_loop;
use super::hls::hls_decode_loop;
use super::source::{
    DEFAULT_CHANNELS, DEFAULT_SAMPLE_RATE, DecodedSource, INITIAL_VOLUME_PCT, VisualizerSource,
    vol_to_gain,
};
use super::{AudioCommand, AudioEvent, ConnectionState};

/// Linear fade-out duration on Stop / cancellation. 200 ms is the v1.x
/// canon (pinned in `tests::fade_canon_is_200ms`).
pub(crate) const DEFAULT_FADE_OUT: Duration = Duration::from_millis(200);

/// Mute-drain guard after a `sink.set_volume(0)` on station switch.
/// Sleeps long enough for cpal's hardware ring to physically drain at
/// zero gain (CoreAudio output buffers are typically 256-1024 frames
/// ≈ 6-23 ms @ 44.1 kHz; 100 ms also gives Bluetooth audio codecs
/// AAC/AptX time to flush their packet-loss-concealment state).
pub(crate) const MUTE_DRAIN_GUARD: Duration = Duration::from_millis(100);

/// Supervisor poll interval for the post-pre-roll `ready_flag`.
pub(crate) const READY_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Linear fade resolution for [`fade_and_clear`].
pub(crate) const FADE_STEPS: u16 = 20;

/// Bounded sample queue capacity between the decode thread and the
/// rodio audio thread. Sized to ~2 s @ 44.1 kHz stereo (interleaved i16,
/// so 176 400 samples). MP3 streams continuous so a small queue suffices,
/// but HLS pauses while fetching the next playlist + first segment of
/// the next batch — combined ~500–700 ms in the worst case. 2 s of
/// headroom hides those refreshes with margin to spare. ~352 KB RAM.
/// On station switch the supervisor drains via `sink.clear()` and the
/// `MUTE_DRAIN_GUARD` envelope still bounds the bleed window.
pub(crate) const SAMPLE_QUEUE_CAP: usize = 176_400;

/// Supervisor task. Owns the rodio output stream + sink and the active
/// stream task lifetime.
///
/// # Errors
///
/// The supervisor itself does not return an error to its caller — it
/// emits `AudioEvent::Error(...)` on `evt_tx` and continues running.
/// On `token.cancelled()` it executes the canonical stop sequence and
/// returns.
#[allow(
    clippy::too_many_lines,
    reason = "supervisor body owns the cpal Sink + per-stream cancellation \
              hierarchy as one unit; splitting per-arm fragments the lifecycle \
              invariants (mute → drain → cancel → clear → play → attach → ready)"
)]
pub async fn supervisor(
    mut cmd_rx: mpsc::Receiver<AudioCommand>,
    evt_tx: mpsc::Sender<AudioEvent>,
    amp_tx: mpsc::Sender<Vec<f32>>,
    visualizer_width: Arc<AtomicUsize>,
    token: CancellationToken,
) {
    // OutputStream lives for the supervisor's lifetime. Sink is per-stream
    // and recreated on station switch.
    //
    // `rodio::OutputStream` wraps a `cpal::Stream` which is `!Send` (the
    // device pointer cannot cross threads). We build it inside
    // `spawn_blocking` so the `!Send` value never crosses an await in
    // this async fn — only the `OutputStreamHandle` (Send + Sync) bubbles
    // back. The Stream is leaked via `mem::forget` because the supervisor
    // owns audio output for the process lifetime.
    let stream_handle =
        match tokio::task::spawn_blocking(|| match rodio::OutputStream::try_default() {
            Ok((stream, handle)) => {
                std::mem::forget(stream);
                Ok(handle)
            }
            Err(err) => Err(err.to_string()),
        })
        .await
        {
            Ok(Ok(handle)) => handle,
            Ok(Err(detail)) => {
                emit_audio_error(&evt_tx, format!("audio device unavailable: {detail}")).await;
                return;
            }
            Err(join_err) => {
                emit_audio_error(&evt_tx, format!("audio init join error: {join_err}")).await;
                return;
            }
        };

    // Persistent Sink across the supervisor's lifetime. Station switches
    // call `sink.clear()` to drop pending audio + `sink.append(new_source)`
    // to attach the next stream — avoiding the double-Sink glitch where
    // the old + new sinks briefly played together during a switch.
    let sink = match rodio::Sink::try_new(&stream_handle) {
        Ok(s) => Arc::new(s),
        Err(err) => {
            emit_audio_error(&evt_tx, format!("sink init: {err}")).await;
            return;
        }
    };
    sink.pause();

    let mut current_station: Option<&'static Station> = None;
    let mut current_stream_token: Option<CancellationToken> = None;
    let mut current_volume: u8 = INITIAL_VOLUME_PCT;
    let speaker_rate: OnceLock<u32> = OnceLock::new();
    sink.set_volume(vol_to_gain(current_volume));

    // Station-switch ready gate: when the supervisor mutes the sink on
    // Play/SetStation, it parks `volume_pending_restore = true` and
    // awaits this flag flipping to `true` before restoring the user
    // volume. The decode thread sets the flag after pre-roll so the
    // tail of the previous station's audio cannot bleed through.
    let mut current_ready_flag: Option<Arc<AtomicBool>> = None;
    let mut volume_pending_restore = false;
    let mut ready_tick = tokio::time::interval(READY_POLL_INTERVAL);

    loop {
        tokio::select! {
            () = token.cancelled() => {
                fade_and_clear(&sink, &mut current_stream_token, DEFAULT_FADE_OUT).await;
                break;
            }
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    AudioCommand::Play(st) => {
                        // Mute FIRST. `sink.clear()` is blocking and
                        // cpal's hardware ring keeps draining whatever
                        // was pre-fetched at the old volume — setting
                        // volume=0 here makes those trailing samples
                        // silent so the user never hears them.
                        sink.set_volume(0.0);
                        tokio::time::sleep(MUTE_DRAIN_GUARD).await;
                        // Cancel the previous stream task; clear the sink
                        // so the new source plays from the front.
                        if let Some(t) = current_stream_token.take() { t.cancel(); }
                        sink.clear();
                        sink.play();

                        let stream_token = token.child_token();
                        let ready_flag = Arc::new(AtomicBool::new(false));
                        // Use default format for Play command
                        match attach_stream(
                            st,
                            &sink,
                            evt_tx.clone(),
                            amp_tx.clone(),
                            visualizer_width.clone(),
                            &speaker_rate,
                            ready_flag.clone(),
                            stream_token.clone(),
                            TransportFormat::default(),
                        ).await {
                            Ok(()) => {
                                current_stream_token = Some(stream_token);
                                current_station = Some(st);
                                current_ready_flag = Some(ready_flag);
                                volume_pending_restore = true;
                            }
                            Err(err) => {
                                // No flag to gate on — restore volume
                                // immediately so the next Play is audible.
                                sink.set_volume(vol_to_gain(current_volume));
                                emit_audio_error(&evt_tx, err.to_string()).await;
                                let _ = evt_tx.send(AudioEvent::ConnectionState(
                                    ConnectionState::Error {
                                        station: st.slug,
                                        detail: err.to_string(),
                                    },
                                )).await;
                            }
                        }
                    }
                    AudioCommand::SetStation(st, input_format) => {
                        // Mute FIRST. `sink.clear()` is blocking and
                        // cpal's hardware ring keeps draining whatever
                        // was pre-fetched at the old volume — setting
                        // volume=0 here makes those trailing samples
                        // silent so the user never hears them.
                        sink.set_volume(0.0);
                        tokio::time::sleep(MUTE_DRAIN_GUARD).await;
                        // Cancel the previous stream task; clear the sink
                        // so the new source plays from the front.
                        if let Some(t) = current_stream_token.take() { t.cancel(); }
                        sink.clear();
                        sink.play();

                        let stream_token = token.child_token();
                        let ready_flag = Arc::new(AtomicBool::new(false));
                        match attach_stream(
                            st,
                            &sink,
                            evt_tx.clone(),
                            amp_tx.clone(),
                            visualizer_width.clone(),
                            &speaker_rate,
                            ready_flag.clone(),
                            stream_token.clone(),
                            input_format,
                        ).await {
                            Ok(()) => {
                                current_stream_token = Some(stream_token);
                                current_station = Some(st);
                                current_ready_flag = Some(ready_flag);
                                volume_pending_restore = true;
                            }
                            Err(err) => {
                                // No flag to gate on — restore volume
                                // immediately so the next Play is audible.
                                sink.set_volume(vol_to_gain(current_volume));
                                emit_audio_error(&evt_tx, err.to_string()).await;
                                let _ = evt_tx.send(AudioEvent::ConnectionState(
                                    ConnectionState::Error {
                                        station: st.slug,
                                        detail: err.to_string(),
                                    },
                                )).await;
                            }
                        }
                    }
                    AudioCommand::SetVolume(v) => {
                        let v = v.min(100);
                        current_volume = v;
                        // Single, canonical volume mutation site.
                        // If a station switch is in flight, the volume
                        // stays muted; the next ready_tick will pick up
                        // `current_volume` once the new stream is ready.
                        if !volume_pending_restore {
                            sink.set_volume(vol_to_gain(v));
                        }
                    }
                }
            }
            _ = ready_tick.tick() => {
                if volume_pending_restore
                    && let Some(flag) = current_ready_flag.as_ref()
                    && flag.load(Ordering::Acquire)
                {
                    sink.set_volume(vol_to_gain(current_volume));
                    volume_pending_restore = false;
                }
            }
        }
    }

    debug!(?current_station, "supervisor exit");
}

/// Cancel the active stream task and apply a linear fade-out on the
/// persistent sink over `fade`, then `clear` it so any pending audio
/// drops cleanly. Restores the volume after clear so the next play
/// resumes at the user's setting.
async fn fade_and_clear(
    sink: &rodio::Sink,
    token_slot: &mut Option<CancellationToken>,
    fade: Duration,
) {
    if let Some(t) = token_slot.take() {
        t.cancel();
    }
    let step_dur = fade / u32::from(FADE_STEPS);
    let start_vol = sink.volume();
    for i in 1..=FADE_STEPS {
        let factor = 1.0 - (f32::from(i) / f32::from(FADE_STEPS));
        sink.set_volume(start_vol * factor.max(0.0));
        tokio::time::sleep(step_dur).await;
    }
    sink.clear();
    sink.set_volume(start_vol);
    sink.pause();
}

/// Attach a fresh decoder source to the persistent sink + spawn the
/// matching decode thread. Sink lifecycle (clear / play / pause) is the
/// supervisor's responsibility.
#[allow(
    clippy::too_many_arguments,
    reason = "wiring 7 distinct longevity domains; struct grouping would obscure ownership"
)]
async fn attach_stream(
    station: &'static Station,
    sink: &Arc<rodio::Sink>,
    evt_tx: mpsc::Sender<AudioEvent>,
    amp_tx: mpsc::Sender<Vec<f32>>,
    visualizer_width: Arc<AtomicUsize>,
    speaker_rate: &OnceLock<u32>,
    ready_flag: Arc<AtomicBool>,
    stream_token: CancellationToken,
    input_format: TransportFormat,
) -> Result<(), NightrideError> {
    let _ = evt_tx
        .send(AudioEvent::ConnectionState(ConnectionState::Connecting {
            station: station.slug,
            started_at: std::time::Instant::now(),
        }))
        .await;

    // Sample queue: decode thread → DecodedSource consumer.
    let (sample_tx, sample_rx) = sync_channel::<i16>(SAMPLE_QUEUE_CAP);

    // Default to the canonical rate / channel pair until the decoder
    // reports otherwise. The decode thread writes the actual rate via
    // the OnceLock once known.
    let decoded = DecodedSource::new(sample_rx, DEFAULT_SAMPLE_RATE, DEFAULT_CHANNELS);

    // Wrap in the visualizer decorator so PCM samples feed the UI tap.
    let decorated = VisualizerSource::new(decoded, amp_tx, visualizer_width);

    // Lock-once sample rate: if the speaker rate is already locked and
    // differs from this stream's, resample. UniformSourceIterator handles
    // the conversion transparently.
    let final_source: Box<dyn Source<Item = i16> + Send> =
        match speaker_rate.get() {
            Some(&locked) if locked != decorated.sample_rate() => Box::new(
                UniformSourceIterator::new(decorated, DEFAULT_CHANNELS, locked),
            ),
            _ => Box::new(decorated),
        };
    sink.append(final_source);

    // Use the input format from config (MP3 or HLS).
    let use_hls = input_format == TransportFormat::Hls;

    // Spawn the decode thread. It owns the HTTP body, the symphonia
    // decoder, and the sample_tx end of the queue.
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_thread = stop_flag.clone();
    let evt_tx_thread = evt_tx.clone();
    let station_slug = station.slug;
    let speaker_rate_thread = speaker_rate.clone();

    std::thread::spawn(move || {
        if use_hls {
            debug!(station = station_slug, "attach_stream: using HLS path");
            let hls_url = station.stream_hls.to_string();
            hls_decode_loop(
                hls_url,
                sample_tx,
                stop_flag_thread,
                evt_tx_thread,
                station_slug,
                speaker_rate_thread,
                ready_flag,
            );
        } else {
            debug!(station = station_slug, "attach_stream: using MP3 path");
            let mp3_url = station.stream_mp3.to_string();
            decode_loop(
                mp3_url,
                sample_tx,
                stop_flag_thread,
                evt_tx_thread,
                station_slug,
                speaker_rate_thread,
                ready_flag,
            );
        }
    });

    // If HLS mode is active, spawn the SSE metadata supervisor.
    // The task runs in a tokio::spawn and is cancelled when the stream
    // detaches (via stream_token cancellation).
    if use_hls {
        debug!(
            station = station.slug,
            "attach_stream: spawning SSE metadata supervisor"
        );
        let evt_tx_sse = evt_tx.clone();
        let sse_token = stream_token.child_token();
        tokio::spawn(async move {
            crate::metadata::sse::spawn_sse_supervisor(evt_tx_sse, station_slug, sse_token).await;
        });
    }

    // Bridge the cancellation token to the AtomicBool so a child cancel
    // unwinds the decode thread.
    tokio::spawn(async move {
        stream_token.cancelled().await;
        stop_flag.store(true, Ordering::Release);
    });

    Ok(())
}

/// Send an `AudioEvent::Error(detail)` ignoring closed-channel drops.
/// Centralised here so the supervisor body stays uniform.
async fn emit_audio_error(evt_tx: &mpsc::Sender<AudioEvent>, detail: impl Into<String>) {
    let _ = evt_tx.send(AudioEvent::Error(detail.into())).await;
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_FADE_OUT, FADE_STEPS, MUTE_DRAIN_GUARD, READY_POLL_INTERVAL, SAMPLE_QUEUE_CAP,
    };
    use std::time::Duration;

    /// Pin the canonical fade-out duration (200 ms default).
    #[test]
    fn fade_canon_is_200ms() {
        assert_eq!(DEFAULT_FADE_OUT, Duration::from_millis(200));
    }

    #[test]
    fn supervisor_constants_are_sane() {
        assert_eq!(MUTE_DRAIN_GUARD, Duration::from_millis(100));
        assert_eq!(READY_POLL_INTERVAL, Duration::from_millis(50));
        assert_eq!(FADE_STEPS, 20);
        assert_eq!(SAMPLE_QUEUE_CAP, 176_400);
    }
}
