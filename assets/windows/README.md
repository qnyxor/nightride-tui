<!--
SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
SPDX-License-Identifier: Apache-2.0
-->

# nightride-tui — Windows assets

Windows-only build assets embedded into the PE binary by `build.rs` via the `embed-resource` crate (see `Cargo.toml` `[target.'cfg(target_os = "windows")'.build-dependencies]`).

## Files

- `Resource.rc` — Windows resource script. Declares `IDI_ICON1` and the `VS_VERSION_INFO` block (CompanyName=QNYXOR, FileDescription, FileVersion, LegalCopyright, OriginalFilename, ProductName, ProductVersion). `FILEVERSION` / `PRODUCTVERSION` numeric tuples MUST be hand-bumped in the same commit as `Cargo.toml` `version`.
- `nightride-tui.ico` — multi-resolution icon (16 / 32 / 48 / 64 / 128 / 256 px). Generated from `icon.png`.
- `icon.png` — 256×256 source PNG with QNYXOR EXIF metadata (Artist, Author, Copyright, Description, Source, URL, CreatorTool). Refresh path on rebrand:
  ```sh
  exiftool -overwrite_original -all= icon.png
  exiftool -overwrite_original \
      -Artist="QNYXOR" -Author="QNYXOR" \
      -Copyright="(c) 2026 QNYXOR. Apache-2.0." \
      -Description="nightride-tui application icon" \
      -ImageDescription="nightride-tui application icon" \
      -Source="https://qnyxor.nexus" \
      -URL="https://github.com/qnyxor/nightride-tui" \
      -CreatorTool="QNYXOR / nightride-tui" \
      icon.png
  magick icon.png -define icon:auto-resize=256,128,64,48,32,16 nightride-tui.ico
  ```

The ICO format does not carry standard EXIF, so per-resolution metadata lives only in the source PNG.
