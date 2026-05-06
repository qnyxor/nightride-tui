// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Metadata module: ICY (Icecast / Shoutcast) and SSE (HLS) parsers.
//!
//! Two transport paths:
//! - **ICY (MP3 mode)**: frame-based metadata from Icecast ICY-metaint headers.
//!   Frame-based parser + in-memory history ring buffer.
//! - **SSE (HLS mode)**: push-based JSON from `https://nightride.fm/meta`.
//!
//! ## ICY Parser
//!
//! Pure parser: turn the ICY-metaint byte stream into a [`Metadata`]
//! record. Defensive: malformed UTF-8 falls back to lossy decoding,
//! empty payloads emit `None`, multi-key payloads tolerate any field
//! order, and `--`-prefixed garbage cannot inject control characters.
//! History ring: a fixed-capacity [`History`] buffer (10 items by
//! default) of recently played tracks. Skips consecutive duplicates so
//! a long-running stream does not flood the panel with the same entry.
//! IO machinery (interleaving the metadata frames into the audio stream)
//! lives in `audio.rs`. This module operates on byte slices only.

pub mod sse;

use std::collections::VecDeque;
use std::time::SystemTime;

/// Now-playing metadata snapshot extracted from an ICY-metaint payload.
///
/// Both fields are `Option` because the stream may emit empty tags between
/// tracks; callers distinguish "unknown" (None) from "deliberately blank"
/// at the UI layer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    /// Track artist when known.
    pub artist: Option<String>,
    /// Track title when known.
    pub title: Option<String>,
    /// Raw decoded text for debug surfaces.
    pub raw: Option<String>,
}

impl Metadata {
    /// Convenience: returns true when both artist and title are absent.
    ///
    /// # Examples
    ///
    /// ```
    /// use nightride_tui::metadata::Metadata;
    ///
    /// let empty = Metadata { artist: None, title: None, raw: None };
    /// assert!(empty.is_empty());
    ///
    /// let has_title = Metadata {
    ///     artist: None,
    ///     title: Some("Turbo Killer".to_string()),
    ///     raw: None,
    /// };
    /// assert!(!has_title.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.artist.is_none() && self.title.is_none()
    }
}

/// Parse a raw ICY metadata payload (the bytes following the length byte).
///
/// Accepts arbitrary `&[u8]`; any UTF-8 decode failure falls back to lossy
/// decoding so a single malformed byte does not silently kill the metadata
/// surface. Returns `None` only for empty or whitespace-only payloads.
///
/// Common payload shape: `StreamTitle='Artist - Title';StreamUrl='...';\0\0\0`
/// (NUL-padded to 16-byte granularity per Shoutcast spec).
///
/// # Examples
///
/// ```
/// use nightride_tui::metadata::parse_stream_title;
///
/// let payload = b"StreamTitle='Carpenter Brut - Turbo Killer';";
/// let md = parse_stream_title(payload).expect("payload parses");
/// assert_eq!(md.artist.as_deref(), Some("Carpenter Brut"));
/// assert_eq!(md.title.as_deref(), Some("Turbo Killer"));
///
/// // Empty / NUL-padded payloads return None.
/// assert!(parse_stream_title(b"").is_none());
/// assert!(parse_stream_title(b"\0\0\0\0").is_none());
/// ```
#[must_use]
pub fn parse_stream_title(raw: &[u8]) -> Option<Metadata> {
    let trimmed = trim_padding(raw);
    if trimmed.is_empty() {
        return None;
    }

    // UTF-8 fallback: lossy decoding preserves render even for corrupted
    // bytes (malformed UTF-8 must not panic).
    let text_owned = String::from_utf8_lossy(trimmed).into_owned();
    let raw_text = text_owned.trim().to_string();

    let mut artist: Option<String> = None;
    let mut title: Option<String> = None;

    for segment in text_owned.split(';') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        let Some((key, raw_value)) = segment.split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("StreamTitle") {
            continue;
        }
        let value = raw_value.trim().trim_matches('\'').trim();
        if value.is_empty() {
            // StreamTitle is present but empty — distinct from absence.
            return None;
        }
        if let Some((a, t)) = value.split_once(" - ") {
            let a = a.trim();
            let t = t.trim();
            artist = (!a.is_empty()).then(|| a.to_string());
            title = (!t.is_empty()).then(|| t.to_string());
        } else {
            // No separator: keep the whole value as the title.
            title = Some(value.to_string());
        }
        break;
    }

    if artist.is_none() && title.is_none() {
        return None;
    }

    Some(Metadata {
        artist,
        title,
        raw: Some(raw_text),
    })
}

/// Decode an ICY metadata frame given the leading length byte and payload.
///
/// `frame[0]` is the length-multiplier byte: payload size = `frame[0] * 16`.
/// `frame[1..]` is the payload itself, NUL-padded to that exact size.
///
/// Returns `None` when:
/// - `frame` is empty (no length byte).
/// - The advertised payload size exceeds the slice length (truncated frame).
/// - The payload parses as empty per [`parse_stream_title`].
#[must_use]
pub fn parse_icy(frame: &[u8]) -> Option<Metadata> {
    let length_byte = *frame.first()?;
    let payload_len = usize::from(length_byte) * 16;
    if payload_len == 0 {
        return None;
    }
    let payload_end = 1usize.checked_add(payload_len)?;
    if payload_end > frame.len() {
        // Truncated frame — defer rather than panic.
        return None;
    }
    parse_stream_title(&frame[1..payload_end])
}

