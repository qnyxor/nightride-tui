<!--
SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
SPDX-License-Identifier: Apache-2.0
-->

# nightride-tui — Windows assets

This directory holds the Windows-only build assets: the icon (`nightride-tui.ico`) and any future resource files for the PE VERSIONINFO block.

## Pending

- `nightride-tui.ico` — multi-resolution icon (16, 32, 48, 64, 128, 256). Awaiting source PNG (>= 256x256, 512x512 ideal) from QNYXOR. Conversion: `convert logo.png -define icon:auto-resize=256,128,64,48,32,16 nightride-tui.ico` (ImageMagick).

Once `nightride-tui.ico` is in place, `build.rs` will wire it into the Windows binary via `embed-resource` (see `Cargo.toml` `[target.'cfg(target_os = "windows")'.build-dependencies]`).
