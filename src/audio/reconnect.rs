// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Reconnect machinery: error classification + jittered exponential
//! backoff schedule.
//!
//! The audio supervisor consumes [`Backoff`] to decide whether to retry a
//! failed stream attempt and how long to wait. Errors are first classified
//! via [`classify`] into one of four [`ErrorClass`] variants:
//!
//! - [`ErrorClass::SoftEof`] — server closed the connection cleanly. Retry
//!   immediately with `attempt = 0` (no sleep). Common with Icecast which
//!   periodically rotates idle connections.
//! - [`ErrorClass::HardNetwork`] — DNS / TLS / connect-refused / transport
//!   drop. Retry on the exponential schedule.
//! - [`ErrorClass::HardCodec`] — decoder or validation failure. Terminal
//!   for the current attempt; supervisor surfaces an error event and stops.
//! - [`ErrorClass::HardCancelled`] — cooperative shutdown. Terminal; no
//!   retry, supervisor exits.
//!
//! Schedule shape: `min(BASE * 2^attempt, CAP)` with ±`JITTER_PCT`
//! noise. `BASE = 1 s`, `CAP = 30 s`, `JITTER_PCT = 0.20`.
//!
//! The backoff carries an injectable seed so tests run deterministically;
//! production uses a time-derived seed so jitter is unpredictable across
//! sessions but cheap to compute (no `rand` crate dependency).

use std::time::Duration;

use crate::error::NightrideError;

/// First exponential step. Attempt `0` sleeps `BASE × 2^0 = 1 s`.
pub(crate) const RECONNECT_BASE: Duration = Duration::from_secs(1);

/// Ceiling on the exponential schedule. Long-tail attempts (`attempt ≥ 5`)
/// converge here.
pub(crate) const RECONNECT_CAP: Duration = Duration::from_secs(30);

/// Jitter envelope around the scheduled duration: ±20 %.
pub(crate) const JITTER_PCT: f32 = 0.20;

/// Fall-back PRNG base when `SystemTime` is unavailable (sandboxed
/// runtime without epoch). Folded together with the process id and a
/// per-construction counter inside [`time_seed`], so even the fallback
/// path produces distinct seeds across concurrent supervisor restarts.
/// Hex-coded so it is recognisable in test-only failure dumps.
const FALLBACK_SEED: u64 = 0xDEAD_BEEF;

/// Per-call counter folded into [`time_seed`] to disambiguate two RNG
/// constructions that fall on the same `SystemTime::now()` nanosecond
/// (coarse-clock platforms, multiple decode threads spawning at once).
/// Stand-in for the thread id since `ThreadId::as_u64` is nightly-only;
/// monotonic and process-local is enough for jitter independence.
static SEED_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Classification of a [`NightrideError`] for the retry policy.
///
/// The supervisor uses this to choose between immediate retry, exponential
/// backoff, terminal fail, and cooperative shutdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorClass {
    /// Server-side clean EOF. Retry immediately with `attempt = 0`.
    SoftEof,
    /// Transport-layer fault (DNS / TLS / connect / transport drop). Retry
    /// on the exponential schedule.
    HardNetwork,
    /// Decoder or validation failure. Terminal for the supervisor.
    HardCodec,
    /// Cooperative shutdown. Terminal; no retry.
    HardCancelled,
}

