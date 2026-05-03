// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Terminal lifecycle wrapper.
//!
//! [`Tui`] owns the crossterm raw-mode + alt-screen + bracketed-paste
//! state for the running app. State is encoded as a bitflags-style `u8`
//! so a single byte tracks every reversible terminal mutation, and
//! [`Drop`] guarantees restoration even on panic (zero-panic
//! invariant via the zero-unwrap discipline in lib code).

use std::io::{self, Stderr};

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    is_raw_mode_enabled,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::error::{NightrideError, Result};

const TUI_RAW: u8 = 0b0001;
const TUI_ALT: u8 = 0b0010;
const TUI_PASTE: u8 = 0b0100;

/// Terminal lifecycle wrapper. Implements [`Drop`] so panics still restore
/// the terminal.
///
/// `state` is a bitflags-style `u8` so a single byte tracks every
/// reversible terminal mutation (raw mode, alt-screen, bracketed paste,
/// mouse capture). Bits stay set after `enter` and clear individually as
/// `restore` walks them. Avoids the "many bools" pattern.
pub struct Tui {
    pub(super) terminal: Terminal<CrosstermBackend<Stderr>>,
    state: u8,
}

impl Tui {
    /// Initialise the terminal: raw mode, alt-screen, bracketed paste.
    ///
    /// Mouse capture is intentionally NOT enabled so the host terminal
    /// keeps native click-drag text selection (copy song titles, log
    /// snippets). Trade-off: the scroll wheel does not map to volume —
    /// `+`, `-`, and `m` remain the canonical volume controls.
    ///
    /// # Errors
    /// Returns [`NightrideError::Io`] if any of the terminal mode
    /// transitions or backend initialisation fails.
    pub fn enter() -> Result<Self> {
        enable_raw_mode().map_err(|err| NightrideError::Io {
            op: "tui::enter::raw",
            source: err,
        })?;
        execute!(io::stderr(), EnterAlternateScreen, EnableBracketedPaste).map_err(|err| {
            NightrideError::Io {
                op: "tui::enter::execute",
                source: err,
            }
        })?;
        let backend = CrosstermBackend::new(io::stderr());
        let terminal = Terminal::new(backend).map_err(|err| NightrideError::Io {
            op: "tui::enter::terminal",
            source: err,
        })?;
        Ok(Self {
            terminal,
            state: TUI_RAW | TUI_ALT | TUI_PASTE,
        })
    }

    /// Restore the terminal. Idempotent — `Drop` calls this too.
    fn restore(&mut self) {
        if self.state & TUI_PASTE != 0 {
            let _ = execute!(io::stderr(), DisableBracketedPaste);
            self.state &= !TUI_PASTE;
        }
        if self.state & TUI_ALT != 0 {
            let _ = execute!(io::stderr(), LeaveAlternateScreen);
            self.state &= !TUI_ALT;
        }
        if self.state & TUI_RAW != 0 {
            if let Ok(true) = is_raw_mode_enabled() {
                let _ = disable_raw_mode();
            }
            self.state &= !TUI_RAW;
        }
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        self.restore();
    }
}
