// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Application state machine + input handling.
//!
//! [`App`] is mutated by `on_audio_event`, `on_terminal_event`, and
//! `on_tick`; rendered by `render::render_main`. [`Action`] is the
//! user-action enum produced by event handlers and consumed by
//! `App::dispatch`, which translates UI intents into supervisor
//! `AudioCommand`s. [`FinalState`] is the snapshot persisted by
//! `lib::run` on graceful exit so the next launch resumes where the
//! user left off.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use chrono::{DateTime, Local};
use ratatui::style::Style;
use tokio::sync::mpsc;
use tracing::warn;

use crate::audio::{AudioEvent, ConnectionState};
use crate::config::Config;
use crate::error::{NightrideError, Result};
use crate::metadata::{History, Metadata};
use crate::station::{self, Station};
use crate::theme::Theme;

use super::chrome::set_window_title;

/// Volume step per `+`/`-` press (also for mouse wheel).
pub(super) const VOLUME_STEP: u8 = 5;

/// Volume restored on `m` (unmute) when the user has no recorded
/// `pre_mute_volume` for this session — typically because the
/// previous session was saved muted (`default_volume_percent: 0`)
/// and the operator never touched the volume keys before pressing
/// `m`. Reading `config.default_volume` instead would re-mute (the
/// persisted value is 0), leaving the user stuck — so the canon
/// is a deliberate non-zero mid-band level.
pub(super) const UNMUTE_FALLBACK_VOLUME: u8 = 50;

/// Marquee pause at start and end of scroll cycle.
pub(super) const MARQUEE_PAUSE_TICKS: u32 = 8;

/// Per-frame step applied to `panel_visibility` toward `panel_target`.
/// At 30 fps a step of 0.15 settles the fade in ~7 frames (≈230 ms),
/// the canon "fast fade" target for the song panel + spectrum
/// visibility gate.
pub(super) const PANEL_VISIBILITY_STEP: f32 = 0.15;

/// Threshold under which the song panel + spectrum skip rendering
/// entirely. Anything above renders with a faded colour scaled by
/// `panel_visibility`.
pub(super) const PANEL_VISIBILITY_HIDE_THRESHOLD: f32 = 0.05;

/// User-action enum. Produced by event handlers, consumed by
/// [`App::dispatch`].
#[derive(Debug, Clone, Copy)]
pub enum Action {
    /// Quit the app cleanly.
    Quit,
    /// Volume +5%.
    VolumeUp,
    /// Volume -5%.
    VolumeDown,
    /// Toggle mute (0 ↔ default volume).
    MuteToggle,
    /// Next station in the registry order.
    NextStation,
    /// Previous station in the registry order.
    PrevStation,
    /// Toggle transport format (MP3 ⇄ HLS).
    TransportToggle,
}

/// Snapshot of UI state at the moment `run` returns. `lib::run` uses this
/// to persist the latest station + volume to `nightride-tui.md` so the
/// next launch resumes where the user left off.
#[derive(Debug, Clone)]
pub struct FinalState {
    /// Slug of the station that was active when the UI exited.
    pub station_slug: String,
    /// Volume percent (0..=100) at exit time.
    pub volume: u8,
}

