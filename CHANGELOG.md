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

[Unreleased]: https://github.com/qnyxor/nightride-tui/compare/v1.0.1...HEAD
[1.0.1]: https://github.com/qnyxor/nightride-tui/releases/tag/v1.0.1
[1.0.0]: https://github.com/qnyxor/nightride-tui/releases/tag/v1.0.0
