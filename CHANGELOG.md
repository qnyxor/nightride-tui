<!--
SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
SPDX-License-Identifier: Apache-2.0
SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
-->

# Changelog

All notable changes to nightride-tui are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [1.1.0] — 2026-05-07

### Added

- **HLS Live transport.** AAC fMP4 streaming via `symphonia` +
  `m3u8-rs`, hardcoded to the `aac_hifi` variant on Nightride's `:8443`
  edge. Selectable per session; persisted to the platform state file
  (`audio.input_format`).
- **`t` keybind — runtime transport toggle.** Switch MP3 ↔ HLS without
  restarting; the new selection writes through to the state file and
  survives the next launch.
- **SSE metadata channel.** `nightride.fm/meta` server-sent events
  drive now-playing. `HLS_AUDIO_LAG = 15s` deferred-emit smooths the
  gap between server-side track changes and audible audio; the first
  emit per SSE connection bypasses the delay so the line populates
  immediately. Cancellation propagates through a child token, so
  station switches drop in-flight emits cleanly.
- **Skip-to-live-edge preroll.** `PREROLL_LIVE_EDGE_OFFSET = 3` —
  attach starts at `segments[len - 3]`, discarding ~35 s of stale
  backlog on every fresh connection. Closes the "30-second-ago audio"
  surprise after a station switch.
- **Per-station init-segment cache + background TLS prewarm.** Cache
  populated on first attach; subsequent attaches of the same station
  skip the init fetch. TLS handshake to `:8443` warmed at startup.
  Combined: ~350 ms saved on first attach of the session.
- **Tuning indicator.** Braille spinner anchored to the spectrum row
  while the stream is locking on; animated `tuning .` / `tuning ..` /
  `tuning ...` ping-pong on the now-playing line. Both surfaces share
  the new `is_tuning()` predicate; `TUNING_GRACE_SECS = 5` caps the
  state so ICY-less or laggy streams cannot trap the UI in tuning mode.

### Changed

- **Logging hygiene.** Every keypress action now surfaces at `info`
  (audit trail of operator intent). Third-party crate noise from
  `symphonia`, `reqwest`, `hyper`, `h2`, `rustls`, and
  `reqwest-eventsource` is filtered down to `warn` so the journal
  reflects nightride-tui's own behaviour rather than the network stack
  underneath.
- **Release matrix expanded.** Adds Linux aarch64 (gnu + musl) lanes
  alongside the existing macOS aarch64 / x86_64 + Linux x86_64.
  End-to-end self-update flow validated on the wire (`nightride-tui
  update` 1.0.4 → 1.1.0).

## [1.0.4] — 2026-05-05

### Changed

- Install script (`scripts/install.sh`, served via the Cloudflare Worker)
  gains a Nightride-pink (`#FA275D`) header, TTY-gated ANSI markers
  (bracket plain, only the symbol carries colour), `LABEL :: VALUE`
  rows aligned vertically across the whole flow, and a closing
  `// shell-hint ::` row reminding the user to run `hash -r` (or open
  a new shell). Honours `NO_COLOR`; falls back to plain text on pipes,
  CI runners, and non-truecolor terminals.
- Install script now detects shadow `nightride-tui` binaries earlier in
  `$PATH` (typical case: a stale `cargo install` in `~/.cargo/bin`)
  and prints a `[!]` warn block with both versions side-by-side and a
  concrete fix (`rm` or `cargo uninstall`). The closing `online ::` row
  reports the version of the resolved binary — what the shell will
  actually run — not the freshly copied path. Closes the surprise where
  the install boasted `1.0.3` while `nightride-tui -v` returned an old
  shadow.
- `nightride-tui update` now hits the GitHub `releases/latest` API
  *before* downloading, compares the resolved tag with
  `CARGO_PKG_VERSION`, and short-circuits with
  `[+] check    :: nightride-tui X.Y.Z — already on latest, nothing to
  do.` when the binary is already current. On an actual update the row
  becomes `… X.Y.Z → A.B.C` and the script is invoked with
  `NIGHTRIDE_VERSION` pre-pinned, skipping the script's own API call.
  Saves two HTTP fetches and an unnecessary atomic rename on the common
  no-op path. Tag parsing is a zero-dependency manual substring walk
  (no `serde_json` added); covered by unit tests.
- `nightride-tui update` ends with a compact `[ok] update  :: complete`
  row plus a single trailing `// shell-hint ::` (Nightride pink). The
  install script suppresses its own shell-hint when invoked by the
  binary (via `NIGHTRIDE_INVOKED_BY_UPDATE=1`), so the closing line
  appears exactly once at the very end of the flow.

## [1.0.3] — 2026-05-05

### Changed

- Canonical install URL switched from
  `raw.githubusercontent.com/qnyxor/nightride-tui/main/scripts/install.sh`
  to `sh.nightride-tui.qnyxor.nexus` (Cloudflare Worker Custom Domain —
  subdomain pattern, no Routes failure-mode, source URL opaque to caller).
  README curl-pipe instruction and `INSTALL_URL` const (consumed by
  `nightride-tui update`) updated in lockstep. Behaviour is identical:
  same script, same SHA pinning, same release pull.
- `nightride-tui update` success message now hints `hash -r` (bash/zsh) so
  the user can refresh the shell's binary-lookup cache without opening a
  new terminal. Avoids the surprise of running an old inode after a
  successful in-place replace.

