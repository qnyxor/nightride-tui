// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Audio source decorators consumed by `rodio::Sink::append`.
//!
//! - [`VisualizerSource`] taps PCM samples to compute per-column RMS for
//!   the spectrum widget. Drop-on-full so the cpal callback thread never
//!   blocks on UI consumers.
//! - [`DecodedSource`] is the pull-side bridge between the symphonia
//!   decode thread and the rodio output. Returns silence on transient
//!   under-runs because blocking the cpal callback stalls the audio
//!   device.
//! - [`rms_per_col`] computes per-bucket RMS over a `[i16]` window.
//! - [`vol_to_gain`] maps a `0..=100` volume percent to the `f32` gain
//!   factor consumed by `rodio::Sink::set_volume`. Single canonical
//!   conversion site.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Receiver as SyncReceiver;
use std::time::Duration;

use rodio::Source;
use tokio::sync::mpsc;

/// Batch size in samples before computing a visualizer RMS frame.
/// ~1024 samples ≈ 23 ms @ 44.1 kHz tick budget.
pub(crate) const VISUALIZER_BATCH: usize = 1024;

/// Default initial volume percent (0..=100). Applied at supervisor start
/// before the persisted `Config::default_volume` lands.
pub(crate) const INITIAL_VOLUME_PCT: u8 = 50;

/// Default sample rate fed into `DecodedSource` until the decoder reports
/// the actual stream rate via the speaker `OnceLock`. Matches the
/// majority case for nightride.fm streams.
pub(crate) const DEFAULT_SAMPLE_RATE: u32 = 44_100;

/// Default channel count. Stereo is universal for the nightride registry.
pub(crate) const DEFAULT_CHANNELS: u16 = 2;

/// Map a `0..=100` volume percent to the `f32` gain consumed by
/// `rodio::Sink::set_volume`. Inputs above 100 are clamped.
#[must_use]
pub(crate) fn vol_to_gain(pct: u8) -> f32 {
    f32::from(pct.min(100)) / 100.0
}

/// `rodio::Source` decorator that taps PCM samples for the visualizer.
///
/// On every batch of `VISUALIZER_BATCH` samples the decorator computes
/// per-column RMS amplitudes and `try_send`s them to `amp_tx`. The channel
/// is capacity-1; full sends are dropped silently — the audio thread
/// NEVER blocks on the UI.
pub(crate) struct VisualizerSource<S: Source<Item = i16>> {
    pub(crate) inner: S,
    amp_tx: mpsc::Sender<Vec<f32>>,
    width: Arc<AtomicUsize>,
    buf: Vec<i16>,
}

impl<S: Source<Item = i16>> VisualizerSource<S> {
    /// Wrap `inner`. `width` is shared with `ui.rs` so renderer width
    /// changes feed back without a re-construction.
    pub(crate) fn new(inner: S, amp_tx: mpsc::Sender<Vec<f32>>, width: Arc<AtomicUsize>) -> Self {
        Self {
            inner,
            amp_tx,
            width,
            buf: Vec::with_capacity(VISUALIZER_BATCH),
        }
    }
}

impl<S: Source<Item = i16>> Iterator for VisualizerSource<S> {
    type Item = i16;
    fn next(&mut self) -> Option<i16> {
        let sample = self.inner.next()?;
        self.buf.push(sample);
        if self.buf.len() >= VISUALIZER_BATCH {
            let cols = self.width.load(Ordering::Relaxed).max(1);
            let amps = rms_per_col(&self.buf, cols);
            let _ = self.amp_tx.try_send(amps);
            self.buf.clear();
        }
        Some(sample)
    }
}

impl<S: Source<Item = i16>> Source for VisualizerSource<S> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

/// Per-column root-mean-square over a sample window.
/// Output values lie in `[0, 1]`.
#[must_use]
#[allow(clippy::cast_precision_loss, reason = "rms output is approximate")]
#[allow(
    clippy::cast_possible_truncation,
    reason = "f64 → f32 narrowing is intentional for the UI tap"
)]
pub(crate) fn rms_per_col(samples: &[i16], cols: usize) -> Vec<f32> {
    if cols == 0 {
        return Vec::new();
    }
    if samples.is_empty() {
        return vec![0.0; cols];
    }
    let bucket = (samples.len() / cols).max(1);
    let mut amps = Vec::with_capacity(cols);
    for c in 0..cols {
        let start = c * bucket;
        let end = (start + bucket).min(samples.len());
        if start >= end {
            amps.push(0.0);
            continue;
        }
        let mut sum_sq: f64 = 0.0;
        for &s in &samples[start..end] {
            let n = f64::from(s) / f64::from(i16::MAX);
            sum_sq += n * n;
        }
        let rms = (sum_sq / (end - start) as f64).sqrt();
        amps.push(rms as f32);
    }
    amps
}

/// Pull-based `rodio::Source` fed by the symphonia decode thread.
///
/// `try_recv` returns `0i16` (silence) when the queue is momentarily
/// empty so the audio thread keeps running rather than stalling. The
/// pre-roll guarantees the queue is non-empty when the first `play()`
/// fires, so silence-on-empty only happens during transient under-runs.
pub(crate) struct DecodedSource {
    rx: SyncReceiver<i16>,
    sample_rate: u32,
    channels: u16,
}

