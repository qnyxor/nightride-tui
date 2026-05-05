#!/bin/sh
# ---
# SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
# SPDX-License-Identifier: Apache-2.0
# SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
# ---
#
# nightride-tui installer — POSIX sh, curl | sh safe.
# Works in dash, ash, and busybox sh.
#
# Environment overrides:
#   NIGHTRIDE_TARGET   — target triple (auto-detected by default)
#   NIGHTRIDE_VERSION  — release tag, e.g. v1.0.0 (default: latest)
#   NIGHTRIDE_PREFIX   — install prefix (default: $HOME/.local)

set -eu

REPO="qnyxor/nightride-tui"
BIN_NAME="nightride-tui"
NIGHTRIDE_VERSION="${NIGHTRIDE_VERSION:-latest}"
NIGHTRIDE_PREFIX="${NIGHTRIDE_PREFIX:-${HOME}/.local}"

# ---------------------------------------------------------------------------
# Cleanup
# ---------------------------------------------------------------------------
TMPDIR_WORK=""
cleanup() {
    if [ -n "$TMPDIR_WORK" ] && [ -d "$TMPDIR_WORK" ]; then
        rm -rf "$TMPDIR_WORK"
    fi
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# Colours — only when stdout is a TTY. Pipes, logs, CI runners stay plain.
# NO_COLOR (https://no-color.org) opts out unconditionally.
#
# Style: brackets and surrounding text default; only the marker symbol
# inside the brackets is coloured. Header in Nightride pink (#FA275D)
# via truecolor, with a bright-magenta fallback for terminals that do
# not advertise 24-bit support.
# ---------------------------------------------------------------------------
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    C_INFO=$(printf '\033[1;36m')   # bright cyan   →  +
    C_OK=$(printf '\033[1;32m')     # bright green  →  ok
    C_WARN=$(printf '\033[1;33m')   # bright yellow →  !
    C_ERR=$(printf '\033[1;31m')    # bright red    →  !!
    case "${COLORTERM:-}" in
        truecolor|24bit)
            C_HEADER=$(printf '\033[1;38;2;250;39;93m')   # #FA275D
            ;;
        *)
            C_HEADER=$(printf '\033[1;35m')               # fallback
            ;;
    esac
    C_DIM=$(printf '\033[2m')
    C_RESET=$(printf '\033[0m')
else
    C_INFO=""
    C_OK=""
    C_WARN=""
    C_ERR=""
    C_HEADER=""
    C_DIM=""
    C_RESET=""
fi

# ---------------------------------------------------------------------------
# Utilities — bracket plain, only the marker symbol carries colour.
#
# Two flavours of info/ok rows:
#   * info / ok        — free-form messages (no alignment).
#   * row_info / row_ok — block-aligned `LABEL :: VALUE` rows. Padding picks
#                         a label width of 8, with `[ok]` carrying one
#                         char extra (4-char marker) so the `::` column
#                         stays on the same vertical line as `[+]` rows.
# ---------------------------------------------------------------------------
die() {
    printf '[%s!!%s] %s\n' "$C_ERR" "$C_RESET" "$*" >&2
    exit 1
}

info() {
    printf '[%s+%s] %s\n' "$C_INFO" "$C_RESET" "$*"
}

ok() {
    printf '[%sok%s] %s\n' "$C_OK" "$C_RESET" "$*"
}

warn() {
    printf '[%s!%s] %s\n' "$C_WARN" "$C_RESET" "$*"
}

row_info() {
    _label=$1
    shift
    printf '[%s+%s] %-8s :: %s\n' "$C_INFO" "$C_RESET" "$_label" "$*"
}

row_ok() {
    _label=$1
    shift
    printf '[%sok%s] %-7s :: %s\n' "$C_OK" "$C_RESET" "$_label" "$*"
}

# ---------------------------------------------------------------------------
# Header
# ---------------------------------------------------------------------------
printf '\n%s// nightride-tui :: install%s\n\n' "$C_HEADER" "$C_RESET"

# ---------------------------------------------------------------------------
# Platform detection
# ---------------------------------------------------------------------------
if [ -n "${NIGHTRIDE_TARGET:-}" ]; then
    TARGET="$NIGHTRIDE_TARGET"
    row_info "target" "$TARGET (override)"
else
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Darwin)
            case "$ARCH" in
                arm64)   TARGET="aarch64-apple-darwin" ;;
                x86_64)  TARGET="x86_64-apple-darwin" ;;
                *)       die "unsupported macOS arch: $ARCH" ;;
            esac
            ;;
        Linux)
            case "$ARCH" in
                x86_64)          TARGET="x86_64-unknown-linux-gnu" ;;
                aarch64|arm64)   TARGET="aarch64-unknown-linux-gnu" ;;
                *)               die "unsupported Linux arch: $ARCH" ;;
            esac
            ;;
        *)
            die "unsupported OS: $OS (only macOS and Linux are supported)" ;;
    esac

    row_info "target" "$TARGET"
fi

# ---------------------------------------------------------------------------
# Resolve version
# ---------------------------------------------------------------------------
if [ "$NIGHTRIDE_VERSION" = "latest" ]; then
    printf '[%s+%s] %-8s :: ' "$C_INFO" "$C_RESET" "version"
    NIGHTRIDE_VERSION="$(curl -fsSL \
        "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep '"tag_name"' \
        | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"
    if [ -z "$NIGHTRIDE_VERSION" ]; then
        printf '\n'
        die "could not resolve latest release tag"
    fi
    printf '%s\n' "$NIGHTRIDE_VERSION"
else
    row_info "version" "$NIGHTRIDE_VERSION (pinned)"
fi