/// Application state machine. Mutated by [`App::on_audio_event`],
/// [`App::on_terminal_event`], and [`App::on_tick`]; rendered by
/// `render::render_main`.
#[derive(Debug)]
pub struct App {
    /// Resolved theme tokens.
    pub theme: Theme,
    /// Loaded user configuration.
    pub config: Config,
    /// Active audio target — the station the supervisor is asked to
    /// play. Set immediately on user action (←/→) so the audio task
    /// can begin transitioning at once.
    pub current_station: &'static Station,
    /// Station whose accent is currently driving the painted UI. Held
    /// in lockstep with `current_station` while the panel is fully
    /// visible; on station switch it lags behind during the fade-out
    /// and only swaps to the new target the moment the new stream
    /// reaches `Streaming` — so no surface ever changes colour while
    /// the OLD station's audio is still leaving the screen.
    pub displayed_station: &'static Station,
    /// Current volume display (0..=100). Mirrors the audio task's
    /// internal atomic; ground-truth lives in audio.rs.
    pub volume: u8,
    /// Volume captured at the moment of the most recent mute, or
    /// `None` if the user hasn't muted yet in this session.
    /// Pressing `m` while at zero restores from this slot rather
    /// than reading `config.default_volume` — otherwise resuming a
    /// session that was saved muted (`default_volume_percent: 0`)
    /// would leave the user stuck at zero with no way to unmute.
    pub(crate) pre_mute_volume: Option<u8>,
    /// Current connection state from the audio task.
    pub connection: ConnectionState,
    /// Last metadata snapshot.
    pub metadata: Option<Metadata>,
    /// Bounded history ring (cap 10).
    pub history: History,
    /// Latest visualizer amplitude frame (carried as fallback when the
    /// channel is empty — avoids flicker from spatial blur).
    pub last_amp: Option<Vec<f32>>,
    /// Wall-clock instant the active stream entered Streaming state
    /// (used by the dual-timestamp display).
    pub stream_started_at: Option<DateTime<Local>>,
    /// Monotonic instant the app launched. Drives the uptime counter
    /// in the header cluster (`HHH:MM:SS` since launch). `Instant`
    /// because wall-clock changes (NTP, DST) must NOT skew uptime.
    pub app_launched_at: std::time::Instant,
    /// Marquee scroll offset (in graphemes).
    pub(crate) marquee_offset: usize,
    /// Marquee pause counter (ticks remaining at endpoints).
    pub(crate) marquee_pause: u32,
    /// Width tap shared with `audio::VisualizerSource`.
    pub(crate) visualizer_width: Arc<AtomicUsize>,
    /// Quit requested.
    pub should_quit: bool,
    /// Sender to the save-debouncer task in `lib::run`. Every action
    /// that mutates persisted state pushes a snapshot here.
    pub(crate) save_tx: mpsc::Sender<FinalState>,
    /// Lerped 0..=1 visibility for the song-panel + spectrum row. Driven
    /// toward `panel_target` once per render frame; hides the now-playing
    /// box and oscilloscope completely when audio is not actually
    /// streaming.
    pub(crate) panel_visibility: f32,
    /// Target visibility set by `on_audio_event`. 1.0 when the supervisor
    /// reports `ConnectionState::Streaming`, 0.0 in every other state
    /// (Idle, Connecting, Reconnecting, Error).
    pub(crate) panel_target: f32,
    /// Tag of the latest available release when a newer version exists on
    /// GitHub, or `None` while the background check is pending or when the
    /// running binary is already current. Set once by the update-check task
    /// in `ui::run`; never cleared during the session.
    pub update_available: Option<String>,
    /// Monotonic tick counter for the Braille loading spinner. Advances
    /// once per `on_tick` call; the rendered glyph is
    /// `theme.glyphs.spinner_frames[(phase / TICKS_PER_SPINNER_FRAME)
    /// % len()]`. Keeping the raw counter (instead of the frame index)
    /// lets the spinner speed change without resetting visual position.
    pub(crate) spinner_phase: u32,
}

impl App {
    /// Build a fresh app from a loaded `Config` and a shared visualizer
    /// width slot.
    ///
    /// # Errors
    /// Returns [`NightrideError::Validation`] when `config.default_station`
    /// does not match a known station slug.
    pub fn new(
        config: Config,
        visualizer_width: Arc<AtomicUsize>,
        save_tx: mpsc::Sender<FinalState>,
    ) -> Result<Self> {
        let station = station::by_slug(&config.default_station).ok_or_else(|| {
            NightrideError::Validation {
                op: "ui::App::new",
                field: "default_station",
                detail: format!("unknown station slug {}", config.default_station),
            }
        })?;
        Ok(Self {
            theme: Theme::detect(),
            volume: config.default_volume,
            pre_mute_volume: None,
            current_station: station,
            displayed_station: station,
            connection: ConnectionState::Idle,
            metadata: None,
            history: History::new(),
            last_amp: None,
            stream_started_at: None,
            app_launched_at: std::time::Instant::now(),
            marquee_offset: 0,
            marquee_pause: MARQUEE_PAUSE_TICKS,
            visualizer_width,
            should_quit: false,
            save_tx,
            config,
            panel_visibility: 0.0,
            panel_target: 1.0,
            update_available: None,
            spinner_phase: 0,
        })
    }