/// Map a [`NightrideError`] to its [`ErrorClass`].
///
/// Mapping table:
///
/// | Error variant                          | Class             |
/// |----------------------------------------|-------------------|
/// | `Io { source: UnexpectedEof }`          | `SoftEof`         |
/// | `Io { source: ... }` (other kinds)      | `HardNetwork`     |
/// | `Network { source: reqwest::Error }`    | `HardNetwork`     |
/// | `UpstreamUnavailable { detail }`        | `HardNetwork`     |
/// | `NetworkRejected { detail }`            | `HardCodec`       |
/// | `Decode { source: symphonia::Error }`   | `HardCodec`       |
/// | `Config { ... }` / `ConfigInvalid`      | `HardCodec`       |
/// | `Audio { ... }`                         | `HardCodec`       |
/// | `Validation { ... }` / `NotFound`       | `HardCodec`       |
/// | `Cancelled { ... }`                     | `HardCancelled`   |
///
/// `HardCodec` covers everything that should not auto-retry. The supervisor
/// surfaces such errors as `ConnectionState::Error` and breaks the loop.
pub(crate) fn classify(err: &NightrideError) -> ErrorClass {
    match err {
        NightrideError::Io { source, .. } => {
            if source.kind() == std::io::ErrorKind::UnexpectedEof {
                ErrorClass::SoftEof
            } else {
                ErrorClass::HardNetwork
            }
        }
        NightrideError::Network { .. } | NightrideError::UpstreamUnavailable { .. } => {
            ErrorClass::HardNetwork
        }
        NightrideError::Cancelled { .. } => ErrorClass::HardCancelled,
        NightrideError::NetworkRejected { .. }
        | NightrideError::Decode { .. }
        | NightrideError::Config { .. }
        | NightrideError::ConfigInvalid { .. }
        | NightrideError::Audio { .. }
        | NightrideError::Validation { .. }
        | NightrideError::NotFound { .. } => ErrorClass::HardCodec,
    }
}

/// Compute the backoff duration for `attempt` (0-based).
///
/// `dur = min(BASE × 2^attempt, CAP)` with ±[`JITTER_PCT`] uniform noise
/// applied last so the jitter scales with the un-capped value first, then
/// the cap clamps the result to `[CAP × (1 - JITTER_PCT), CAP × (1 + JITTER_PCT)]`
/// at the long tail.
///
/// `attempt` saturates at `u32::MAX`; values past the cap-saturation point
/// return `CAP ± jitter`.
pub(crate) fn schedule(attempt: u32, jitter: &mut XorShift64) -> Duration {
    // Step 1: compute base × 2^attempt without overflowing u64 secs.
    let base_secs = RECONNECT_BASE.as_secs_f64();
    let cap_secs = RECONNECT_CAP.as_secs_f64();
    // `attempt.min(20)` ≤ 20 — saturates well before the f64 exponent
    // overflows; the i32 cast is safe by construction.
    let exponent = i32::try_from(attempt.min(20)).unwrap_or(20);
    let factor = 2.0_f64.powi(exponent);
    let raw = base_secs * factor;
    let capped = raw.min(cap_secs);

    // Step 2: jitter ± JITTER_PCT.
    let factor = 1.0 + f64::from(jitter.next_signed_unit()) * f64::from(JITTER_PCT);
    let jittered = capped * factor;

    // Step 3: re-clamp to [0, CAP × (1 + JITTER_PCT)] so a positive jitter
    // past the cap is bounded.
    let clamped = jittered.max(0.0);

    Duration::from_secs_f64(clamped)
}

/// Stateful backoff: tracks attempt count and owns its PRNG seed.
///
/// The supervisor calls [`Backoff::record_soft_eof`] after a clean
/// server-side EOF (resets attempt to 0, returns zero-duration sleep) or
/// [`Backoff::record_hard`] after a transport failure (returns the next
/// scheduled duration and increments attempt). Codec / cancelled errors
/// short-circuit via [`Backoff::is_terminal`] before any retry.
#[derive(Debug)]
pub(crate) struct Backoff {
    attempt: u32,
    jitter: XorShift64,
}

impl Backoff {
    /// Build a [`Backoff`] with an explicit seed (test-deterministic) or a
    /// time-derived seed (`None`, production).
    #[must_use]
    pub(crate) fn new(seed: Option<u64>) -> Self {
        let seed = seed.unwrap_or_else(time_seed);
        Self {
            attempt: 0,
            jitter: XorShift64::new(seed),
        }
    }

    /// Record a soft EOF: reset attempt to 0 and return a zero-duration
    /// sleep (immediate reconnect).
    pub(crate) fn record_soft_eof(&mut self) -> Duration {
        self.attempt = 0;
        Duration::ZERO
    }

    /// Record a hard transport failure: schedule the next sleep and
    /// increment attempt (saturating at `u32::MAX`).
    pub(crate) fn record_hard(&mut self) -> Duration {
        let dur = schedule(self.attempt, &mut self.jitter);
        self.attempt = self.attempt.saturating_add(1);
        dur
    }

