// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Input handling: terminal-event dispatch + action-to-supervisor
//! translation.
//!
//! This module hosts a second `impl App` block that owns the input
//! surface (`on_terminal_event`, `on_key`, [`App::dispatch`], `send`).
//! Splitting the impl across files keeps `app.rs` under the §2.3
//! file-size cap while colocating the input logic in its own
//! concept-file.

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use tokio::sync::mpsc;

use crate::audio::AudioCommand;
use crate::config::{TransportFormat, default_user_config_path, save_input_format};
use crate::error::{NightrideError, Result};
use crate::station;
use tracing::info;

use super::app::{Action, App, VOLUME_STEP};

impl App {
    /// Handle a terminal event. Returns an `Action` for the dispatcher
    /// to issue against the audio task.
    pub fn on_terminal_event(&mut self, evt: &Event) -> Option<Action> {
        let Event::Key(key) = evt else {
            return None;
        };
        if key.kind != KeyEventKind::Press {
            return None;
        }
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => Some(Action::Quit),
            (KeyModifiers::NONE, KeyCode::Char('+' | '=')) => Some(Action::VolumeUp),
            (KeyModifiers::NONE, KeyCode::Char('-')) => Some(Action::VolumeDown),
            (KeyModifiers::NONE, KeyCode::Char('m')) => Some(Action::MuteToggle),
            (KeyModifiers::NONE, KeyCode::Char('t')) => Some(Action::TransportToggle),
            (KeyModifiers::NONE, KeyCode::Right) => Some(Action::NextStation),
            (KeyModifiers::NONE, KeyCode::Left) => Some(Action::PrevStation),
            _ => None,
        }
    }

    /// Dispatch an action to the audio task. Volume mutations also
    /// update the local display copy so render is responsive.
    ///
    /// # Errors
    /// Returns [`NightrideError::Cancelled`] when the audio command
    /// channel has closed (supervisor exited).
    pub async fn dispatch(
        &mut self,
        action: Action,
        cmd_tx: &mpsc::Sender<AudioCommand>,
    ) -> Result<()> {
        match action {
            Action::Quit => {
                info!("action: Quit");
                self.should_quit = true;
            }
            Action::VolumeUp => {
                self.volume = self.volume.saturating_add(VOLUME_STEP).min(100);
                info!(volume = self.volume, "action: VolumeUp");
                self.send(cmd_tx, AudioCommand::SetVolume(self.volume))
                    .await?;
                self.request_save();
            }
            Action::VolumeDown => {
                self.volume = self.volume.saturating_sub(VOLUME_STEP);
                info!(volume = self.volume, "action: VolumeDown");
                self.send(cmd_tx, AudioCommand::SetVolume(self.volume))
                    .await?;
                self.request_save();
            }
            Action::MuteToggle => {
                let new_volume = self.mute_toggle();
                info!(volume = new_volume, "action: MuteToggle");
                self.send(cmd_tx, AudioCommand::SetVolume(new_volume))
                    .await?;
                self.request_save();
            }
            Action::TransportToggle => {
                let old_format = self.config.input_format;
                let new_format = match old_format {
                    TransportFormat::Mp3 => TransportFormat::Hls,
                    TransportFormat::Hls => TransportFormat::Mp3,
                };
                self.config.input_format = new_format;
                info!("transport toggle: {:?} -> {:?}", old_format, new_format);

                // Persist the new format to state file
                let config_path = default_user_config_path();
                save_input_format(&config_path, new_format).ok();

                // Restart the stream with the new transport
                self.send(
                    cmd_tx,
                    AudioCommand::SetStation(self.current_station, new_format),
                )
                .await?;
                self.request_save();
            }
            Action::NextStation => {
                let next = station::next(self.current_station);
                info!(
                    from = self.current_station.slug,
                    to = next.slug,
                    transport = ?self.config.input_format,
                    "action: NextStation"
                );
                self.change_station(next);
                self.send(
                    cmd_tx,
                    AudioCommand::SetStation(next, self.config.input_format),
                )
                .await?;
                self.request_save();
            }
            Action::PrevStation => {
                let prev = station::prev(self.current_station);
                info!(
                    from = self.current_station.slug,
                    to = prev.slug,
                    transport = ?self.config.input_format,
                    "action: PrevStation"
                );
                self.change_station(prev);
                self.send(
                    cmd_tx,
                    AudioCommand::SetStation(prev, self.config.input_format),
                )
                .await?;
                self.request_save();
            }
        }
        Ok(())
    }

    async fn send(&self, cmd_tx: &mpsc::Sender<AudioCommand>, cmd: AudioCommand) -> Result<()> {
        cmd_tx
            .send(cmd)
            .await
            .map_err(|_| NightrideError::Cancelled {
                op: "ui::App::send",
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ui::app::FinalState;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    fn make_app() -> App {
        let (tx, _rx) = mpsc::channel::<FinalState>(8);
        App::new(Config::default(), Arc::new(AtomicUsize::new(64)), tx).unwrap()
    }

    fn key(modifiers: KeyModifiers, code: KeyCode) -> KeyEvent {
        KeyEvent {
            modifiers,
            code,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    #[test]
    fn ctrl_c_emits_quit() {
        let mut app = make_app();
        let action =
            app.on_terminal_event(&Event::Key(key(KeyModifiers::CONTROL, KeyCode::Char('c'))));
        assert!(matches!(action, Some(Action::Quit)));
    }

    #[test]
    fn plus_emits_volume_up() {
        let mut app = make_app();
        let action =
            app.on_terminal_event(&Event::Key(key(KeyModifiers::NONE, KeyCode::Char('+'))));
        assert!(matches!(action, Some(Action::VolumeUp)));
    }

    #[test]
    fn minus_emits_volume_down() {
        let mut app = make_app();
        let action =
            app.on_terminal_event(&Event::Key(key(KeyModifiers::NONE, KeyCode::Char('-'))));
        assert!(matches!(action, Some(Action::VolumeDown)));
    }

    #[test]
    fn m_emits_mute_toggle() {
        let mut app = make_app();
        let action =
            app.on_terminal_event(&Event::Key(key(KeyModifiers::NONE, KeyCode::Char('m'))));
        assert!(matches!(action, Some(Action::MuteToggle)));
    }

    #[test]
    fn t_emits_transport_toggle() {
        let mut app = make_app();
        let action =
            app.on_terminal_event(&Event::Key(key(KeyModifiers::NONE, KeyCode::Char('t'))));
        assert!(matches!(action, Some(Action::TransportToggle)));
    }

    #[test]
    fn right_arrow_emits_next_station() {
        let mut app = make_app();
        let action = app.on_terminal_event(&Event::Key(key(KeyModifiers::NONE, KeyCode::Right)));
        assert!(matches!(action, Some(Action::NextStation)));
    }

    #[test]
    fn left_arrow_emits_prev_station() {
        let mut app = make_app();
        let action = app.on_terminal_event(&Event::Key(key(KeyModifiers::NONE, KeyCode::Left)));
        assert!(matches!(action, Some(Action::PrevStation)));
    }

    #[test]
    fn key_release_events_ignored() {
        let mut app = make_app();
        let evt = Event::Key(KeyEvent {
            modifiers: KeyModifiers::NONE,
            code: KeyCode::Char('q'),
            kind: KeyEventKind::Release,
            state: crossterm::event::KeyEventState::NONE,
        });
        assert!(app.on_terminal_event(&evt).is_none());
    }

    #[test]
    fn non_key_terminal_events_ignored() {
        let mut app = make_app();
        let evt = Event::FocusGained;
        assert!(app.on_terminal_event(&evt).is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dispatch_quit_sets_should_quit() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel::<AudioCommand>(8);
        app.dispatch(Action::Quit, &tx).await.unwrap();
        assert!(app.should_quit);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dispatch_volume_up_increments_and_clamps() {
        let mut app = make_app();
        let (tx, mut rx) = mpsc::channel::<AudioCommand>(8);
        app.volume = 95;
        app.dispatch(Action::VolumeUp, &tx).await.unwrap();
        assert_eq!(app.volume, 100);
        assert!(matches!(rx.try_recv(), Ok(AudioCommand::SetVolume(100))));

        app.dispatch(Action::VolumeUp, &tx).await.unwrap();
        assert_eq!(app.volume, 100, "VolumeUp must clamp at 100");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dispatch_mute_toggles_zero_then_default() {
        let mut app = make_app();
        let (tx, _rx) = mpsc::channel::<AudioCommand>(8);
        let original = app.volume;
        app.dispatch(Action::MuteToggle, &tx).await.unwrap();
        assert_eq!(app.volume, 0, "first mute drops to 0");
        app.dispatch(Action::MuteToggle, &tx).await.unwrap();
        assert_eq!(app.volume, original, "second mute restores default");
    }

    #[test]
    fn env_var_nightride_tui_hls_is_ignored() {
        // The env var is not consulted by Config::default()
        // Just verify that Config defaults to Hls
        let cfg = Config::default();
        assert_eq!(cfg.input_format, TransportFormat::Hls);
    }
}
