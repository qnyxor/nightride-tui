// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! HTTP transport + ICY-metaint demultiplexer.
//!
//! - [`HttpStream`] is the symphonia [`MediaSource`] wrapping a
//!   [`reqwest::blocking::Response`]. Either raw pass-through (`Plain`)
//!   when the upstream did not honour `Icy-MetaData: 1`, or interleaved
//!   metadata demuxed via [`IcyDemuxReader`] (`Icy`).
//! - [`IcyDemuxReader`] parses Shoutcast-style metadata frames every
//!   `interval` audio bytes: a single length byte (multiplied by
//!   [`ICY_FRAME_MULTIPLIER`]) followed by the NUL-padded payload.
//!   Parsed `StreamTitle` values feed `AudioEvent::Metadata` via the
//!   blocking sender.

use std::io::{self, Read};

use symphonia::core::io::MediaSource;
use tokio::sync::mpsc;

use super::AudioEvent;

/// Shoutcast/Icecast ICY-metaint multiplier: the wire byte count is
/// the leading `len` byte times this constant.
pub(crate) const ICY_FRAME_MULTIPLIER: usize = 16;

/// HTTP request header advertising client support for ICY metadata
/// interleave. Some endpoints ignore it; both code paths are covered
/// by [`HttpStream`].
pub(crate) const ICY_METADATA_HEADER: (&str, &str) = ("Icy-MetaData", "1");

/// Symphonia [`MediaSource`] wrapping an HTTP body. Non-seekable, unknown
/// length — exactly what an Icecast stream is. Either the raw response
/// (`Plain`) or one with ICY metadata interleave demultiplexed
/// (`Icy`).
pub(crate) enum HttpStream {
    Plain(reqwest::blocking::Response),
    Icy(IcyDemuxReader),
}

impl Read for HttpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(r) => r.read(buf),
            Self::Icy(r) => r.read(buf),
        }
    }
}

impl io::Seek for HttpStream {
    fn seek(&mut self, _pos: io::SeekFrom) -> io::Result<u64> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "http stream is not seekable",
        ))
    }
}

impl MediaSource for HttpStream {
    fn is_seekable(&self) -> bool {
        false
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// Reader that demultiplexes audio bytes from interleaved ICY metadata
/// per the Shoutcast protocol. Audio bytes pass through unchanged;
/// every `interval` bytes the underlying body emits one length-byte +
/// payload pair, which is parsed via [`crate::metadata::parse_stream_title`]
/// and emitted as `AudioEvent::Metadata`.
pub(crate) struct IcyDemuxReader {
    inner: reqwest::blocking::Response,
    interval: usize,
    audio_left: usize,
    evt_tx: mpsc::Sender<AudioEvent>,
}

impl IcyDemuxReader {
    pub(crate) fn new(
        inner: reqwest::blocking::Response,
        interval: usize,
        evt_tx: mpsc::Sender<AudioEvent>,
    ) -> Self {
        Self {
            inner,
            interval,
            audio_left: interval,
            evt_tx,
        }
    }
}

impl Read for IcyDemuxReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Iterative loop instead of tail-recursion: a hostile or
        // misbehaving Icecast feeding back-to-back zero-byte audio
        // segments would otherwise grow the stack one frame per
        // segment until overflow. The loop bound is the inner reader
        // — a real Icy stream serves audio bytes between metadata
        // frames, so this returns on the first non-empty read.
        loop {
            // Audio segment in progress: copy up to `min(audio_left, buf.len())`.
            if self.audio_left > 0 {
                let want = self.audio_left.min(buf.len());
                let got = self.inner.read(&mut buf[..want])?;
                self.audio_left -= got;
                return Ok(got);
            }
            // Audio segment exhausted → read the metadata frame and
            // restart from the next segment.
            let mut len_byte = [0u8; 1];
            self.inner.read_exact(&mut len_byte)?;
            let payload_len = usize::from(len_byte[0]) * ICY_FRAME_MULTIPLIER;
            if payload_len > 0 {
                let mut payload = vec![0u8; payload_len];
                self.inner.read_exact(&mut payload)?;
                if let Some(md) = crate::metadata::parse_stream_title(&payload) {
                    // Best-effort metadata: drop on full or closed
                    // channel rather than stalling the decode read
                    // loop. State transitions are emitted from
                    // `decode.rs::try_emit`, which handles
                    // supervisor-gone reclassification; the metadata
                    // stream is lossy by design.
                    let _ = self.evt_tx.try_send(AudioEvent::Metadata(md));
                }
            }
            self.audio_left = self.interval;
        }
    }
}