## [1.0.2] — 2026-05-04

### Changed

- `install-tui-font` now downloads Iosevka Term Nerd Font Regular from
  the official upstream repo (`ryanoasis/nerd-fonts` at tag `v3.4.0`)
  on first invocation. Three pre-write integrity gates fire before any
  byte reaches disk: HTTP status, SFNT magic bytes (`00 01 00 00`), and
  SHA-256 against the pin (`d5116846…7880`, identical to the v1.0.1
  embedded pin). Atomic rename via `tempfile::NamedTempFile::persist`.
  Binary slim: ~18 MB → ~5.4 MB stripped.

### Added

- User-Agent attribution `nightride-tui/{version} (+repo URL)` on every
  HTTP request the binary makes (Iosevka download, update check,
  Icecast stream). RFC 9110 compliant. Identifies the calling app to
  upstream operators in their server logs.
- `tempfile ~3.27.0` dependency for atomic write semantics.
- `indicatif ~0.18.4` dependency for download progress bar.

### Removed

- Embedded `assets/IosevkaTermNerdFont-Regular.ttf` (~13 MB) from the
  source tree. Its companion license blob (884 B) remains embedded and
  is written next to the downloaded TTF on install per OFL 1.1 §2.
- Redundant per-request `User-Agent` header in `update_check.rs` — UA
  now flows uniformly from the `Client::builder().user_agent(...)` site.
- `IOSEVKA_PATH` and `IOSEVKA_SHA256_PIN` constants and the corresponding
  `verify_asset` invocation in `build.rs`. Nightride FM Mono build-time
  verification preserved bit-for-bit.

### Hardened

- All `reqwest::Client` builders in the binary now share a single
  `crate::USER_AGENT` const (compile-time `concat!`). No drift, no
  runtime allocation.
- Three independent integrity gates (transport, format, hash) defend
  the install path. Lessons from the v1.0.1 asset corruption incident
  applied to the runtime fetch surface.

## [1.0.1] — 2026-05-04

### Fixed

- **Critical asset corruption.** The `IosevkaTermNerdFont-Regular.ttf`
  shipped with v1.0.0 was a GitHub 404 HTML page (~298 KB) instead of the
  real TrueType font. Root cause: the upstream Nerd Fonts URL in
  `Makefile`'s `fetch-iosevka` target carried a stray `Regular/` path
  segment that no longer exists; `curl -L` saved the 404 page, and the
  build-time SHA-256 pin was computed against that HTML, so the integrity
  check passed for poisoned input. Users who ran
  `nightride-tui install-tui-font` ended up with the HTML written into
  their system font directory (inert — the OS just ignored it — but the
  recommended TUI face was never actually installed). Fixed: real
  TrueType (~13 MB) bundled, SHA-256 repinned to
  `d5116846a175ef4a988f61241dd3572d6a9dd3e09d4d168c67954b10783a7880`.
  Run `nightride-tui install-tui-font` again to overwrite the poisoned
  copy locally.

### Added

- `assets/IosevkaTermNerdFont-Regular.LICENSE.txt` — SIL OFL 1.1
  attribution + Reserved Font Name notice for Iosevka, paired with the
  `.ttf`.
- `install-tui-font` and `install-nightride-font` now write the
  companion `.LICENSE.txt` next to the `.ttf` in the user's font
  directory, satisfying OFL 1.1 §2 and the Nightride FM grant on the
  redistributed copy (not just inside the source tree).

### Hardened

- `build.rs` now verifies SFNT magic bytes (`00 01 00 00`, `OTTO`,
  `true`, `typ1`) before the SHA-256 pin check. A SHA pinned against an
  HTML page will no longer compile silently — the magic-byte gate fires
  first with an explicit "not a valid font file" error.
- `Makefile`'s `fetch-iosevka` target now points at the correct upstream
  path and aborts with a clear error if the downloaded blob is not a
  TrueType file.
- New roster invariant: every `InstallableFont` must declare a paired
  license blob + filename. Surfaces missing redistribution paperwork at
  test time rather than ship time.

### Removed

- Unused ASCII banner files (`assets/banner-3.txt`,
  `assets/banner-ansi-shadow.txt`, `assets/banner-bloody.txt`). No
  callsite referenced them.

## [1.0.0] — 2026-05-03

### Added

- Initial public release.
- Terminal radio player for the Nightride.fm station registry.
- Reactive Braille spectrum visualizer.
- Per-station accent palette + dynamic theme.
- ICY-MetaInt now-playing parser with bounded history ring.
- Bundled Iosevka Term Nerd Font and Nightride FM Monospace install commands.
- Curl-pipe installer for macOS arm64 / x86_64 and Linux x86_64 / aarch64.

[Unreleased]: https://github.com/qnyxor/nightride-tui/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/qnyxor/nightride-tui/compare/v1.0.4...v1.1.0
[1.0.4]: https://github.com/qnyxor/nightride-tui/compare/v1.0.3...v1.0.4
[1.0.3]: https://github.com/qnyxor/nightride-tui/compare/v1.0.2...v1.0.3
[1.0.2]: https://github.com/qnyxor/nightride-tui/compare/v1.0.1...v1.0.2
[1.0.1]: https://github.com/qnyxor/nightride-tui/releases/tag/v1.0.1
[1.0.0]: https://github.com/qnyxor/nightride-tui/releases/tag/v1.0.0
