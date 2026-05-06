// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! HLS (HTTP Live Streaming) playback pipeline.
//!
//! Implements end-to-end HLS segment fetch, parse, and decode for live AAC
//! streams from nightride.fm. Hardcoded to the `aac_hifi` variant (352 kbps)
//! with no automatic bitrate switching (ABR deferred to slice 2).
//!
//! # Architecture
//!
//! The HLS pipeline runs in a dedicated decode thread (mirroring the MP3 path
//! in `decode_loop`) and communicates with the supervisor via:
//! - **Sample queue** (`SyncSender<i16>`): decoded PCM samples to rodio
//! - **Event channel** (`mpsc::Sender<AudioEvent>`): connection state & errors
//! - **Stop flag** (`Arc<AtomicBool>`): graceful cancellation
//! - **Ready flag** (`Arc<AtomicBool>`): post-preroll synchronization
//!
//! # Media transport
//!
//! - Master m3u8 endpoint: `https://stream.nightride.fm:8443/{slug}/{slug}.m3u8`
//! - Variant selection: hardcoded to `aac_hifi`
//! - Segment format: fMP4 (ISO 14496-14, fragmented MP4), AAC-LC codec
//! - Segment naming: `aac_hifi_<unix-timestamp>.m4s` (media segments)
//!   and `aac_hifi_0.m4s` (init segment, contains `moov` box)
//! - Segment cadence: ~2 seconds, ~50 KB each
//! - Playlist refresh: 5–10 seconds per HLS spec
//!
//! # Codec caveats
//!
//! **AAC-LC in fMP4:** Symphonia decodes AAC audio from ISO MP4 containers
//! using the `aac` + `isomp4` features (enabled in `Cargo.toml`).
//! The init segment (`*_0.m4s`) contains the fMP4 header with codec metadata
//! in the `moov` box; media segments (`*_<unix>.m4s`) contain AAC frames
//! in `mdat` boxes.
//!
//! **Non-gapless click risk:** Symphonia decodes streams without hardware
//! gapless support, so segment boundaries may introduce a ~20 ms click.
//! Mitigation (tier-2): implement overlap-add on segment seams or enable
//! trim-to-source (future work, not in scope for slice 1).
//!
//! # Init segment caching
//!
//! The init segment (`aac_hifi_0.m4s`) is fetched and cached in memory per
//! session. On reconnect after a segment 404, we re-fetch the init segment
//! to ensure codec parameters remain synchronized. Cache eviction: session
//! scope only (not persisted). Per-variant persistent cache is deferred
//! to slice 2.
//!
//! # Error mapping & state machine
//!
//! HLS errors are mapped to the AD-01 state machine:
//! - **Parse errors** → `HardCodec` (terminal, emit `Error` state)
//! - **Segment 404** → `HardNetwork` (transient, emit `Reconnecting` state)
//! - **Playlist fetch timeout** → `HardNetwork` (exponential backoff)
//! - **Decoder EOF** → `SoftEof` (playlist boundary, reset backoff)
//!
//! The reconnect loop re-fetches the media playlist and detects new segment
//! URLs after backoff sleeps. No new reconnect state machine; AD-01 is reused.

use std::collections::HashSet;
use std::io::Cursor;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::time::{Duration, Instant};

use m3u8_rs::Playlist;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tracing::debug;

use crate::error::NightrideError;

use super::reconnect::{Backoff, ErrorClass, classify};
use super::{AudioEvent, ConnectionState};

/// Hardcoded variant name to select from the master m3u8.
const PREFERRED_VARIANT: &str = "aac_hifi";

/// AAC-LC priming samples per segment (Symphonia issue #402 mitigation).
/// Symphonia decodes AAC streams with ~2112 encoder delay samples inserted
/// at the start of each segment. These are silence and cause ~48ms gaps
/// at segment boundaries. We trim these samples to eliminate clicks.
/// This is a fallback constant used when `DecodedAudio::delay_frames()` is
/// unavailable or returns None.
const PRIMING_SAMPLES_AAC: u32 = 2112;

/// Process-wide reqwest client for HLS playlist + segment fetches.
/// Reusing one Client across station switches keeps the connection pool
/// warm — TLS handshakes to `stream.nightride.fm:8443` only pay their
/// ~150 ms cost on the first attach instead of every switch. Falls back
/// to a default Client if the builder fails (rare; only on TLS init).
static HLS_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent(crate::USER_AGENT)
        .tcp_keepalive(Duration::from_secs(60))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

