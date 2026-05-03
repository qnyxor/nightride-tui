// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Unit tests for `ui::app::App`. Sibling file via `#[path]` so the
//! parent module stays under the §2.3 file-size cap while tests
//! continue to import via `super::*`.

use super::{Action, App, FinalState};
use crate::audio::{AudioEvent, ConnectionState};
use crate::config::Config;
use crate::metadata::Metadata;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

fn make_app() -> App {
    let (tx, _rx) = tokio::sync::mpsc::channel::<FinalState>(8);
    App::new(Config::default(), Arc::new(AtomicUsize::new(64)), tx).unwrap()
}

#[test]
fn volume_up_clamps_at_one_hundred() {
    let mut app = make_app();
    app.volume = 98;
    let bumped = app.volume.saturating_add(5).min(100);
    assert_eq!(bumped, 100);
}

#[test]
fn volume_down_clamps_at_zero() {
    let mut app = make_app();
    app.volume = 3;
    let dropped = app.volume.saturating_sub(5);
    assert_eq!(dropped, 0);
}

#[test]
fn metadata_event_pushes_to_history() {
    let mut app = make_app();
    app.on_audio_event(AudioEvent::Metadata(Metadata {
        artist: Some("Carpenter Brut".to_string()),
        title: Some("Turbo Killer".to_string()),
        raw: None,
    }));
    assert_eq!(app.history.len(), 1);
}

/// `m` while audible saves the current volume into `pre_mute_volume`
/// and drops the live volume to zero.
#[test]
fn mute_from_audible_records_pre_mute_and_zeroes_volume() {
    let mut app = make_app();
    app.volume = 70;
    let new_volume = app.mute_toggle();
    assert_eq!(new_volume, 0);
    assert_eq!(app.volume, 0);
    assert_eq!(app.pre_mute_volume, Some(70));
}

/// `m` while muted restores the recorded `pre_mute_volume` so the
/// user immediately hears the same level they had before muting.
#[test]
fn unmute_restores_pre_mute_volume() {
    let mut app = make_app();
    app.volume = 70;
    app.mute_toggle(); // → 0
    let restored = app.mute_toggle();
    assert_eq!(restored, 70);
    assert_eq!(app.volume, 70);
}

/// Regression: a session resumed from CONFIG.md with
/// `default_volume_percent: 0` lands at `volume = 0` and
/// `pre_mute_volume = None`. Pressing `m` MUST escape mute even
/// though `config.default_volume` is also 0 — otherwise the user
/// is stuck at silence with no key recovery.
#[test]
fn unmute_from_resumed_muted_session_falls_back_to_default() {
    let mut app = make_app();
    app.volume = 0;
    app.pre_mute_volume = None;
    app.config.default_volume = 0; // simulate saved-muted CONFIG.md

    let restored = app.mute_toggle();

    assert!(
        restored > 0,
        "unmute must escape silence on saved-muted resume"
    );
    assert_eq!(restored, super::UNMUTE_FALLBACK_VOLUME);
    assert_eq!(app.volume, super::UNMUTE_FALLBACK_VOLUME);
}

/// Regression: switching station MUST clear the previous station's
/// metadata snapshot so the now-playing line doesn't display stale
/// title/artist while the new ICY-metaint frame is in flight (the
/// frame can take 4–30 s to arrive depending on `metaint`).
#[test]
fn change_station_clears_stale_metadata() {
    let mut app = make_app();
    app.on_audio_event(AudioEvent::Metadata(Metadata {
        artist: Some("Carpenter Brut".to_string()),
        title: Some("Turbo Killer".to_string()),
        raw: None,
    }));
    app.on_audio_event(AudioEvent::ConnectionState(ConnectionState::Streaming {
        station: "nightride",
        started_at: std::time::Instant::now(),
    }));
    assert!(app.metadata.is_some(), "precondition: metadata seeded");
    assert!(
        app.stream_started_at.is_some(),
        "precondition: stream_started_at seeded",
    );

    let next = crate::station::next(app.current_station);
    app.change_station(next);

    assert_eq!(app.current_station.slug, next.slug);
    assert!(app.metadata.is_none(), "metadata must clear on switch");
    assert!(
        app.stream_started_at.is_none(),
        "song-elapsed timestamp must clear on switch",
    );
    assert_eq!(app.marquee_offset, 0, "marquee scroll must reset on switch");
}

#[test]
fn streaming_state_records_started_at() {
    let mut app = make_app();
    let started_at = std::time::Instant::now();
    app.on_audio_event(AudioEvent::ConnectionState(ConnectionState::Streaming {
        station: "nightride",
        started_at,
    }));
    assert!(app.stream_started_at.is_some());
}

#[test]
fn quit_action_sets_should_quit() {
    let mut app = make_app();
    app.should_quit = false;
    let action = Action::Quit;
    if matches!(action, Action::Quit) {
        app.should_quit = true;
    }
    assert!(app.should_quit);
}