impl DecodedSource {
    pub(crate) fn new(rx: SyncReceiver<i16>, sample_rate: u32, channels: u16) -> Self {
        Self {
            rx,
            sample_rate,
            channels,
        }
    }
}

impl Iterator for DecodedSource {
    type Item = i16;
    fn next(&mut self) -> Option<i16> {
        // CRITICAL: this iterator is pulled from the cpal callback
        // thread. Blocking here stalls the audio device's render
        // callback, which CoreAudio (and Bluetooth codec packet-loss
        // concealment paths) react to by replicating the last rendered
        // frame — the audible "stuttering tail" symptom on station
        // switch. The contract is therefore: NEVER block. Always return
        // either a real sample or silence. The supervisor's
        // mute-on-switch + ready-flag-gated volume restore makes
        // pre-roll silence inaudible to the user.
        Some(self.rx.try_recv().unwrap_or(0))
    }
}

impl Source for DecodedSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{INITIAL_VOLUME_PCT, VISUALIZER_BATCH, VisualizerSource, rms_per_col, vol_to_gain};
    use rodio::Source;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    /// Constants are within reason — failure here means a design drift.
    #[test]
    fn constants_are_sane() {
        assert_eq!(VISUALIZER_BATCH, 1024);
        assert_eq!(INITIAL_VOLUME_PCT, 50);
    }

    #[test]
    fn vol_to_gain_zero_is_silent() {
        assert!((vol_to_gain(0)).abs() < f32::EPSILON);
    }

    #[test]
    fn vol_to_gain_full_is_unity() {
        assert!((vol_to_gain(100) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn vol_to_gain_clamps_above_full() {
        assert!((vol_to_gain(200) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn vol_to_gain_half() {
        assert!((vol_to_gain(50) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn rms_per_col_zero_cols_returns_empty() {
        assert!(rms_per_col(&[100, 200, 300], 0).is_empty());
    }

    #[test]
    fn rms_per_col_empty_samples_returns_zero_vec() {
        let v = rms_per_col(&[], 4);
        assert_eq!(v.len(), 4);
        assert!(v.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn rms_per_col_zero_signal_yields_zero() {
        let zeros = vec![0i16; 1000];
        let v = rms_per_col(&zeros, 8);
        assert!(v.iter().all(|&x| x == 0.0));
    }

    /// Maximum-amplitude i16 signal yields RMS ≈ 1.0.
    #[test]
    fn rms_per_col_full_scale_yields_one() {
        let full = vec![i16::MAX; 1000];
        let v = rms_per_col(&full, 4);
        for x in v {
            assert!((x - 1.0).abs() < 0.01, "expected ~1.0, got {x}");
        }
    }

    /// Generic test source that produces a known sample sequence.
    struct TestSource {
        samples: std::vec::IntoIter<i16>,
    }
    impl TestSource {
        fn new(s: Vec<i16>) -> Self {
            Self {
                samples: s.into_iter(),
            }
        }
    }
    impl Iterator for TestSource {
        type Item = i16;
        fn next(&mut self) -> Option<i16> {
            self.samples.next()
        }
    }
    impl Source for TestSource {
        fn current_frame_len(&self) -> Option<usize> {
            None
        }
        fn channels(&self) -> u16 {
            2
        }
        fn sample_rate(&self) -> u32 {
            44100
        }
        fn total_duration(&self) -> Option<Duration> {
            None
        }
    }

    /// Capacity-1 channel with try_send drops on full — audio thread
    /// NEVER blocks. We simulate by NOT consuming the receiver and
    /// pushing > 1 batch through the decorator.
    #[tokio::test(flavor = "current_thread")]
    async fn visualizer_source_drops_on_full_channel() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<Vec<f32>>(1);
        let width = Arc::new(AtomicUsize::new(64));
        let mut samples = Vec::with_capacity(VISUALIZER_BATCH * 4);
        for _ in 0..(VISUALIZER_BATCH * 4) {
            samples.push(i16::MAX / 2);
        }
        let source = TestSource::new(samples);
        let mut decorated = VisualizerSource::new(source, tx, width);
        // Pull all samples — must not panic, must not block.
        let mut count = 0;
        while decorated.next().is_some() {
            count += 1;
        }
        assert_eq!(count, VISUALIZER_BATCH * 4);
    }

    /// Visualizer width updates feed back via the shared atomic.
    #[tokio::test(flavor = "current_thread")]
    async fn visualizer_source_reads_dynamic_width() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<f32>>(4);
        let width = Arc::new(AtomicUsize::new(8));
        let samples = vec![i16::MAX / 2; VISUALIZER_BATCH];
        let mut decorated = VisualizerSource::new(TestSource::new(samples), tx, width.clone());
        for _ in 0..VISUALIZER_BATCH {
            let _ = decorated.next();
        }
        // First frame should arrive with 8 cols.
        let first = rx.try_recv().expect("frame emitted");
        assert_eq!(first.len(), 8);

        // Mutate width, re-feed batch, expect 16 cols on next frame.
        width.store(16, std::sync::atomic::Ordering::Relaxed);
        let extra = vec![i16::MAX / 2; VISUALIZER_BATCH];
        decorated.inner = TestSource::new(extra);
        for _ in 0..VISUALIZER_BATCH {
            let _ = decorated.next();
        }
        let second = rx.try_recv().expect("second frame emitted");
        assert_eq!(second.len(), 16);
    }
}