    /// Skip-render gate: when the song-panel group is below the
    /// visibility threshold every render fn that participates in the
    /// "now-playing" lockstep skips its work. Centralised here so
    /// future visibility refactors stay single-sourced.
    #[must_use]
    pub(crate) fn panel_hidden(&self) -> bool {
        self.panel_visibility < PANEL_VISIBILITY_HIDE_THRESHOLD
    }

    /// Toggle mute and return the new volume value to push to the
    /// audio supervisor.
    ///
    /// - Volume > 0 → save as `pre_mute_volume`, drop volume to 0.
    /// - Volume == 0 → restore from `pre_mute_volume` if present;
    ///   otherwise fall back to a sensible non-zero default (50)
    ///   because reading `config.default_volume` would re-mute when
    ///   the persisted state already had `default_volume_percent: 0`
    ///   (that's the bug this helper exists to fix).
    pub fn mute_toggle(&mut self) -> u8 {
        if self.volume > 0 {
            self.pre_mute_volume = Some(self.volume);
            self.volume = 0;
        } else {
            self.volume = self.pre_mute_volume.unwrap_or(UNMUTE_FALLBACK_VOLUME);
        }
        self.volume
    }

    /// Apply a user-driven station switch.
    ///
    /// Beyond updating `current_station`, this clears the metadata
    /// snapshot, the song-elapsed timestamp, and the marquee scroll —
    /// otherwise the now-playing line would keep displaying the
    /// PREVIOUS station's title/artist until the new stream produces
    /// its first ICY-metaint frame (4–30 s depending on `metaint`),
    /// which the user perceives as a refresh lag. `displayed_station`
    /// swaps in lockstep so every painted surface (now-playing line,
    /// volume pill, spectrum, header trio) reflects the user's intent
    /// immediately while the audio supervisor connects.
    pub fn change_station(&mut self, st: &'static Station) {
        self.current_station = st;
        self.displayed_station = st;
        self.metadata = None;
        self.stream_started_at = None;
        self.marquee_offset = 0;
        self.marquee_pause = MARQUEE_PAUSE_TICKS;
        set_window_title(&self.window_title_text());
    }

    /// Push a snapshot to the save-debouncer. Best-effort: if the
    /// channel is full or closed we drop the request silently — the
    /// debouncer will catch up on the next mutation.
    pub(super) fn request_save(&self) {
        let _ = self.save_tx.try_send(FinalState {
            station_slug: self.current_station.slug.to_string(),
            volume: self.volume,
        });
    }

    /// Apply an audio event coming from the supervisor.
    pub fn on_audio_event(&mut self, evt: AudioEvent) {
        match evt {
            AudioEvent::Metadata(md) => self.apply_metadata(md),
            AudioEvent::ConnectionState(state) => self.apply_connection_state(state),
            AudioEvent::Error(detail) => warn!(detail = %detail, "audio error"),
        }
    }

