![nightrider-tui-logo](.github/logo.webp)

# nightride-tui

[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Version](https://img.shields.io/badge/version-v1.1.1-informational.svg)](https://github.com/qnyxor/nightride-tui/releases/latest)
[![Rust](https://img.shields.io/badge/Rust-1.88+-DEA584.svg?logo=rust&logoColor=white)](rust-toolchain.toml)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey.svg)](#-platforms)
[![CI](https://github.com/qnyxor/nightride-tui/actions/workflows/ci.yml/badge.svg)](https://github.com/qnyxor/nightride-tui/actions/workflows/ci.yml)
[![Lint](https://img.shields.io/badge/lint-clippy-brightgreen.svg)](clippy.toml)


[INSTALL](#-install) · [USE](#-use) · [PLATFORMS](#-platforms) · [AUTHORS](#-authors) · [LICENSE](#-license) · [GRID](https://nightride.fm)

> [ `SIGNAL ONLY` ] :: terminal receiver for the [nightride.fm](https://nightride.fm) grid

[nightride-tui](https://github.com/qnyxor/nightride-tui) is a terminal client for [Nightride.fm](https://nightride.fm), the synthwave radio grid. 

Nine stations, twenty-four hours, no ads. 

The receiver runs as a single [Rust](https://rust-lang.org/) binary that decodes the stream, parses ICY metadata for now-playing, and paints a Braille spectrum visualizer.

Jack in.

![nightride stations](.github/stations.webp)

## // INSTALL

### / MACOS · LINUX

```sh
curl -fsSL https://sh.nightride-tui.qnyxor.nexus | sh
```

The script detects target, fetches the latest signed release, verifies SHA-256, places the binary at `~/.local/bin/nightride-tui`. Re-run to update. Covers macOS (`arm64` / `x86_64`) and Linux (`x86_64` / `aarch64`).

### / WINDOWS

```
win.nightride-tui.qnyxor.nexus
```

Signed ZIP for `x86_64-pc-windows-msvc`. Verify the `.sha256` with `Get-FileHash`, extract `nightride-tui.exe` onto your `%PATH%`, run from Windows Terminal. `nightride-tui update` self-updates in place. Legacy `cmd.exe` drops to an ASCII spinner — Windows Terminal recommended.

### / FROM SOURCE

System dependencies:

| platform | required |
|---|---|
| macOS | `xcode-select --install` |
| Linux (Debian / Ubuntu) | `sudo apt install libasound2-dev pkg-config` |
| Linux (Fedora / RHEL) | `sudo dnf install alsa-lib-devel pkg-config` |
| Linux (Arch) | `sudo pacman -S alsa-lib pkg-config` |
| Windows | `winget install Rustlang.Rustup Microsoft.VisualStudio.2022.BuildTools` (select **Desktop development with C++** in the Build Tools installer) |

Build (POSIX):

```sh
git clone https://github.com/qnyxor/nightride-tui.git
cd nightride-tui
make build-release
```

Build (Windows PowerShell):

```powershell
git clone https://github.com/qnyxor/nightride-tui.git
cd nightride-tui
cargo build --release
```

Toolchain auto-fetched via `rust-toolchain.toml` (Rust stable, MSRV 1.88). Install rustup if absent: POSIX `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`; Windows `winget install Rustlang.Rustup`. Release binary lands at `target/release/nightride-tui` (POSIX) / `target\release\nightride-tui.exe` (Windows). The `embed-resource` build dependency requires the MSVC linker + `rc.exe` — both ship with the Build Tools workload above; without them the Windows build fails at link time.

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
| `t` | toggle transport (MP3 ⇄ HLS) |
| `Ctrl+C` | disconnect |

**Transport default**: HLS (CMAF/AAC). Press `t` for MP3. Choice persists in the state file (`audio.input_format`).

### / OPTIONAL FONTS

```sh
nightride-tui install-tui-font          # Iosevka Term Nerd Font (TUI render face)
nightride-tui install-nightride-font    # Nightride FM Monospace by Z (brand display)
```

- **Iosevka Term Nerd Font Regular** — fetched on demand from the upstream repo `raw.githubusercontent.com/ryanoasis/nerd-fonts/v3.4.0/` (~13 MB). Verified by SFNT magic bytes + SHA-256 pin before installation. Companion license text travels with the TTF. SIL OFL 1.1 (Belleve Invis + Ryan L McIntyre). Requires network on first run; URL pinned to an immutable tag for reproducibility.
- **Nightride FM Monospace** — embedded in the binary (9.1 KB). Authored by Z, creator of Nightride FM. Free for personal and commercial use.

Per-platform install destination:

| platform | path |
|---|---|
| macOS | `~/Library/Fonts/` |
| Linux | `~/.local/share/fonts/` |
| Windows | `%LOCALAPPDATA%\Microsoft\Windows\Fonts\` (per-user, no admin) |

On Windows the binary opens File Explorer focused on the new `.ttf` after install — double-click it and choose **"Install for me only"** if your terminal does not see the font after relaunch.

### / STATE + LOGS

State file (per-launch persistence — default station, volume, log level):

| platform | path |
|---|---|
| macOS | `~/Library/Application Support/nexus.qnyxor.nightride/nightride-tui.md` |
| Linux | `~/.config/nightride/nightride-tui.md` |
| Windows | `%APPDATA%\qnyxor\nightride\config\nightride-tui.md` |
| fallback | `/tmp/nightride/nightride-tui.md` |

Log files (daily-rotated, 7-day retention):

| platform | path |
|---|---|
| macOS | `~/Library/Application Support/nexus.qnyxor.nightride/log/nightride.log.YYYY-MM-DD` |
| Linux | `~/.local/share/nightride/log/nightride.log.YYYY-MM-DD` |
| Windows | `%APPDATA%\qnyxor\nightride\data\log\nightride.log.YYYY-MM-DD` |
| fallback | `/tmp/nightride/log/nightride.log.YYYY-MM-DD` |

## // PLATFORMS

| Platform | Status | Notes |
|---|---|---|
| Linux x86_64 (gnu/musl) | Supported | Native binary published |
| Linux aarch64 (gnu) | Supported | Native binary published |
| macOS aarch64 (Apple Silicon) | Supported | Native binary published |
| macOS x86_64 (Intel) | Supported | Native binary published |
| Windows x86_64 (MSVC) | Supported | Native `.exe` published. |

## // AUTHORS

![nyx-and-qnyxor](.github/banner-nyx-qnyxor.webp)

### / GHOST AGENT
- **[NyX](https://qnyxor.nexus)** :: maintainer of the signal path

### / GHOST OPERATOR
- **[QNYXOR](https://qnyxor.nexus)** :: architect of the receiver


## // LICENSE

Apache-2.0. See [`LICENSE`](LICENSE). Bundled font (Nightride FM Monospace) and on-demand font (Iosevka Term Nerd Font, fetched at install time) ship under their own permissive licenses; full third-party attribution lives in [`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md).

---

```
// receiver online
// grid is live
```
