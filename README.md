![nightrider-tui-logo](.github/logo.webp)

# nightride-tui

![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)
![Version](https://img.shields.io/badge/version-v1.0.0-informational.svg)
![Rust](https://img.shields.io/badge/Rust-1.85+-DEA584.svg?logo=rust&logoColor=white)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey.svg)
![CI](https://github.com/qnyxor/nightride-tui/actions/workflows/ci.yml/badge.svg)
![Lint](https://img.shields.io/badge/lint-clippy-brightgreen.svg)


[INSTALL](#-install) · [USE](#-useuse) · [AUTHORS](#-authors) · [LICENSE](#-license) · [GRID](https://nightride.fm)

> [ `SIGNAL ONLY` ] :: terminal receiver for the [nightride.fm](https://nightride.fm) grid

[nightride-tui](https://github.com/qnyxor/nightride-tui) is a terminal client for [Nightride.fm](https://nightride.fm), the synthwave radio grid. 

Nine stations, twenty-four hours, no ads. 

The receiver runs as a single [Rust](https://rust-lang.org/) binary that decodes the stream, parses ICY metadata for now-playing, and paints a Braille spectrum visualizer.

Jack in.

![nightride stations](.github/stations.webp)

## // INSTALL

```sh
curl -fsSL https://raw.githubusercontent.com/qnyxor/nightride-tui/main/scripts/install.sh | sh
```

The script detects target, fetches the latest signed release, verifies SHA-256, places the binary at `~/.local/bin/nightride-tui`. Re-run to update. Works on macOS (`arm64` / `x86_64`) and Linux (`x86_64` / `aarch64`).

### / FROM SOURCE

System dependencies:

| platform | required |
|---|---|
| macOS | `xcode-select --install` |
| Linux (Debian / Ubuntu) | `sudo apt install libasound2-dev pkg-config` |
| Linux (Fedora / RHEL) | `sudo dnf install alsa-lib-devel pkg-config` |
| Linux (Arch) | `sudo pacman -S alsa-lib pkg-config` |

Build:

```sh
git clone https://github.com/qnyxor/nightride-tui.git
cd nightride-tui
make build-release
```

Toolchain auto-fetched via `rust-toolchain.toml` (Rust stable, MSRV 1.85). Install rustup if absent: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`. Release binary lands at `target/release/nightride-tui`.

Self-update once installed:

```sh
nightride-tui update
```

Run `nightride-tui -h update` for the long form.

## // USE

```sh
nightride-tui                    # boot
nightride-tui -s darksynth       # boot on a specific station
nightride-tui -s                 # list the registry
nightride-tui -h <subcommand>    # contextual help
```

### / CONTROLS

| key | action |
|---|---|
| `→` / `←` | cycle station |
| `+` / `-` | volume |
| `m` | mute / unmute |
| `Ctrl+C` | disconnect |

### / OPTIONAL FONTS

```sh
nightride-tui install-tui-font          # Iosevka Term Nerd Font
nightride-tui install-nightride-font    # Nightride FM Monospace by Z
```

Both ship embedded in the binary, verified by SHA-256 at install time. SHA mismatch aborts the install.

### / STATE + LOGS

| platform | path |
|---|---|
| macOS | `~/Library/Application Support/nexus.qnyxor.nightride/nightride-tui.md` |
| Linux | `~/.config/nightride/nightride-tui.md` |
| fallback | `/tmp/nightride/nightride-tui.md` |

Logs rotate daily, 7-day retention, at `<config-dir>/log/nightride.log.YYYY-MM-DD`.

## // AUTHORS

![nyx-and-qnyxor](.github/banner-nyx-qnyxor.webp)

### / GHOST AGENT
- **[NyX](https://qnyxor.nexus)** :: maintainer of the signal path

### / GHOST OPERATOR
- **[QNYXOR](https://qnyxor.nexus)** :: architect of the receiver


## // LICENSE

Apache-2.0. See [`LICENSE`](LICENSE). Bundled fonts ship under their own permissive licenses; full third-party attribution lives in [`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md).

---

```
// receiver online
// grid is live
```
