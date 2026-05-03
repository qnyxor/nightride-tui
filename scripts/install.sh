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
# Utilities
# ---------------------------------------------------------------------------
die() {
    printf '[!!] %s\n' "$*" >&2
    exit 1
}

info() {
    printf '[+] %s\n' "$*"
}

ok() {
    printf '[ok] %s\n' "$*"
}

# ---------------------------------------------------------------------------
# Header
# ---------------------------------------------------------------------------
printf '\n// nightride-tui :: install\n\n'

# ---------------------------------------------------------------------------
# Platform detection
# ---------------------------------------------------------------------------
if [ -n "${NIGHTRIDE_TARGET:-}" ]; then
    TARGET="$NIGHTRIDE_TARGET"
    info "target override: $TARGET"
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

    info "target detected: $TARGET"
fi

# ---------------------------------------------------------------------------
# Resolve version
# ---------------------------------------------------------------------------
if [ "$NIGHTRIDE_VERSION" = "latest" ]; then
    printf '[+] resolving version ... '
    NIGHTRIDE_VERSION="$(curl -fsSL \
        "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep '"tag_name"' \
        | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"
    if [ -z "$NIGHTRIDE_VERSION" ]; then
        die "could not resolve latest release tag"
    fi
    printf '%s\n' "$NIGHTRIDE_VERSION"
else
    info "version pinned: $NIGHTRIDE_VERSION"
fi

# ---------------------------------------------------------------------------
# Download
# ---------------------------------------------------------------------------
ARCHIVE="${BIN_NAME}-${TARGET}.tar.gz"
CHECKSUM_FILE="${ARCHIVE}.sha256"
BASE_URL="https://github.com/${REPO}/releases/download/${NIGHTRIDE_VERSION}"

TMPDIR_WORK="$(mktemp -d)"

info "download :: $ARCHIVE"
curl -fsSL --progress-bar \
    "${BASE_URL}/${ARCHIVE}" \
    -o "${TMPDIR_WORK}/${ARCHIVE}"

info "download :: $CHECKSUM_FILE"
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

info "verify   :: sha256 ok"

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

info "install  :: ${BIN_DIR}/${BIN_NAME}"

# ---------------------------------------------------------------------------
# PATH advisory
# ---------------------------------------------------------------------------
case ":${PATH}:" in
    *":${BIN_DIR}:"*)
        # already on PATH — nothing to say
        ;;
    *)
        printf '\n[!] %s is not on your PATH.\n' "$BIN_DIR"
        printf '    Add one of the following to your shell profile:\n\n'
        printf '    bash / zsh:\n'
        printf '      export PATH="%s:$PATH"\n\n' "$BIN_DIR"
        printf '    fish:\n'
        printf '      fish_add_path %s\n\n' "$BIN_DIR"
        ;;
esac

# ---------------------------------------------------------------------------
# Verify installation
# ---------------------------------------------------------------------------
INSTALLED_VERSION="$("${BIN_DIR}/${BIN_NAME}" --version 2>&1 || true)"
ok "online :: $INSTALLED_VERSION"
printf '\n'