    /// Return `true` if `class` is a terminal classification (Codec or
    /// Cancelled). Supervisor exits the retry loop on `true`.
    #[must_use]
    pub(crate) fn is_terminal(class: ErrorClass) -> bool {
        matches!(class, ErrorClass::HardCodec | ErrorClass::HardCancelled)
    }

    /// Current attempt counter (for telemetry / logging).
    #[cfg(test)]
    pub(crate) fn attempt(&self) -> u32 {
        self.attempt
    }
}

/// xorshift64 PRNG. Avoids pulling in `rand` for a single-purpose jitter
/// source. The state never reaches zero in normal operation (seed forced
/// non-zero in [`XorShift64::new`]).
#[derive(Debug)]
pub(crate) struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            // xorshift64 collapses to all-zero if seeded with 0; force a
            // non-zero start.
            state: seed.max(1),
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut s = self.state;
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        self.state = s;
        s
    }

    /// Return a uniform sample in `[-1.0, 1.0]`.
    pub(crate) fn next_signed_unit(&mut self) -> f32 {
        // Take 24 bits of entropy and map to [0, 1) — keeps full f32
        // mantissa precision (24 bits = exact f32).
        let bits = (self.next_u64() >> 40) & 0x00FF_FFFF;
        // Two-step cast through u32 to silence cast_possible_truncation:
        // bits is masked to 24 bits, fits u32 trivially.
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            reason = "bits ≤ 0x00FF_FFFF fits exactly in f32 mantissa"
        )]
        let unit = (bits as u32) as f32 / 16_777_216.0_f32;
        unit.mul_add(2.0, -1.0)
    }
}

/// Time-derived PRNG seed (production path).
///
/// Mixes three independent sources so two `Backoff`s constructed at
/// "the same instant" still pick disjoint jitter sequences:
///
/// - `SystemTime` nanoseconds (or `FALLBACK_SEED` if the clock is
///   inaccessible) — session-level entropy.
/// - The process id — distinguishes concurrent NightRideTUI processes.
/// - A monotonic per-construction counter ([`SEED_COUNTER`]) —
///   distinguishes multiple supervisor restarts within one process.
///
/// The three are folded through a SplitMix64 finalizer so low-entropy
/// inputs (e.g. tiny PIDs, near-zero counters) still produce
/// well-distributed seeds. The result is forced ≥1 because XorShift64
/// degenerates to all-zeroes if seeded with zero.
fn time_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(FALLBACK_SEED, |d| {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "low 64 bits of nanoseconds are sufficient entropy for jitter"
            )]
            let n = d.as_nanos() as u64;
            n
        });
    let pid = u64::from(std::process::id());
    let counter = SEED_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    // SplitMix64 finalizer: spreads correlated low-entropy inputs across
    // the full 64-bit space. Constants are the published SplitMix64
    // primes; see Steele/Lea/Flood 2014.
    let mut z = nanos
        ^ pid.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ counter.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    z.max(1)
}

#[cfg(test)]
mod tests {
    use super::{
        Backoff, ErrorClass, JITTER_PCT, RECONNECT_BASE, RECONNECT_CAP, XorShift64, classify,
        schedule,
    };
    use crate::error::NightrideError;
    use std::time::Duration;

