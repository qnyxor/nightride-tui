# ---
# SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
# SPDX-License-Identifier: Apache-2.0
# ---

# NightRideTUI Makefile.
# Wraps the Rust toolchain entry points required by ghost-standard-rust §9.

.PHONY: build test lint fmt check clean doc audit verify fmt-check clippy build-release size install-hooks ci coverage coverage-text sign-release deploy notice

build:
	cargo build

test:
	cargo test --all --features test-export

lint:
	cargo clippy --all-targets --features test-export -- -D warnings

fmt:
	cargo fmt

fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets --features test-export -- -D warnings

check:
	cargo fmt --check && cargo clippy --all-targets --features test-export -- -D warnings && cargo test --features test-export

doc:
	cargo doc --no-deps

audit:
	@if command -v cargo-audit >/dev/null 2>&1; then \
		cargo audit; \
	else \
		echo "cargo-audit not installed; run: cargo install cargo-audit --locked (skipping)"; \
	fi

# audit-warn: like audit but exits 0 on warnings; exits non-zero only on vulns.
# Use until RUSTSEC-2026-0009 (time 0.3.45) is unblocked by rustc >= 1.88.
audit-warn:
	@if command -v cargo-audit >/dev/null 2>&1; then \
		cargo audit --deny warnings=none 2>&1 | true; \
		cargo audit; \
	else \
		echo "cargo-audit not installed; run: cargo install cargo-audit --locked (skipping)"; \
	fi

# verify-noaudit: fmt-check + clippy + test only (audit runs separately).
verify-noaudit: fmt-check clippy test
	@echo "==> fmt-check + clippy + test green (audit excluded)"

verify: fmt-check clippy test audit
	@echo "==> all gates green"

build-release:
	cargo build --release

size: build-release
	@ls -lh target/release/nightride-tui | awk '{print $$5, $$NF}'
	@strip target/release/nightride-tui 2>/dev/null || true
	@echo "--- after strip ---"
	@ls -lh target/release/nightride-tui | awk '{print $$5, $$NF}'

# Re-sign the release binary with the QNYXOR identity (`Identifier`
# matches the bundle id used by `directories::ProjectDirs::from(
# "nexus", "qnyxor", "nightride")`). cargo's default linker-signed
# adhoc identifier is a hash-suffixed crate name; this normalises it
# to a reverse-DNS form so `codesign -dvv` reads as project canon.
#
# Requires the binary to exist (run `make build-release` first).
# Adhoc only — no Apple Developer cert needed. Distribution-grade
# signing (Developer ID Application + notarisation) lands separately
# when the project leaves pre-alpha.
sign-release: build-release
	@codesign --remove-signature target/release/nightride-tui 2>/dev/null || true
	@codesign -s - --identifier nexus.qnyxor.nightride-tui --force target/release/nightride-tui
	@codesign -dvv target/release/nightride-tui 2>&1 | head -5
	@echo "==> signed adhoc as nexus.qnyxor.nightride-tui"

# Build, re-sign, and copy the release binary to ~/.local/bin/ so
# the installed `nightride-tui` on the operator's PATH gets the
# fresh build with the QNYXOR-canonical identifier. Idempotent;
# re-running just overwrites the previous deploy.
DEPLOY_DEST ?= $(HOME)/.local/bin/nightride-tui

deploy: sign-release
	@mkdir -p $(dir $(DEPLOY_DEST))
	@cp target/release/nightride-tui $(DEPLOY_DEST)
	@codesign --remove-signature $(DEPLOY_DEST) 2>/dev/null || true
	@codesign -s - --identifier nexus.qnyxor.nightride-tui --force $(DEPLOY_DEST)
	@echo "==> deployed to $(DEPLOY_DEST)"
	@codesign -dvv $(DEPLOY_DEST) 2>&1 | head -5

# CI-grade verification gate. Runs fmt + clippy + tests + audit in one
# pass; the same target the GitHub Actions workflow invokes.
ci: fmt-check clippy test audit
	@echo "==> ci gate green"

