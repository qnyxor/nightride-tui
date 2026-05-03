#!/usr/bin/env bash
# ---
# SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
# SPDX-License-Identifier: Apache-2.0
# SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
# ---
#
# commit-msg hook: enforce the dual Co-Authored-By trailer.
#
# Per QNYXOR project canon.
# every NightRideTUI commit must carry BOTH:
#   Co-Authored-By: QNYXOR <qnyxor@pm.me>
#   Co-Authored-By: NyX <nyx@qnyxor.nexus>
#
# Install with `make install-hooks` or symlink manually:
#   ln -sf ../../scripts/check-trailers.sh .git/hooks/commit-msg
#
# Bypass during emergencies with `git commit --no-verify`.

set -euo pipefail

# Argument 1: path to the commit message file (git passes this).
MSG_FILE="${1:?usage: check-trailers.sh COMMIT_MSG_FILE}"

# Skip the check on merge commits + amends-via-rebase squash messages.
# Heuristic: if the message file contains a `Merge ...` line at the top,
# trust the trailers from the merged commits.
FIRST_LINE="$(head -n1 "$MSG_FILE" || true)"
case "$FIRST_LINE" in
  Merge\ *|fixup!\ *|squash!\ *|amend!\ *)
    exit 0
    ;;
esac

QNYXOR_PATTERN='^Co-Authored-By: QNYXOR <qnyxor@pm\.me>'
NYX_PATTERN='^Co-Authored-By: NyX <nyx@qnyxor\.nexus>'

if ! grep -qE "$QNYXOR_PATTERN" "$MSG_FILE"; then
  echo "✗ commit message missing trailer:" >&2
  echo "  Co-Authored-By: QNYXOR <qnyxor@pm.me>" >&2
  echo "" >&2
  echo "Add it manually or use the canonical template in CONTRIBUTING.md." >&2
  exit 1
fi

if ! grep -qE "$NYX_PATTERN" "$MSG_FILE"; then
  echo "✗ commit message missing trailer:" >&2
  echo "  Co-Authored-By: NyX <nyx@qnyxor.nexus>" >&2
  echo "" >&2
  echo "Add it manually or use the canonical template in CONTRIBUTING.md." >&2
  exit 1
fi

exit 0
