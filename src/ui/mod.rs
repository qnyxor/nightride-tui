// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! TUI surface — `Tui` lifecycle + `App` state + canonical async event loop.
//!
//! Implements the canonical ratatui async event loop pattern: a
//! `tokio::select!` multiplex over the crossterm `EventStream`, audio
//! events, visualizer frames, tick interval, render interval, and the
//! cancellation token.
//! No widget tree: every render function is named, inline, and free.
//!
//! Layout:
//!
//! ```text
//!   [ DARKSYNTH // artist / title ]            ← → stations | mute | + - volume
//!   50 % ||||||||||  Dwallclock  uptime  song-elapsed
//!   ········ (Braille spectrum) ········
//! ```
//!
//! ## Module map
//!
//! - `chrome` — OS-level window-title (OSC 0).
//! - `tui` — terminal lifecycle (raw mode, alt-screen, bracketed paste).
//! - [`app`] — `App` state machine + `Action` + `FinalState` + input handling.
//! - `run` — `tokio::select!` event-loop driver.
//! - [`render`] — render orchestrator + per-surface render submodules.

pub mod app;
pub(super) mod chrome;
mod input;
pub mod render;
mod run;
mod tui;

pub use app::{Action, App, FinalState};
#[cfg(any(test, feature = "test-export"))]
pub use render::render_main_test_export;
#[cfg(any(test, feature = "test-export"))]
pub use render::{format_d_timestamp, format_d_timestamp_red_fragment, format_elapsed_hms};
pub use run::run;
pub use tui::Tui;