/// Trim NUL padding inserted by the server plus ASCII whitespace at either
/// end. Returns a zero-byte slice if everything was padding.
fn trim_padding(raw: &[u8]) -> &[u8] {
    let mut end = raw.len();
    while end > 0 {
        let b = raw[end - 1];
        if b == 0 || b.is_ascii_whitespace() {
            end -= 1;
        } else {
            break;
        }
    }
    let mut start = 0;
    while start < end {
        let b = raw[start];
        if b == 0 || b.is_ascii_whitespace() {
            start += 1;
        } else {
            break;
        }
    }
    &raw[start..end]
}

/// One history entry. Stored chronologically (oldest first) in [`History`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryItem {
    /// Station slug at the time the metadata arrived.
    pub station_slug: &'static str,
    /// Track metadata snapshot.
    pub metadata: Metadata,
    /// System time the entry was pushed (for the STREAM tab timestamp).
    pub at: SystemTime,
}

/// Bounded chronological ring of recent tracks.
///
/// Capacity is fixed at construction; oldest entries drop when the ring is
/// full. `push_distinct` skips consecutive duplicates so a long-running
/// stream emitting the same `StreamTitle` repeatedly only records one
/// entry. Persistence is out of scope (in-memory only).
#[derive(Debug, Clone)]
pub struct History {
    items: VecDeque<HistoryItem>,
    cap: usize,
}

impl History {
    /// Default capacity (10 entries).
    pub const DEFAULT_CAPACITY: usize = 10;