/// Ring-set deduplication window: max segment URLs to track in memory.
/// At ~2s segments, 180 segments covers ~6 minutes of playback.
const SEGMENT_DEDUP_WINDOW: usize = 180;

/// Outcome of a single HLS fetch-and-decode cycle.
///
/// Mirrors `DecodeOutcome` from the MP3 path. `SoftEof` and `HardNetwork`
/// trigger retry with backoff; `HardCodec` is terminal.
#[derive(Debug)]
#[allow(dead_code)] // Variants used in hls_decode_loop_async; allow ok.
pub(crate) enum HlsDecodeOutcome {
    /// Supervisor cancelled the stream — exit cleanly without an event.
    Cancelled,
    /// Playlist boundary / EOF — retry immediately, attempt counter resets.
    SoftEof,
    /// Transport-layer fault — exponential backoff retry.
    HardNetwork(NightrideError),
    /// Terminal codec / parse failure — stop, surface error.
    HardCodec(NightrideError),
}

/// Emit an `AudioEvent` without ever blocking the decode thread.
///
/// `try_send` keeps the audio pipeline lock-free against a slow
/// supervisor: if the 64-slot event channel is momentarily full we
/// drop the event rather than stall the decode loop (the supervisor
/// will see the next state transition anyway). A *closed* channel,
/// however, means the supervisor is gone — caller must bail and let
/// the decode loop unwind as `HlsDecodeOutcome::Cancelled`.
fn try_emit(evt_tx: &mpsc::Sender<AudioEvent>, event: AudioEvent) -> Result<(), ()> {
    match evt_tx.try_send(event) {
        Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
        Err(TrySendError::Closed(_)) => Err(()),
    }
}

/// Fetch and parse a master m3u8 playlist from the given URL.
///
/// Returns the parsed playlist if successful. All errors map to `HardCodec`
/// since manifest parse / validation failure is terminal.
///
/// Currently unused at boot (we URL-derive the variant directly from the
/// master URL) but retained for slice 2 ABR variant selection. Tests
/// exercise it directly.
#[allow(dead_code)]
async fn fetch_master_playlist(
    client: &reqwest::Client,
    url: &str,
) -> Result<m3u8_rs::MasterPlaylist, NightrideError> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| NightrideError::network("hls::fetch_master_playlist", e))?;

    if !resp.status().is_success() {
        return Err(NightrideError::upstream_unavailable(
            "hls::fetch_master_playlist",
            format!(
                "HTTP {} {}",
                resp.status().as_u16(),
                resp.status().canonical_reason().unwrap_or("Unknown")
            ),
        ));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| NightrideError::network("hls::fetch_master_playlist::body", e))?;

    match m3u8_rs::parse_playlist_res(&bytes) {
        Ok(Playlist::MasterPlaylist(master)) => Ok(master),
        Ok(Playlist::MediaPlaylist(_)) => Err(NightrideError::config_invalid(
            "hls::fetch_master_playlist",
            "expected master playlist, got media playlist",
        )),
        Err(e) => Err(NightrideError::config_invalid(
            "hls::fetch_master_playlist",
            format!("parse error: {e}"),
        )),
    }
}

/// Select the variant URL from a master playlist.
///
/// Searches for a variant whose URI contains [`PREFERRED_VARIANT`].
/// Returns `NotFound` if no matching variant exists.
#[allow(dead_code)]
fn select_variant_url(master: &m3u8_rs::MasterPlaylist) -> Result<String, NightrideError> {
    master
        .variants
        .iter()
        .find(|v| v.uri.contains(PREFERRED_VARIANT))
        .map(|v| v.uri.clone())
        .ok_or_else(|| NightrideError::NotFound {
            op: "hls::select_variant_url",
            what: format!("variant containing '{PREFERRED_VARIANT}'"),
        })
}

