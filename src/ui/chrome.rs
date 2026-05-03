// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! OS-level window-title chrome — OSC 0 escape writer.
//!
//! [`set_window_title`] is the single write site for the terminal
//! window-title sequence. It is called from `app.rs` whenever the
//! now-playing line changes so the terminal tab and window title stay
//! in sync with the in-app marquee.

use std::io::Write;

/// Set the host terminal's window title via OSC 0. Called whenever the
/// now-playing line changes — keeps the terminal tab + window title in
/// sync with the in-app marquee. Output format: `\x1b]0;<title>\x07`
/// (xterm-style OSC 0 with BEL terminator). Best-effort: write failures
/// are silently dropped.
pub(super) fn set_window_title(title: &str) {
    // Sanitise: strip control chars + the BEL terminator itself so the
    // sequence cannot terminate prematurely on hostile metadata.
    let cleaned: String = title
        .chars()
        .filter(|c| !c.is_control() || *c == ' ')
        .collect();
    let payload = format!("\x1b]0;{cleaned}\x07");
    // Write to stderr because that's where ratatui renders the
    // alt-screen — both flows share the same FD.
    let _ = std::io::stderr().write_all(payload.as_bytes());
    let _ = std::io::stderr().flush();
}