# Install the dual Co-Authored-By trailer hook into .git/hooks. Must
# be run once per fresh clone (git hooks are not tracked in-tree).
install-hooks:
	@mkdir -p .git/hooks
	@ln -sf ../../scripts/check-trailers.sh .git/hooks/commit-msg
	@chmod +x scripts/check-trailers.sh
	@echo "==> commit-msg hook installed -> .git/hooks/commit-msg"

# Coverage report via cargo-llvm-cov. Install once with:
#   cargo install cargo-llvm-cov --locked
# v1.2 cycle floor: ≥70 %. v1.3 target: ≥80 % (ghost-standard-rust §6).
# The HTML report lands in target/coverage/; the JSON summary feeds
# the cycle closure note.
coverage:
	@if command -v cargo-llvm-cov >/dev/null 2>&1; then \
		cargo llvm-cov --workspace --features test-export --html --output-dir target/coverage; \
		cargo llvm-cov --workspace --features test-export --json --output-path target/coverage/summary.json; \
		echo "==> HTML report: target/coverage/html/index.html"; \
		echo "==> JSON summary: target/coverage/summary.json"; \
	else \
		echo "cargo-llvm-cov not installed; run: cargo install cargo-llvm-cov --locked"; \
		echo "Fallback: cargo install cargo-tarpaulin && cargo tarpaulin --features test-export --out Html"; \
		exit 1; \
	fi

# Plain-text coverage summary (line + region %). Useful in CI logs and
# for the cycle closure note.
coverage-text:
	@if command -v cargo-llvm-cov >/dev/null 2>&1; then \
		cargo llvm-cov --workspace --features test-export --summary-only; \
	else \
		echo "cargo-llvm-cov not installed; run: cargo install cargo-llvm-cov --locked"; \
		exit 1; \
	fi

clean:
	cargo clean

# Generate THIRD_PARTY_LICENSES.md from the upstream `license` and
# `license-file` metadata of every transitive dependency in the
# resolved dep graph. Run before each public release so the bundled
# notice file matches the actual ship-time dep set.
#
# Requires `cargo-about` (one-time install):
#   cargo install cargo-about --locked
#
# The `about.toml` config (in repo root) controls the accepted
# license set + per-crate overrides. Keep that in version control.
notice:
	@if command -v cargo-about >/dev/null 2>&1; then \
		cargo about generate -o THIRD_PARTY_LICENSES.md about.hbs; \
		sh scripts/append-font-licenses.sh THIRD_PARTY_LICENSES.md; \
		echo "==> THIRD_PARTY_LICENSES.md regenerated from Cargo.lock + bundled fonts"; \
	else \
		echo "cargo-about not installed; run: cargo install cargo-about --locked"; \
		exit 1; \
	fi

# Refresh the embedded Iosevka Term Nerd Font from upstream.
#
# After running this target, paste the new shasum output into
# IOSEVKA_SHA256_PIN in build.rs and commit asset + Cargo.lock + the
# constant together so reviewers see the integrity link in one diff.
fetch-iosevka:
	@mkdir -p assets
	curl --fail-with-body -L -o assets/IosevkaTermNerdFont-Regular.ttf \
	  "https://github.com/ryanoasis/nerd-fonts/raw/master/patched-fonts/IosevkaTerm/IosevkaTermNerdFont-Regular.ttf"
	@head -c 4 assets/IosevkaTermNerdFont-Regular.ttf | xxd | grep -q '0001 0000' \
	  || { echo "ERROR: downloaded blob is not a TrueType font (magic bytes mismatch). Aborting."; rm -f assets/IosevkaTermNerdFont-Regular.ttf; exit 1; }
	@echo "--- new SHA-256 (paste into IOSEVKA_SHA256_PIN in build.rs) ---"
	@shasum -a 256 assets/IosevkaTermNerdFont-Regular.ttf
