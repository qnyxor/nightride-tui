// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! UI task entry point — `tokio::select!` event-loop driver.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use crossterm::event::EventStream;
use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::audio::{AudioCommand, AudioEvent};
use crate::config::Config;
use crate::error::{NightrideError, Result};
use crate::update_check;

use super::app::{App, FinalState};
use super::render::render_main;
use super::tui::Tui;

/// Frames per second for the render interval and spectrum cadence.
const FRAME_RATE: f64 = 30.0;

/// Background tick rate for non-render bookkeeping (live clock, marquee
/// scroll). 4 Hz — enough for sub-second clock updates without burning CPU.
const TICK_RATE: f64 = 4.0;

/// Entry point for the UI task.
///
/// Owns a [`Tui`] (terminal lifecycle) and an [`App`] (state). Runs the
/// canonical `tokio::select!` event loop until cancellation or quit.
/// Returns the [`FinalState`] snapshot on graceful exit so `lib::run`
/// can persist it.
///
/// # Errors
/// Returns [`NightrideError::Io`] for terminal init/teardown failures
/// and [`NightrideError::Cancelled`] when channels close.
#[allow(
    clippy::too_many_arguments,
    reason = "wiring 6 distinct longevity domains; struct grouping would obscure ownership"
)]
pub async fn run(
    config: Config,
    visualizer_width: Arc<AtomicUsize>,
    cmd_tx: mpsc::Sender<AudioCommand>,
    mut evt_rx: mpsc::Receiver<AudioEvent>,
    mut amp_rx: mpsc::Receiver<Vec<f32>>,
    save_tx: mpsc::Sender<FinalState>,
    token: CancellationToken,
) -> Result<FinalState> {
    let mut tui = Tui::enter()?;
    let mut app = App::new(config, visualizer_width, save_tx)?;
    let mut event_stream = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_secs_f64(1.0 / TICK_RATE));
    let mut render = tokio::time::interval(Duration::from_secs_f64(1.0 / FRAME_RATE));

    // Spawn background update-check. The task makes a single HTTP request to
    // the GitHub releases API and sends the result back via a oneshot channel.
    // Build a minimal one-shot reqwest client; rustls-tls is already in deps.
    let (update_tx, mut update_rx) = oneshot::channel::<Option<String>>();
    let update_client = reqwest::Client::builder()
        .user_agent(format!("nightride-tui/{}", env!("CARGO_PKG_VERSION")))
        .build();
    if let Ok(client) = update_client {
        tokio::spawn(async move {
            let result = update_check::check_for_update(&client).await;
            // Ignore send errors — the UI loop may have exited already.
            let _ = update_tx.send(result);
        });
    }
    // Disabled after the oneshot resolves once. tokio::select! would panic
    // if we polled an already-completed receiver on a subsequent iteration.
    let mut update_done = false;

    // Seed the audio supervisor with the saved volume BEFORE the first
    // Play so the post-pre-roll restore picks up the user's value
    // instead of the supervisor's hardcoded 50% startup default.
    let _ = cmd_tx.send(AudioCommand::SetVolume(app.volume)).await;
    // Initial Play of the configured default station.
    let _ = cmd_tx.send(AudioCommand::Play(app.current_station)).await;

    loop {
        tokio::select! {
            () = token.cancelled() => break,
            maybe_evt = evt_rx.recv() => {
                let Some(evt) = maybe_evt else { break };
                app.on_audio_event(evt);
            }
            maybe_amp = amp_rx.recv() => {
                let Some(amp_frame) = maybe_amp else { continue };
                app.on_amp_frame(amp_frame);
            }
            maybe_term = event_stream.next() => {
                match maybe_term {
                    Some(Ok(evt)) => {
                        if let Some(action) = app.on_terminal_event(&evt) {
                            app.dispatch(action, &cmd_tx).await?;
                        }
                    }
                    Some(Err(err)) => {
                        debug!(err = %err, "terminal event error");
                    }
                    None => break,
                }
            }
            _ = tick.tick() => {
                app.on_tick();
            }
            _ = render.tick() => {
                app.on_render_tick();
                tui.terminal.draw(|f| render_main(f, &app)).map_err(|err| NightrideError::Io {
                    op: "ui::run::draw",
                    source: err,
                })?;
            }
            // Receive the background update-check result. The oneshot
            // fires at most once; the `if !update_done` guard disables the
            // arm forever after the first resolution so tokio::select!
            // does not poll the consumed receiver again.
            result = &mut update_rx, if !update_done => {
                update_done = true;
                if let Ok(Some(tag)) = result {
                    app.update_available = Some(tag);
                }
            }
        }
        if app.should_quit {
            token.cancel();
            break;
        }
    }
    Ok(FinalState {
        station_slug: app.current_station.slug.to_string(),
        volume: app.volume,
    })
}
