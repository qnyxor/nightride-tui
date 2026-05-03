#!/bin/sh
# ---
# SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
# SPDX-License-Identifier: Apache-2.0
# SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
# ---
#
# Appends the bundled-font license bodies to the NOTICE produced by
# `cargo about`. cargo-about scans Cargo.lock, so it never includes the
# two TTF assets we embed via build.rs (Iosevka Term Nerd Font Regular
# and Nightride FM Monospace). Their license text lives under
# LICENSES/ and is concatenated here, after the crate attribution
# table, so a single THIRD_PARTY_LICENSES.md ships with the binary
# release archive.

set -eu

OUT="${1:-THIRD_PARTY_LICENSES.md}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

cat >> "$OUT" <<EOF

## Bundled fonts

The release binary embeds two typefaces. Each ships under its own
permissive license. The crates above cover Rust dependencies; the
sections below cover the typography assets.

### Iosevka Term Nerd Font Regular

Used by: TUI render face. Source: <https://github.com/ryanoasis/nerd-fonts>.
Original: <https://github.com/be5invis/Iosevka>. Patched glyphs sourced
from the Nerd Fonts project. License: SIL Open Font License 1.1.

\`\`\`text
EOF

cat "$ROOT/LICENSES/SIL-OFL-1.1.txt" >> "$OUT"

cat >> "$OUT" <<EOF
\`\`\`

### Nightride FM Monospace

Used by: optional brand display font, installed on demand via
\`nightride-tui install-nightride-font\`. Author: **Z**, Nightride FM
operator. Source: Nightride FM upstream. License: see embedded grant
below.

\`\`\`text
EOF

cat "$ROOT/LICENSES/Nightride-FM-Font.txt" >> "$OUT"

echo '```' >> "$OUT"