    /// Build a ring with the canonical capacity (10).
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(Self::DEFAULT_CAPACITY)
    }

    /// Build a ring with an arbitrary capacity. `cap` of 0 yields an
    /// always-empty ring; `push_distinct` is a no-op.
    #[must_use]
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            items: VecDeque::with_capacity(cap),
            cap,
        }
    }

    /// Number of stored entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True when the ring contains zero entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Snapshot of all entries, oldest first.
    #[must_use]
    pub fn snapshot(&self) -> Vec<HistoryItem> {
        self.items.iter().cloned().collect()
    }

    /// Push a new entry, skipping if it duplicates the most recent one
    /// (same station + same artist + same title). Trims to capacity when
    /// the ring is full.
    pub fn push_distinct(&mut self, item: HistoryItem) {
        if self.cap == 0 {
            return;
        }
        if self.items.back().is_some_and(|last| {
            last.station_slug == item.station_slug && last.metadata == item.metadata
        }) {
            return;
        }
        if self.items.len() == self.cap {
            self.items.pop_front();
        }
        self.items.push_back(item);
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{History, HistoryItem, Metadata, parse_icy, parse_stream_title, trim_padding};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn ts(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn parse_basic_artist_title() {
        let raw = b"StreamTitle='Carpenter Brut - Turbo Killer';";
        let md = parse_stream_title(raw).expect("non-empty payload");
        assert_eq!(md.artist.as_deref(), Some("Carpenter Brut"));
        assert_eq!(md.title.as_deref(), Some("Turbo Killer"));
    }

    /// Malformed UTF-8 must NOT panic. `from_utf8_lossy` produces
    /// replacement chars; metadata still emits.
    #[test]
    fn parse_malformed_utf8_falls_back_to_lossy() {
        let mut raw = Vec::from(b"StreamTitle='Ca");
        raw.push(0xC3);
        raw.push(0x28); // invalid continuation byte
        raw.extend_from_slice(b" - Track';");
        let md = parse_stream_title(&raw).expect("lossy decode still emits");
        assert!(md.title.is_some());
    }

    /// EC-01: empty payload yields None, never panics.
    #[test]
    fn parse_empty_payload_returns_none() {
        assert!(parse_stream_title(b"").is_none());
        assert!(parse_stream_title(b"   ").is_none());
        assert!(parse_stream_title(&[0u8; 16]).is_none());
    }

    /// Empty StreamTitle='' is distinct from absent and yields None.
    #[test]
    fn parse_empty_stream_title_returns_none() {
        let raw = b"StreamTitle='';StreamUrl='https://example.com';";
        assert!(parse_stream_title(raw).is_none());
    }

    /// Title without a ` - ` separator: keep whole value as title.
    #[test]
    fn parse_title_only_no_separator() {
        let raw = b"StreamTitle='Just a Title';";
        let md = parse_stream_title(raw).unwrap();
        assert_eq!(md.artist, None);
        assert_eq!(md.title.as_deref(), Some("Just a Title"));
    }

    /// Multi-pair payload (StreamTitle + StreamUrl + custom): we pick
    /// StreamTitle and ignore the rest.
    #[test]
    fn parse_multi_pair_picks_stream_title() {
        let raw = b"StreamUrl='https://example';StreamTitle='Mega Drive - The Way of the Cobra';CustomKey='whatever';";
        let md = parse_stream_title(raw).unwrap();
        assert_eq!(md.artist.as_deref(), Some("Mega Drive"));
        assert_eq!(md.title.as_deref(), Some("The Way of the Cobra"));
    }

    /// UTF-8 with non-ASCII characters (eñe, accents, em-dash) survives
    /// round-trip without grapheme corruption.
    #[test]
    fn parse_utf8_preserves_grapheme_clusters() {
        let raw = "StreamTitle='Daño - Cañón con eñe — final';".as_bytes();
        let md = parse_stream_title(raw).unwrap();
        assert_eq!(md.artist.as_deref(), Some("Daño"));
        assert_eq!(md.title.as_deref(), Some("Cañón con eñe — final"));
    }

    /// `parse_icy` length byte arithmetic: length_byte * 16 = payload size.
    #[test]
    fn parse_icy_length_arithmetic() {
        // length_byte = 2 → payload size 32 bytes.
        let payload = b"StreamTitle='X - Y';\0\0\0\0\0\0\0\0\0\0\0\0";
        assert_eq!(payload.len(), 32);
        let mut frame = vec![2u8];
        frame.extend_from_slice(payload);
        let md = parse_icy(&frame).unwrap();
        assert_eq!(md.title.as_deref(), Some("Y"));
    }

    #[test]
    fn parse_icy_zero_length_returns_none() {
        let frame = [0u8];
        assert!(parse_icy(&frame).is_none());
    }

    #[test]
    fn parse_icy_truncated_frame_returns_none() {
        // length_byte = 4 → payload size 64, but slice has only 16 bytes.
        let mut frame = vec![4u8];
        frame.extend_from_slice(&[0u8; 16]);
        assert!(parse_icy(&frame).is_none());
    }

    #[test]
    fn parse_icy_missing_length_byte_returns_none() {
        assert!(parse_icy(&[]).is_none());
    }

    #[test]
    fn trim_padding_strips_nul_and_whitespace() {
        let raw = b"  StreamTitle='X';  \0\0\0\0";
        let trimmed = trim_padding(raw);
        assert_eq!(trimmed, b"StreamTitle='X';");
    }

    /// Fuzz-equivalent: 1 000 random byte sequences, each must yield Some
    /// or None — never panic.
    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "byte truncation is the intent of this LCG fuzz step"
    )]
    fn parse_random_bytes_never_panics() {
        for seed in 0u32..1000 {
            // Tiny LCG to avoid adding a `rand` test-dep just for this.
            let mut state = seed.wrapping_mul(0x9E37_79B9).wrapping_add(0x1234_5678);
            let len = (state % 96) as usize;
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
                buf.push((state >> 16) as u8);
            }
            // Both entry points must accept arbitrary bytes without panic.
            let _ = parse_stream_title(&buf);
            let _ = parse_icy(&buf);
        }
    }

    fn item(station: &'static str, artist: &str, title: &str, secs: u64) -> HistoryItem {
        HistoryItem {
            station_slug: station,
            metadata: Metadata {
                artist: Some(artist.to_string()),
                title: Some(title.to_string()),
                raw: None,
            },
            at: ts(secs),
        }
    }

    #[test]
    fn history_default_capacity_is_ten() {
        assert_eq!(History::DEFAULT_CAPACITY, 10);
        let h = History::new();
        assert_eq!(h.len(), 0);
        assert!(h.is_empty());
    }

    #[test]
    fn history_skips_consecutive_duplicates() {
        let mut h = History::new();
        let a = item("nightride", "Carpenter Brut", "Turbo Killer", 1);
        let b = item("nightride", "Carpenter Brut", "Turbo Killer", 2);
        h.push_distinct(a.clone());
        h.push_distinct(b);
        assert_eq!(h.len(), 1);
        assert_eq!(h.snapshot()[0], a);
    }

    #[test]
    fn history_keeps_distinct_entries() {
        let mut h = History::new();
        h.push_distinct(item("nightride", "A", "1", 1));
        h.push_distinct(item("nightride", "B", "2", 2));
        h.push_distinct(item("nightride", "A", "1", 3)); // distinct from previous
        assert_eq!(h.len(), 3);
    }

    #[test]
    fn history_trims_to_capacity() {
        let mut h = History::with_capacity(3);
        for i in 0..5u64 {
            h.push_distinct(item("nightride", "A", &format!("title-{i}"), i));
        }
        assert_eq!(h.len(), 3);
        let snap = h.snapshot();
        // Oldest two ("title-0", "title-1") were dropped.
        assert_eq!(snap[0].metadata.title.as_deref(), Some("title-2"));
        assert_eq!(snap[2].metadata.title.as_deref(), Some("title-4"));
    }

    #[test]
    fn history_zero_capacity_is_no_op() {
        let mut h = History::with_capacity(0);
        h.push_distinct(item("nightride", "A", "1", 1));
        assert_eq!(h.len(), 0);
    }
}