    /// Branch of [`App::on_audio_event`] handling new metadata.
    fn apply_metadata(&mut self, md: Metadata) {
        if !md.is_empty() {
            // Detect a new track — title OR artist changed
            // versus the previous metadata frame. This is the
            // canonical signal for "song boundary" because
            // ICY-MetaData on Nightride re-emits the same
            // `StreamTitle` for every interleave window during
            // a single track and only changes it when the next
            // track begins.
            let is_new_song = match &self.metadata {
                None => true,
                Some(prev) => prev.title != md.title || prev.artist != md.artist,
            };
            if is_new_song {
                // Reset the song-elapsed counter in the
                // floating header so the third field always
                // reads "time since THIS song started", not
                // "time since the stream connected".
                self.stream_started_at = Some(Local::now());
            }
            self.history.push_distinct(crate::metadata::HistoryItem {
                station_slug: self.current_station.slug,
                metadata: md.clone(),
                at: std::time::SystemTime::now(),
            });
        }
        self.metadata = Some(md);
        self.marquee_offset = 0;
        self.marquee_pause = MARQUEE_PAUSE_TICKS;
        set_window_title(&self.window_title_text());
    }

    /// Branch of [`App::on_audio_event`] handling connection state changes.
    fn apply_connection_state(&mut self, state: ConnectionState) {
        match &state {
            ConnectionState::Streaming { .. } => {
                self.stream_started_at = Some(Local::now());
                // Belt + braces: `change_station` already keeps
                // displayed_station in lockstep with current_station,
                // but a server-side redirect or supervisor-internal
                // restart could arrive without going through that
                // path — re-pin the accent here so painted surfaces
                // never drift from the audible station.
                self.displayed_station = self.current_station;
            }
            _ => {
                // Outside of Streaming there is no live elapsed
                // counter to anchor; clear the wall-clock so the
                // header trio drops the song-time field instead of
                // showing a stale value.
                self.stream_started_at = None;
            }
        }
        self.connection = state;
        // The now-playing line surfaces the connection state itself
        // (connecting…, reconnecting… (N), error: …, idle) when audio
        // is not streaming, so the OS title is rebuilt every time
        // and the panel never hides — `panel_target` stays at 1.0
        // throughout the lifecycle.
        set_window_title(&self.window_title_text());
    }

    /// Background tick: marquee advancement, live clock refresh.
    pub fn on_tick(&mut self) {
        if self.marquee_pause > 0 {
            self.marquee_pause -= 1;
            return;
        }
        let len = self.now_playing_line().chars().count();
        if len == 0 {
            return;
        }
        self.marquee_offset = (self.marquee_offset + 1) % len.max(1);
        if self.marquee_offset == 0 {
            self.marquee_pause = MARQUEE_PAUSE_TICKS;
        }
    }

    /// Current Braille spinner glyph for loading states.
    pub(crate) fn spinner_glyph(&self) -> char {
        // 2 ticks per frame at 30 fps tick rate → 15 fps spinner, matches
        // the cli-spinners "dots" cadence used by opencode / claude-code.
        const TICKS_PER_FRAME: u32 = 2;
        let frames = self.theme.glyphs.spinner_frames;
        if frames.is_empty() {
            return ' ';
        }
        let idx = ((self.spinner_phase / TICKS_PER_FRAME) as usize) % frames.len();
        frames[idx]
    }

    /// `true` while audio is establishing or recovering — spinner-eligible.
    pub(crate) fn is_loading(&self) -> bool {
        matches!(
            self.connection,
            ConnectionState::Connecting { .. } | ConnectionState::Reconnecting { .. }
        )
    }

    /// `true` while the spectrum row should display the `tuning .` ping-
    /// pong placeholder instead of bars: either we are mid-connect, or
    /// we are streaming but the metadata has not yet arrived AND the
    /// stream is too young to give up on. Capped at
    /// `TUNING_GRACE_SECS` post-Streaming so an ICY-less MP3 station
    /// (or an unusually slow SSE first-emit) doesn't trap the UI in
    /// "tuning" forever — after the grace window the bars render and
    /// the now-playing line collapses to `[ STATION ]`.
    pub(crate) fn is_tuning(&self) -> bool {
        const TUNING_GRACE_SECS: i64 = 5;
        if matches!(self.connection, ConnectionState::Connecting { .. }) {
            return true;
        }
        if matches!(self.connection, ConnectionState::Streaming { .. }) {
            let no_meta = match &self.metadata {
                None => true,
                Some(md) => md.is_empty(),
            };
            if !no_meta {
                return false;
            }
            return match self.stream_started_at {
                Some(started) => (Local::now() - started).num_seconds().max(0) < TUNING_GRACE_SECS,
                None => true,
            };
        }
        false
    }