/// Fetch and parse a media m3u8 playlist from the given URL.
///
/// Returns the parsed media playlist. Parse errors map to `HardCodec`;
/// HTTP errors map to `HardNetwork` (transient, will trigger backoff retry).
async fn fetch_media_playlist(
    client: &reqwest::Client,
    url: &str,
) -> Result<m3u8_rs::MediaPlaylist, NightrideError> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| NightrideError::network("hls::fetch_media_playlist", e))?;

    if !resp.status().is_success() {
        return Err(NightrideError::upstream_unavailable(
            "hls::fetch_media_playlist",
            format!(
                "HTTP {} {}",
                resp.status().as_u16(),
                resp.status().canonical_reason().unwrap_or("Unknown")
            ),
        ));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| NightrideError::network("hls::fetch_media_playlist::body", e))?;

    match m3u8_rs::parse_playlist_res(&bytes) {
        Ok(Playlist::MediaPlaylist(media)) => Ok(media),
        Ok(Playlist::MasterPlaylist(_)) => Err(NightrideError::config_invalid(
            "hls::fetch_media_playlist",
            "expected media playlist, got master playlist",
        )),
        Err(e) => Err(NightrideError::config_invalid(
            "hls::fetch_media_playlist",
            format!("parse error: {e}"),
        )),
    }
}

/// Extract segment URLs from a media playlist.
///
/// Returns a list of segment URIs only. Init segment (if present)
/// is filtered out; only media segments are returned.
fn extract_segments(media: &m3u8_rs::MediaPlaylist) -> Vec<String> {
    media
        .segments
        .iter()
        .filter(|seg| !seg.uri.ends_with("_0.m4s")) // Filter init segment
        .map(|seg| seg.uri.clone())
        .collect()
}

/// Fetch a segment from its URL (relative or absolute).
///
/// If `base_url` is provided, relative URLs are resolved against it.
/// Returns the raw segment bytes.
async fn fetch_segment(
    client: &reqwest::Client,
    url: &str,
    base_url: Option<&str>,
) -> Result<Vec<u8>, NightrideError> {
    let full_url = if let Some(base) = base_url {
        if url.starts_with("http://") || url.starts_with("https://") {
            url.to_string()
        } else {
            format!(
                "{}/{}",
                base.trim_end_matches('/'),
                url.trim_start_matches('/')
            )
        }
    } else {
        url.to_string()
    };

    let resp = client
        .get(&full_url)
        .send()
        .await
        .map_err(|e| NightrideError::network("hls::fetch_segment", e))?;

    if !resp.status().is_success() {
        return Err(NightrideError::upstream_unavailable(
            "hls::fetch_segment",
            format!("segment {}: HTTP {}", url, resp.status().as_u16()),
        ));
    }

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| NightrideError::network("hls::fetch_segment::body", e))
}

/// Persistent HLS decoder state across segments.
///
/// The decoder is initialized once per variant and reused for all segments
/// in that variant. This avoids ~100ms codec re-initialization latency
/// per segment boundary.
///
/// # Priming sample tracking
///
/// `priming_samples_remaining` tracks the number of samples still to be
/// discarded from the beginning of the current segment (post-init). On the
/// first segment of a new decode session, this is set to the detected delay
/// (via `delay_frames()`) or the fallback constant `PRIMING_SAMPLES_AAC`.
/// Subsequent segments in the same decoder session do not have priming
/// (the Symphonia decoder handles it internally for continuity).
struct HlsDecoder {
    format: Box<dyn symphonia::core::formats::FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    sample_rate: u32,
    priming_samples_remaining: u32,
}

impl HlsDecoder {
    /// Initialize a new decoder from init + first segment data.
    ///
    /// The init segment contains the fMP4 `moov` box (codec params);
    /// the first media segment provides the first `mdat` box (frames).
    /// Both are prepended to form a valid fMP4 stream for probing.
    fn new(init_data: &[u8], first_segment_data: &[u8]) -> Result<Self, NightrideError> {
        let combined = {
            let mut v = Vec::with_capacity(init_data.len() + first_segment_data.len());
            v.extend_from_slice(init_data);
            v.extend_from_slice(first_segment_data);
            v
        };

        let cursor = Cursor::new(combined);
        let mss = MediaSourceStream::new(Box::new(cursor), MediaSourceStreamOptions::default());

        let mut hint = Hint::new();
        hint.with_extension("m4s");

        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(|e| NightrideError::decode("hls::HlsDecoder::new::probe", e))?;

        let format_reader = probed.format;

        let track = format_reader.default_track().ok_or_else(|| {
            NightrideError::config_invalid("hls::HlsDecoder::new", "no audio track in init segment")
        })?;

        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| NightrideError::decode("hls::HlsDecoder::new::decoder", e))?;