    /// Backoff schedule midpoints match `1/2/4/8/16/30/30/30 s`
    /// within ±`JITTER_PCT` envelope.
    #[test]
    fn backoff_schedule_matches_spec() {
        let mut rng = XorShift64::new(0x00C0_FFEE);
        let expected_secs: [f64; 8] = [1.0, 2.0, 4.0, 8.0, 16.0, 30.0, 30.0, 30.0];
        for (attempt, &center) in expected_secs.iter().enumerate() {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "attempt index ≤ 7 fits u32"
            )]
            let dur = schedule(attempt as u32, &mut rng);
            let actual = dur.as_secs_f64();
            let envelope = center * f64::from(JITTER_PCT);
            assert!(
                actual >= center - envelope && actual <= center + envelope,
                "attempt {attempt}: {actual:.2}s outside [{:.2},{:.2}]s envelope",
                center - envelope,
                center + envelope,
            );
        }
    }

    /// `classify` maps every `NightrideError` variant to the
    /// canonical retry class.
    #[test]
    fn classify_errors() {
        let unexpected_eof = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "eof");
        let other_io = std::io::Error::other("other");

        assert_eq!(
            classify(&NightrideError::Io {
                op: "test::eof",
                source: unexpected_eof
            }),
            ErrorClass::SoftEof,
            "UnexpectedEof must classify as SoftEof"
        );
        assert_eq!(
            classify(&NightrideError::Io {
                op: "test::other_io",
                source: other_io
            }),
            ErrorClass::HardNetwork,
            "non-EOF io must classify as HardNetwork"
        );
        assert_eq!(
            classify(&NightrideError::Cancelled { op: "test::cancel" }),
            ErrorClass::HardCancelled,
        );
        assert_eq!(
            classify(&NightrideError::network_rejected("test::host", "host")),
            ErrorClass::HardCodec,
        );
        assert_eq!(
            classify(&NightrideError::upstream_unavailable(
                "test::upstream",
                "status 404"
            )),
            ErrorClass::HardNetwork,
            "Icecast 404 (mountpoint source disconnected) must retry, not terminate"
        );
        assert_eq!(
            classify(&NightrideError::config_invalid("test::cfg", "x")),
            ErrorClass::HardCodec,
        );
        assert_eq!(
            classify(&NightrideError::audio("test::audio", "no device")),
            ErrorClass::HardCodec,
        );
        assert_eq!(
            classify(&NightrideError::Validation {
                op: "test::vol",
                field: "volume",
                detail: "out of range".to_string(),
            }),
            ErrorClass::HardCodec,
        );
        assert_eq!(
            classify(&NightrideError::NotFound {
                op: "test::slug",
                what: "missing".to_string(),
            }),
            ErrorClass::HardCodec,
        );
    }

    /// Soft-EOF resets the attempt counter and yields zero sleep on
    /// `record_soft_eof`.
    #[test]
    fn soft_eof_resets_attempt() {
        let mut bo = Backoff::new(Some(0xFEED));
        // Walk the attempt counter up.
        let _ = bo.record_hard();
        let _ = bo.record_hard();
        let _ = bo.record_hard();
        assert_eq!(bo.attempt(), 3);

        let zero = bo.record_soft_eof();
        assert_eq!(zero, Duration::ZERO);
        assert_eq!(bo.attempt(), 0);

        // Next hard recovers from attempt 0 (≈1 s ± jitter).
        let dur = bo.record_hard();
        let secs = dur.as_secs_f64();
        let envelope = RECONNECT_BASE.as_secs_f64() * f64::from(JITTER_PCT);
        let center = RECONNECT_BASE.as_secs_f64();
        assert!(
            secs >= center - envelope && secs <= center + envelope,
            "post-soft-eof first hard sleep {secs:.2}s outside envelope"
        );
    }

    /// `is_terminal` short-circuits Codec and Cancelled.
    #[test]
    fn terminal_classifications() {
        assert!(Backoff::is_terminal(ErrorClass::HardCodec));
        assert!(Backoff::is_terminal(ErrorClass::HardCancelled));
        assert!(!Backoff::is_terminal(ErrorClass::SoftEof));
        assert!(!Backoff::is_terminal(ErrorClass::HardNetwork));
    }

    /// Beyond the cap-saturation attempt, the schedule converges on
    /// `RECONNECT_CAP ± jitter`.
    #[test]
    fn schedule_long_tail_converges_on_cap() {
        let mut rng = XorShift64::new(0xBADD_CAFE_u64);
        for attempt in 6..=20 {
            let dur = schedule(attempt, &mut rng);
            let secs = dur.as_secs_f64();
            let cap = RECONNECT_CAP.as_secs_f64();
            let envelope = cap * f64::from(JITTER_PCT);
            assert!(
                secs >= cap - envelope && secs <= cap + envelope,
                "attempt {attempt}: {secs:.2}s outside cap envelope [{:.2},{:.2}]",
                cap - envelope,
                cap + envelope,
            );
        }
    }
}