    /// Current ping-pong dot frame for the `tuning .` placeholder. Five
    /// frames `. → . . → . . . → . . → .` at ~7 fps so the cadence reads
    /// as "thinking" without feeling jittery against the 30 fps base.
    pub(crate) fn tuning_dots(&self) -> &'static str {
        const TICKS_PER_FRAME: u32 = 4;
        const FRAMES: [&str; 5] = [".", "..", "...", "..", "."];
        let idx = ((self.spinner_phase / TICKS_PER_FRAME) as usize) % FRAMES.len();
        FRAMES[idx]
    }

    /// Apply a visualizer amplitude frame.
    pub fn on_amp_frame(&mut self, amp: Vec<f32>) {
        self.last_amp = Some(amp);
    }

    /// Per-render-frame lerp for the song-panel + spectrum visibility
    /// gate. Called by `lib::run` immediately before each draw so the
    /// fade resolution matches `FRAME_RATE`. Idempotent at the target
    /// value: once `panel_visibility == panel_target`, calls are no-ops.
    pub fn on_render_tick(&mut self) {
        // Spinner phase advances every render frame (30 fps). The
        // renderer divides by `TICKS_PER_FRAME` to get the visible
        // cadence, so spinner cadence rides on the render rate, not
        // the slower bookkeeping tick (4 Hz).
        self.spinner_phase = self.spinner_phase.wrapping_add(1);
        if (self.panel_visibility - self.panel_target).abs() < PANEL_VISIBILITY_STEP {
            self.panel_visibility = self.panel_target;
            return;
        }
        if self.panel_visibility < self.panel_target {
            self.panel_visibility += PANEL_VISIBILITY_STEP;
        } else {
            self.panel_visibility -= PANEL_VISIBILITY_STEP;
        }
    }

    /// Status-text body inserted between `STATION` and the closing
    /// bracket when audio is not streaming. Mirrored byte-for-byte by
    /// the OS window title (without the `// `/` ` padding).
    fn connection_status_text(&self) -> Option<String> {
        match &self.connection {
            ConnectionState::Streaming { .. } => None,
            ConnectionState::Connecting { .. } => Some(format!("tuning {}", self.tuning_dots())),
            ConnectionState::Reconnecting { attempt, .. } => {
                Some(format!("reconnecting… (attempt {attempt})"))
            }
            ConnectionState::Error { detail, .. } => Some(format!("error: {detail}")),
            ConnectionState::Idle => Some("idle".to_string()),
        }
    }

    /// Plain (unstyled) version of the now-playing line as rendered
    /// inside the panel. Drives the marquee length calculation. Wraps
    /// the body in `[ … ]` brackets and pads the `//` + `/` separators
    /// with single breathing cells so the segments do not glue. When
    /// audio is not streaming, surfaces the connection state in the
    /// place where artist + title would otherwise sit.
    pub(crate) fn now_playing_line(&self) -> String {
        let station = self.displayed_station.display_name;
        let sep = self.theme.glyphs.now_separator;
        if let Some(status) = self.connection_status_text() {
            return if self.is_loading() {
                format!("[ {station} // {} {status} ]", self.spinner_glyph())
            } else {
                format!("[ {station} // {status} ]")
            };
        }
        match &self.metadata {
            Some(md) if md.title.is_some() || md.artist.is_some() => {
                let title = md.title.as_deref().unwrap_or("?");
                let artist = md.artist.as_deref().unwrap_or("?");
                format!("[ {station} // {artist} {sep} {title} ]")
            }
            // Streaming but metadata not arrived yet: surface the
            // `tuning .` ping-pong while we are within the grace
            // window; after that, collapse to `[ STATION ]` so the UI
            // doesn't pretend to be loading forever on metadata-less
            // streams.
            _ => {
                if self.is_tuning() {
                    format!("[ {station} // tuning {} ]", self.tuning_dots())
                } else {
                    format!("[ {station} ]")
                }
            }
        }
    }

    /// Build the OS window-title string written via OSC 0. The chrome
    /// title carries the same fields as the marquee but in a compact
    /// no-space, no-bracket form so the OS title bar stays terse:
    /// `STATION//Artist/Title` while streaming, `STATION//<status>`
    /// otherwise (connecting…, reconnecting…, error: …, idle).
    fn window_title_text(&self) -> String {
        let station = self.displayed_station.display_name;
        let sep = self.theme.glyphs.now_separator;
        if let Some(status) = self.connection_status_text() {
            return format!("{station}//{status}");
        }
        match &self.metadata {
            Some(md) if md.title.is_some() || md.artist.is_some() => {
                let title = md.title.as_deref().unwrap_or("?");
                let artist = md.artist.as_deref().unwrap_or("?");
                format!("{station}//{artist}{sep}{title}")
            }
            _ => {
                if self.is_tuning() {
                    format!("{station}//tuning {}", self.tuning_dots())
                } else {
                    station.to_string()
                }
            }
        }
    }

    /// Per-segment styled view of the now-playing line. Composition:
    ///
    /// - `[ ` opening bracket + breathing cell in accent — replaces
    ///   the retired rounded border as the visual frame.
    /// - `STATION` painted in the active station accent.
    /// - ` // ` boundary between station and artist, accent, padded.
    /// - `Artist` neutral light tone — standout content layer.
    /// - ` / ` separator, accent, padded.
    /// - `Title` accent (mirrors the station + artist hierarchy in the
    ///   accent palette).
    /// - ` ]` breathing cell + closing bracket in accent.
    ///
    /// When the connection is not streaming the line collapses to
    /// `[ STATION // <status> ]` with the status (`connecting…`,
    /// `reconnecting… (N)`, `error: …`, `idle`) painted in the neutral
    /// light tone so it reads as the active content layer instead of
    /// the chrome.
    pub(crate) fn now_playing_segments(&self) -> Vec<(String, Style)> {
        let station = self.displayed_station.display_name;
        let sep = self.theme.glyphs.now_separator;
        let accent = self.theme.accent_style(self.displayed_station);
        let neutral = self.theme.text_neutral_style();
        if let Some(status) = self.connection_status_text() {
            // No braille spinner inside the brackets: the spinner now
            // lives on the spectrum row next to `MP3 ` / `HLS `. The
            // `tuning .` text already animates via dots ping-pong.
            return vec![
                ("[ ".to_string(), accent),
                (station.to_string(), accent),
                (" // ".to_string(), accent),
                (status, neutral),
                (" ]".to_string(), accent),
            ];
        }
        match &self.metadata {
            Some(md) if md.title.is_some() || md.artist.is_some() => {
                let title = md.title.as_deref().unwrap_or("?");
                let artist = md.artist.as_deref().unwrap_or("?");
                vec![
                    ("[ ".to_string(), accent),
                    (station.to_string(), accent),
                    (" // ".to_string(), accent),
                    (artist.to_string(), neutral),
                    (format!(" {sep} "), accent),
                    (title.to_string(), accent),
                    (" ]".to_string(), accent),
                ]
            }
            // Streaming but metadata not arrived yet: tuning while in
            // grace window, otherwise collapse to `[ STATION ]`.
            _ => {
                if self.is_tuning() {
                    vec![
                        ("[ ".to_string(), accent),
                        (station.to_string(), accent),
                        (" // ".to_string(), accent),
                        (format!("tuning {}", self.tuning_dots()), neutral),
                        (" ]".to_string(), accent),
                    ]
                } else {
                    vec![
                        ("[ ".to_string(), accent),
                        (station.to_string(), accent),
                        (" ]".to_string(), accent),
                    ]
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