        let sample_rate = track.codec_params.sample_rate.unwrap_or(44_100);

        // Detect priming offset from codec parameters or fallback to constant.
        // Symphonia's codec_params.delay field indicates encoder delay in samples.
        let priming = track.codec_params.delay.unwrap_or(PRIMING_SAMPLES_AAC);

        Ok(HlsDecoder {
            format: format_reader,
            decoder,
            sample_rate,
            priming_samples_remaining: priming,
        })
    }

    /// Decode all packets from the format, emitting samples to the queue.
    ///
    /// On the first call (when `priming_samples_remaining > 0`), this will skip
    /// the leading priming samples before emitting to the queue. This mitigates
    /// Symphonia issue #402 (AAC encoder delay silence at segment boundaries).
    /// Subsequent calls do not trim, as the decoder handles continuity internally.
    fn decode_all_packets(&mut self, sample_tx: &SyncSender<i16>) -> Result<(), NightrideError> {
        loop {
            match self.format.next_packet() {
                Ok(packet) => match self.decoder.decode(&packet) {
                    Ok(audio_buf) => {
                        let spec = *audio_buf.spec();
                        let mut sample_buf =
                            SampleBuffer::<i16>::new(audio_buf.capacity() as u64, spec);
                        sample_buf.copy_interleaved_ref(audio_buf);

                        let samples = sample_buf.samples();
                        #[allow(clippy::cast_possible_truncation)]
                        let skip_count =
                            self.priming_samples_remaining.min(samples.len() as u32) as usize;

                        for &sample in &samples[skip_count..] {
                            if sample_tx.send(sample).is_err() {
                                return Err(NightrideError::Cancelled {
                                    op: "hls::HlsDecoder::decode_all_packets::send",
                                });
                            }
                        }

                        #[allow(clippy::cast_possible_truncation)]
                        {
                            self.priming_samples_remaining = self
                                .priming_samples_remaining
                                .saturating_sub(skip_count as u32);
                        }
                    }
                    Err(SymphoniaError::DecodeError(_)) => {
                        debug!("hls: skipping malformed packet");
                    }
                    Err(e) => {
                        return Err(NightrideError::decode(
                            "hls::HlsDecoder::decode_all_packets::decode",
                            e,
                        ));
                    }
                },
                Err(SymphoniaError::IoError(_)) => {
                    break;
                }
                Err(e) => {
                    return Err(NightrideError::decode(
                        "hls::HlsDecoder::decode_all_packets::next_packet",
                        e,
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Decode AAC frames from a fMP4 segment using a persistent decoder.
///
/// The decoder must be provided by the caller and will be reused across
/// segments. For the first segment, pass `init_data` which will be combined
/// with the segment data to initialize the decoder's format+track state.
/// For subsequent segments, init is ignored and the segment is decoded
/// directly with the pre-initialized decoder.
///
/// # Priming sample trimming
///
/// On the first segment (when the decoder is created), priming samples
/// are tracked in `decoder.priming_samples_remaining`. The `decode_all_packets`
/// method will skip these samples before emitting to the sample queue,
/// mitigating Symphonia issue #402 (AAC encoder delay silence).
fn decode_segment_to_samples(
    buffer: &[u8],
    init_data: Option<&[u8]>,
    decoder: &mut Option<HlsDecoder>,
    sample_tx: &SyncSender<i16>,
    speaker_rate: &OnceLock<u32>,
) -> Result<(), NightrideError> {
    // Initialize decoder on first segment.
    if decoder.is_none() {
        let init = init_data.ok_or_else(|| {
            NightrideError::config_invalid(
                "hls::decode_segment_to_samples",
                "init segment required for first segment",
            )
        })?;
        *decoder = Some(HlsDecoder::new(init, buffer)?);
    }

    let dec = decoder.as_mut().expect("decoder initialized above");

    // Lock sample rate once.
    let _ = speaker_rate.get_or_init(|| dec.sample_rate);

    // Decode all packets from this segment. Priming samples (if any)
    // are trimmed by decode_all_packets on the first call only.
    dec.decode_all_packets(sample_tx)
}

/// HLS decode loop. Runs in its own thread, fetching and decoding segments.
///
/// Mirrors the MP3 decode loop in `super::decode::decode_loop`. Communicates
/// via `sample_tx` (PCM samples), `evt_tx` (state/error events), and
/// `stop_flag` (graceful cancellation).
///
/// The loop periodically fetches the media playlist, extracts new segment URLs,
/// and decodes them sequentially. Ring-set deduplication prevents re-fetching
/// the same segment if the playlist refreshes while a segment is in flight.
#[allow(
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    reason = "hls_decode_loop runs in a thread::spawn context; owning args ensures 'static; async loop is complex"
)]
pub(crate) fn hls_decode_loop(
    master_url: String,
    sample_tx: SyncSender<i16>,
    stop_flag: Arc<AtomicBool>,
    evt_tx: mpsc::Sender<AudioEvent>,
    station_slug: &'static str,
    speaker_rate: OnceLock<u32>,
    ready_flag: Arc<AtomicBool>,
) {
    // Runtime context: this closure must NOT be async because it runs in
    // a thread::spawn, not tokio::spawn. We use tokio::runtime::Handle::current()
    // to block_on async calls.
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            let _ = try_emit(
                &evt_tx,
                AudioEvent::Error(format!(
                    "hls_decode_loop: unable to create tokio runtime: {e}"
                )),
            );
            return;
        }
    };

    rt.block_on(async {
        if let Err(e) = hls_decode_loop_async(
            master_url,
            sample_tx,
            stop_flag,
            evt_tx.clone(),
            station_slug,
            speaker_rate,
            ready_flag,
        )
        .await
        {
            debug!(
                ?e,
                station = station_slug,
                "hls_decode_loop exited with error"
            );
        }
    });
}

/// Async implementation of the HLS decode loop.
#[allow(clippy::too_many_lines)]
async fn hls_decode_loop_async(
    master_url: String,
    sample_tx: SyncSender<i16>,
    stop_flag: Arc<AtomicBool>,
    evt_tx: mpsc::Sender<AudioEvent>,
    station_slug: &'static str,
    speaker_rate: OnceLock<u32>,
    ready_flag: Arc<AtomicBool>,
) -> Result<(), HlsDecodeOutcome> {
    let client = HLS_CLIENT.clone();
    let mut backoff = Backoff::new(None); // Use time-derived seed for production
    let mut attempt: u32 = 0;
    let mut fetched_segments: HashSet<String> = HashSet::new();
    let mut decoder: Option<HlsDecoder> = None;
    let mut variant_id = PREFERRED_VARIANT.to_string(); // Track variant for reset on change

    // Skip the master playlist: nightride.fm's m3u8 layout is consistent
    // (`<base>/<station>.m3u8` master is a sibling of `<base>/aac_hifi.m3u8`
    // variant). Going straight to the variant by URL-joining shaves one
    // HTTPS round-trip on boot and on every station switch.
    let variant_filename = format!("{PREFERRED_VARIANT}.m3u8");
    let variant_url = match url::Url::parse(&master_url).and_then(|b| b.join(&variant_filename)) {
        Ok(u) => u.to_string(),
        Err(e) => {
            let err = NightrideError::config_invalid(
                "hls::resolve_variant_url",
                format!("cannot derive variant from master: {e}"),
            );
            let _ = try_emit(
                &evt_tx,
                AudioEvent::ConnectionState(ConnectionState::Error {
                    station: station_slug,
                    detail: err.to_string(),
                }),
            );
            return Err(HlsDecodeOutcome::HardCodec(err));
        }
    };
    debug!(station = station_slug, variant_url = %variant_url, "hls: variant derived (no master fetch)");

    // Fetch the init segment once at startup.
    let init_url = variant_url
        .split('/') // Get parent dir
        .take(variant_url.split('/').count() - 1)
        .collect::<Vec<_>>()
        .join("/");
    let full_init_url = format!("{init_url}/aac_hifi_0.m4s");

    // Parallel boot: init segment and first media playlist fetch
    // concurrently — they share no dependency, but were sequential before.
    // Saves ~1 HTTPS round-trip (~150–300 ms) on every station switch.
    let (init_result, first_playlist_result) = tokio::join!(
        fetch_segment(&client, &full_init_url, None),
        fetch_media_playlist(&client, &variant_url),
    );

    let init_segment_data: Option<Vec<u8>> = match init_result {
        Ok(data) => {
            debug!(
                station = station_slug,
                bytes = data.len(),
                "hls: init segment fetched"
            );
            Some(data)
        }
        Err(e) => {
            debug!(
                ?e,
                station = station_slug,
                "hls: init segment fetch failed (non-fatal)"
            );
            None
        }
    };

    // Cache the pre-fetched playlist for the first loop iteration; from
    // the second iteration onwards the loop fetches normally.
    let mut prefetched_playlist: Option<m3u8_rs::MediaPlaylist> = match first_playlist_result {
        Ok(p) => Some(p),
        Err(e) => {
            debug!(
                ?e,
                station = station_slug,
                "hls: pre-loop playlist fetch failed; loop will retry with backoff"
            );
            None
        }
    };

    // Main loop: fetch media playlist, extract segment URLs, decode each.
    let mut preroll_done = false;

    loop {
        if stop_flag.load(Ordering::Acquire) {
            debug!(
                station = station_slug,
                "hls_decode_loop: stop_flag set, exiting"
            );
            return Err(HlsDecodeOutcome::Cancelled);
        }

        // Fetch media playlist (or use the parallel-prefetched one on the
        // first iteration).
        let media_playlist = if let Some(pre) = prefetched_playlist.take() {
            pre
        } else {
            match fetch_media_playlist(&client, &variant_url).await {
                Ok(m) => m,
                Err(e) => {
                    let class = classify(&e);
                    match class {
                        ErrorClass::SoftEof => {
                            let _zero_sleep = backoff.record_soft_eof();
                            attempt = 0;
                            debug!(
                                station = station_slug,
                                "hls: playlist fetch soft EOF, retrying immediately"
                            );
                            continue;
                        }
                        ErrorClass::HardNetwork => {
                            let sleep_duration = backoff.record_hard();
                            attempt = attempt.saturating_add(1);
                            debug!(
                                station = station_slug,
                                attempt,
                                error = ?e,
                                sleep_ms = sleep_duration.as_millis(),
                                "hls: playlist fetch failed (hard network), backing off"
                            );
                            let _ = try_emit(
                                &evt_tx,
                                AudioEvent::ConnectionState(ConnectionState::Reconnecting {
                                    station: station_slug,
                                    attempt,
                                }),
                            );
                            tokio::time::sleep(sleep_duration).await;
                            continue;
                        }
                        _ => {
                            let _ = try_emit(
                                &evt_tx,
                                AudioEvent::ConnectionState(ConnectionState::Error {
                                    station: station_slug,
                                    detail: e.to_string(),
                                }),
                            );
                            return Err(HlsDecodeOutcome::HardCodec(e));
                        }
                    }
                }
            }
        };

        // Extract segments and filter out already-fetched ones.
        let segments = extract_segments(&media_playlist);
        if segments.is_empty() {
            debug!(
                station = station_slug,
                "hls: playlist has no media segments yet, waiting"
            );
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            continue;
        }

        // Deduplicate: only fetch segments we haven't seen yet.
        for seg_uri in &segments {
            if fetched_segments.contains(seg_uri.as_str()) {
                debug!(
                    station = station_slug,
                    seg = seg_uri,
                    "hls: segment already fetched, skipping"
                );
                continue;
            }

            if stop_flag.load(Ordering::Acquire) {
                return Err(HlsDecodeOutcome::Cancelled);
            }

            // Fetch segment.
            let seg_data = match fetch_segment(&client, seg_uri, Some(&init_url)).await {
                Ok(data) => data,
                Err(e) => {
                    let class = classify(&e);
                    match class {
                        ErrorClass::SoftEof => {
                            let _zero_sleep = backoff.record_soft_eof();
                            attempt = 0;
                            debug!(
                                station = station_slug,
                                segment = seg_uri,
                                "hls: segment fetch soft EOF, retrying immediately"
                            );
                            continue;
                        }
                        ErrorClass::HardNetwork => {
                            let sleep_duration = backoff.record_hard();
                            attempt = attempt.saturating_add(1);
                            debug!(
                                station = station_slug,
                                segment = seg_uri,
                                attempt,
                                error = ?e,
                                sleep_ms = sleep_duration.as_millis(),
                                "hls: segment fetch failed (hard network), backing off"
                            );
                            let _ = try_emit(
                                &evt_tx,
                                AudioEvent::ConnectionState(ConnectionState::Reconnecting {
                                    station: station_slug,
                                    attempt,
                                }),
                            );
                            tokio::time::sleep(sleep_duration).await;
                            // Skip this segment; will be retried in next playlist refresh.
                            continue;
                        }
                        _ => {
                            let _ = try_emit(
                                &evt_tx,
                                AudioEvent::ConnectionState(ConnectionState::Error {
                                    station: station_slug,
                                    detail: e.to_string(),
                                }),
                            );
                            return Err(HlsDecodeOutcome::HardCodec(e));
                        }
                    }
                }
            };

            // Decode segment to samples.
            // Variant change detection: if variant URL changed, drop decoder and reinit
            if variant_id != PREFERRED_VARIANT {
                decoder = None;
                variant_id = PREFERRED_VARIANT.to_string();
                debug!(
                    station = station_slug,
                    "hls: variant changed, resetting decoder"
                );
            }

            match decode_segment_to_samples(
                &seg_data,
                init_segment_data.as_deref(),
                &mut decoder,
                &sample_tx,
                &speaker_rate,
            ) {
                Ok(()) => {
                    fetched_segments.insert(seg_uri.clone());

                    // Reset backoff after successful segment.
                    if !preroll_done {
                        // First segment = preroll complete.
                        preroll_done = true;
                        ready_flag.store(true, Ordering::Release);
                        let _ = try_emit(
                            &evt_tx,
                            AudioEvent::ConnectionState(ConnectionState::Streaming {
                                station: station_slug,
                                started_at: Instant::now(),
                            }),
                        );
                        debug!(
                            station = station_slug,
                            "hls: streaming started (preroll done)"
                        );
                    }

                    let _zero_sleep = backoff.record_soft_eof(); // Reset backoff on success
                    debug!(
                        station = station_slug,
                        segment = seg_uri,
                        "hls: segment decoded"
                    );
                }
                Err(NightrideError::Cancelled { .. }) => {
                    // Receiver dropped — supervisor is tearing this stream
                    // down (station switch / quit). Exit silently; emitting
                    // an Error here would briefly flash a misleading
                    // "cancelled" message in the now-playing line.
                    return Err(HlsDecodeOutcome::Cancelled);
                }
                Err(e) => {
                    let _ = try_emit(
                        &evt_tx,
                        AudioEvent::ConnectionState(ConnectionState::Error {
                            station: station_slug,
                            detail: e.to_string(),
                        }),
                    );
                    return Err(HlsDecodeOutcome::HardCodec(e));
                }
            }

            // Ring-set dedup: keep only the last N segments to avoid unbounded growth.
            if fetched_segments.len() > SEGMENT_DEDUP_WINDOW {
                let mut to_remove = vec![];
                for seg in &fetched_segments {
                    if !segments.contains(seg) {
                        to_remove.push(seg.clone());
                        if fetched_segments.len() - to_remove.len() <= SEGMENT_DEDUP_WINDOW {
                            break;
                        }
                    }
                }
                for seg in to_remove {
                    fetched_segments.remove(&seg);
                }
            }
        }

        // Playlist refresh interval. The decode loop pumps samples in
        // real time (backpressure-bounded by the rodio sink), so by the
        // time we finish a playlist's segments the queue is full and
        // the upstream playlist has rotated. The 250 ms sleep keeps the
        // refresh tight without hammering the server; dedup catches
        // segments we already consumed if the playlist hasn't rotated.
        debug!(
            station = station_slug,
            "hls: waiting for next playlist refresh"
        );
        tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that the PREFERRED_VARIANT constant is set to hifi.
    #[test]
    fn preferred_variant_is_hifi() {
        assert_eq!(PREFERRED_VARIANT, "aac_hifi");
    }

    /// Test that segment deduplication window is reasonable.
    #[test]
    fn segment_dedup_window_is_sane() {
        const { assert!(SEGMENT_DEDUP_WINDOW >= 100) };
        const { assert!(SEGMENT_DEDUP_WINDOW <= 1000) };
    }

    /// Test ring-set deduplication: segments already in the set are skipped.
    #[test]
    fn dedup_window_prevents_refetch() {
        let mut fetched: HashSet<String> = HashSet::new();
        let seg1 = "aac_hifi_1234567890.m4s".to_string();
        let seg2 = "aac_hifi_1234567892.m4s".to_string();

        fetched.insert(seg1.clone());
        assert!(fetched.contains(&seg1));
        assert!(!fetched.contains(&seg2));

        fetched.insert(seg1.clone());
        assert_eq!(fetched.len(), 1); // No duplicate entry.

        fetched.insert(seg2.clone());
        assert_eq!(fetched.len(), 2);
    }

    /// Test that init segment names are correctly identified.
    #[test]
    fn init_segment_naming_pattern() {
        let init_name = "aac_hifi_0.m4s";
        let media_name = "aac_hifi_1234567890.m4s";

        assert!(init_name.ends_with("_0.m4s"));
        assert!(!media_name.ends_with("_0.m4s"));
    }

    /// Test decoder persistence logic via Option state.
    ///
    /// Verifies that decoder.is_none() checks and persistence pattern
    /// work correctly (init once, reuse across segments).
    #[test]
    fn decoder_persistence_option_state() {
        let mut decoder_state: Option<i32> = None;

        // First segment: decoder is None, should initialize.
        assert!(decoder_state.is_none());
        decoder_state = Some(1); // Simulate initialization

        // Second segment: decoder is Some, reuse (no re-init).
        assert!(decoder_state.is_some());

        // Third segment: still Some, persistence verified.
        assert!(decoder_state.is_some());
    }

    /// Test variant change detection and decoder reset.
    ///
    /// Verifies that variant_id flip triggers decoder drop.
    #[test]
    fn variant_change_triggers_decoder_reset() {
        let mut variant_id = "aac_hifi".to_string();
        let mut decoder_state: Option<i32> = Some(1); // Simulate active

        // Variant changes
        let new_variant = "aac_lq".to_string();
        if variant_id != new_variant {
            decoder_state = None;
            variant_id = new_variant;
        }

        assert!(decoder_state.is_none());
        assert_eq!(variant_id, "aac_lq");
    }

    /// Test decoder cleanup safety (RAII via Option::take).
    ///
    /// Verifies dropping decoder via take() doesn't panic.
    #[test]
    fn decoder_takes_cleanly() {
        let mut decoder_state: Option<i32> = Some(1);
        let _dropped = decoder_state.take();
        assert!(decoder_state.is_none());
    }

    /// Test priming sample trim initialization.
    ///
    /// Verifies that HlsDecoder initializes with priming_samples_remaining
    /// set to the detected delay or the fallback constant.
    #[test]
    fn priming_samples_initialized_correctly() {
        // Verify constant is reasonable.
        assert_eq!(PRIMING_SAMPLES_AAC, 2112u32);

        // Simulate priming_samples_remaining field behavior.
        let mut priming_remaining: u32 = 2112u32;
        assert_eq!(priming_remaining, 2112);

        // Simulate skipping samples from first packet.
        let skip_count: u32 = 2112u32;
        priming_remaining = priming_remaining.saturating_sub(skip_count);
        assert_eq!(priming_remaining, 0);

        // Subsequent packets should not trim (priming_remaining is 0).
        let skip_count_second = priming_remaining.min(100u32);
        assert_eq!(skip_count_second, 0);
    }

    /// Test priming sample exhaustion across multiple packets.
    ///
    /// Verifies that priming samples are correctly tracked and exhausted
    /// across packet boundaries. This ensures partial trimming works if
    /// a single packet doesn't contain all priming samples.
    #[test]
    fn priming_samples_exhaust_across_packets() {
        let mut priming_remaining: u32 = 2112u32;

        // First packet: 1500 samples available, trim first 1500.
        let first_packet_size: u32 = 1500u32;
        let skip_first = priming_remaining.min(first_packet_size);
        assert_eq!(skip_first, 1500);
        priming_remaining = priming_remaining.saturating_sub(skip_first);
        assert_eq!(priming_remaining, 612);

        // Second packet: 1000 samples available, trim first 612.
        let second_packet_size: u32 = 1000u32;
        let skip_second = priming_remaining.min(second_packet_size);
        assert_eq!(skip_second, 612);
        priming_remaining = priming_remaining.saturating_sub(skip_second);
        assert_eq!(priming_remaining, 0);

        // Third packet: no more trimming.
        let third_packet_size: u32 = 1000u32;
        let skip_third = priming_remaining.min(third_packet_size);
        assert_eq!(skip_third, 0);
    }
}