# ---------------------------------------------------------------------------
# Download
# ---------------------------------------------------------------------------
ARCHIVE="${BIN_NAME}-${TARGET}.tar.gz"
CHECKSUM_FILE="${ARCHIVE}.sha256"
BASE_URL="https://github.com/${REPO}/releases/download/${NIGHTRIDE_VERSION}"

TMPDIR_WORK="$(mktemp -d)"

row_info "download" "$ARCHIVE"
curl -fsSL --progress-bar \
    "${BASE_URL}/${ARCHIVE}" \
    -o "${TMPDIR_WORK}/${ARCHIVE}"

row_info "download" "$CHECKSUM_FILE"
curl -fsSL \
    "${BASE_URL}/${CHECKSUM_FILE}" \
    -o "${TMPDIR_WORK}/${CHECKSUM_FILE}"

# ---------------------------------------------------------------------------
# Verify SHA-256
# ---------------------------------------------------------------------------
if command -v shasum >/dev/null 2>&1; then
    SHA_CMD="shasum -a 256"
elif command -v sha256sum >/dev/null 2>&1; then
    SHA_CMD="sha256sum"
else
    die "neither shasum nor sha256sum found — cannot verify checksum"
fi

EXPECTED_HASH="$(awk '{print $1}' "${TMPDIR_WORK}/${CHECKSUM_FILE}")"
ACTUAL_HASH="$($SHA_CMD "${TMPDIR_WORK}/${ARCHIVE}" | awk '{print $1}')"

if [ "$EXPECTED_HASH" != "$ACTUAL_HASH" ]; then
    die "sha256 mismatch\n  expected: $EXPECTED_HASH\n  got:      $ACTUAL_HASH"
fi

row_info "verify" "sha256 ok"

# ---------------------------------------------------------------------------
# Extract and install
# ---------------------------------------------------------------------------
tar -xzf "${TMPDIR_WORK}/${ARCHIVE}" -C "$TMPDIR_WORK"

BIN_DIR="${NIGHTRIDE_PREFIX}/bin"
mkdir -p "$BIN_DIR"

mv "${TMPDIR_WORK}/${BIN_NAME}" "${BIN_DIR}/${BIN_NAME}"
chmod 755 "${BIN_DIR}/${BIN_NAME}"

# macOS 14+ requires a fresh ad-hoc signature whenever the binary is
# moved across volumes, has its quarantine attribute mutated, or otherwise
# triggers the kernel's provenance check. Re-sign in place; harmless on
# already-signed binaries and a no-op on non-Darwin systems.
if [ "$(uname -s)" = "Darwin" ] && command -v codesign >/dev/null 2>&1; then
    codesign --force -s - "${BIN_DIR}/${BIN_NAME}" >/dev/null 2>&1 || true
fi

row_info "install" "${BIN_DIR}/${BIN_NAME}"

# ---------------------------------------------------------------------------
# PATH advisory
# ---------------------------------------------------------------------------
case ":${PATH}:" in
    *":${BIN_DIR}:"*)
        # already on PATH — nothing to say
        ;;
    *)
        printf '\n'
        warn "${BIN_DIR} is not on your PATH."
        printf '    Add one of the following to your shell profile:\n\n'
        printf '    bash / zsh:\n'
        printf '      export PATH="%s:$PATH"\n\n' "$BIN_DIR"
        printf '    fish:\n'
        printf '      fish_add_path %s\n\n' "$BIN_DIR"
        ;;
esac

# ---------------------------------------------------------------------------
# Shadow detection — another nightride-tui earlier in $PATH would silently
# beat the new install. Resolve via `command -v` (POSIX, fresh PATH walk in
# a subshell) and compare with the installed path.
# ---------------------------------------------------------------------------
INSTALLED_VERSION="$("${BIN_DIR}/${BIN_NAME}" --version 2>&1 | awk '/./{print; exit}' || true)"
RESOLVED_PATH="$(command -v "$BIN_NAME" 2>/dev/null || true)"

if [ -n "$RESOLVED_PATH" ] && [ "$RESOLVED_PATH" != "${BIN_DIR}/${BIN_NAME}" ]; then
    SHADOW_VERSION="$("$RESOLVED_PATH" --version 2>&1 | awk '/./{print; exit}' || true)"
    printf '\n'
    warn "another ${BIN_NAME} shadows the new install:"
    printf '    installed: %s  (%s)\n' "${BIN_DIR}/${BIN_NAME}" "$INSTALLED_VERSION"
    printf '    resolved:  %s  (%s)\n' "$RESOLVED_PATH" "$SHADOW_VERSION"
    printf '    fix: remove the shadow, or put %s earlier in $PATH:\n' "$BIN_DIR"
    printf '         rm %s   (or `cargo uninstall %s` if managed by cargo)\n\n' \
           "$RESOLVED_PATH" "$BIN_NAME"
fi

# ---------------------------------------------------------------------------
# Online — report the version that the user's shell will actually run.
# If a shadow is in play the line above already flagged it; here we still
# report what `nightride-tui` resolves to today so the user is not lied to.
# ---------------------------------------------------------------------------
if [ -n "$RESOLVED_PATH" ]; then
    RESOLVED_VERSION="$("$RESOLVED_PATH" --version 2>&1 | awk '/./{print; exit}' || true)"
    row_ok "online" "$RESOLVED_VERSION"
else
    row_ok "online" "$INSTALLED_VERSION  (run via $BIN_DIR/$BIN_NAME)"
fi

# Shell hint, in header style — `hash -r` cannot be auto-applied because
# it is a builtin of the parent shell, not the subshell that runs this
# script. Worth a line so the user knows what to do.
printf '\n%s// shell-hint :: run `hash -r` (bash/zsh) to refresh binary, or open new terminal.%s\n\n' \
       "$C_HEADER" "$C_RESET"
