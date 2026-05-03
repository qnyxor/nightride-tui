---
app:
  name: nightride-tui
  log_level: info            # one of: trace, debug, info, warn, error.

audio:
  default_station: nightride  # station slug to auto-connect at startup; empty -> picker.
  default_volume_percent: 50   # initial volume (0..=100).
  sample_rate_hz: 48000      # output device sample rate.
  fft_size: 1024             # power of two; spectrum frame size.

network:
  connect_timeout_secs: 10            # initial TCP/TLS dial budget.
  reconnect_max_attempts: 8           # exponential backoff attempts before surfacing an error.
  reconnect_backoff_initial_ms: 500   # first delay after a transport failure.
  reconnect_backoff_max_ms: 30000     # ceiling for the exponential schedule.

theme:
  mode: auto                 # auto -> follow current station; fixed -> always use fixed_theme_slug.
  fixed_theme_slug: ""       # only consulted when mode == fixed.
  eq_style: waves             # one of: bars, waves, both.
  reduced_motion: false      # disables animated transitions when true.

keymap:
  play_pause: "Space"
  stop: "s"
  mute: "m"
  volume_up: "+"
  volume_down: "-"
  next_station: "]"
  prev_station: "["
  toggle_eq: "e"
  help: "?"
  quit: "q"

# The station catalog is intentionally empty here. The canonical list of
# nine stations (slugs, URLs, accents) lives in `src/station.rs` as
# `DEFAULT_STATIONS`; entries set here override individual rows by slug
# but every override URL still passes the `stream.nightride.fm` host
# allow-list (security > agility canon).
stations: []
---

# NightRideTUI Configuration

This file is the **only** runtime configuration source. The binary parses the
YAML frontmatter above and treats the body as human-facing documentation. Keep
the body under 250 lines; keep the frontmatter strictly machine-readable.

## `app`

| Key         | Type   | Range / values                            | Rationale                                                                 |
|-------------|--------|-------------------------------------------|---------------------------------------------------------------------------|
| `name`      | string | non-empty                                 | Identifier used for log scopes and OS-level user-agent strings.           |
| `log_level` | string | `trace`, `debug`, `info`, `warn`, `error` | Mapped to a `tracing-subscriber` env filter at startup.                   |

## `audio`

| Key                      | Type    | Range          | Rationale                                                                   |
|--------------------------|---------|----------------|-----------------------------------------------------------------------------|
| `default_station`        | string  | slug or empty  | When non-empty the binary auto-connects on launch instead of showing the picker. |
| `default_volume_percent` | integer | `0..=100`      | Initial volume; clamped on load to keep invariants in `audio::pipeline`.    |
| `sample_rate_hz`         | integer | 22050..=192000 | Output sample rate. Must match the rodio device or be resampled.            |
| `fft_size`               | integer | power of two; 256..=4096 | Spectrum frame size; larger -> finer bins, more CPU.              |

## `network`

| Key                            | Type    | Range            | Rationale                                                          |
|--------------------------------|---------|------------------|--------------------------------------------------------------------|
| `connect_timeout_secs`         | integer | 1..=120          | TCP + TLS dial budget. Low values surface dead networks fast.      |
| `reconnect_max_attempts`       | integer | 0..=64           | Cap before reporting `ConnectionState::Error`. Zero disables retry. |
| `reconnect_backoff_initial_ms` | integer | 50..=10_000      | First sleep after a transport failure.                             |
| `reconnect_backoff_max_ms`     | integer | 1_000..=600_000  | Ceiling for the exponential schedule. Reached after ~log2 attempts.|

The reconnect schedule is `min(initial * 2^attempt, max)`, jittered uniformly by +/-15%.

## `theme`

| Key               | Type    | Range / values    | Rationale                                                          |
|-------------------|---------|-------------------|--------------------------------------------------------------------|
| `mode`            | string  | `auto`, `fixed`   | `auto` swaps theme on station change; `fixed` pins the user choice.|
| `fixed_theme_slug`| string  | known station slug| Only consulted when `mode == fixed`.                               |
| `eq_style`        | string  | `bars`, `waves`, `both` | Visual register of the spectrum analyzer.                    |
| `reduced_motion`  | boolean | `true` / `false`  | Disables animated transitions for accessibility.                   |

## `keymap`

Each entry is a key descriptor parsed by the `crossterm` event layer (single
character, named key like `Space`, or chord `Ctrl+x`). Keys must be unique. The
defaults are designed so the keyboard layout is reachable from a US/EU mapping
without modifiers wherever possible.

| Action         | Default | Description                                      |
|----------------|---------|--------------------------------------------------|
| `play_pause`   | `Space` | Toggle playback; idempotent on the audio pipeline.|
| `stop`         | `s`     | Stop the current stream and free the connection. |
| `mute`         | `m`     | Toggle mute without changing the saved volume.   |
| `volume_up`    | `+`     | Increase volume by 5%.                           |
| `volume_down`  | `-`     | Decrease volume by 5%.                           |
| `next_station` | `]`     | Cycle forward in the station catalog.            |
| `prev_station` | `[`     | Cycle backward in the station catalog.           |
| `toggle_eq`    | `e`     | Cycle the spectrum analyzer style.               |
| `help`         | `?`     | Open the help overlay.                           |
| `quit`         | `q`     | Graceful shutdown; flushes logs and audio.       |

## `stations`

Each entry models a streamable endpoint:

| Field                | Type    | Range / format                | Rationale                                                                 |
|----------------------|---------|-------------------------------|---------------------------------------------------------------------------|
| `slug`               | string  | `[a-z0-9-]+`                  | Stable identifier referenced by `default_station` and theme tables.        |
| `name`               | string  | non-empty                     | Human-readable label shown in the picker.                                  |
| `url`                | string  | https URL                     | Stream endpoint (Icecast or HLS playlist).                                 |
| `metadata_endpoint`  | string  | https URL or empty            | Optional SSE / JSON endpoint when ICY-metaint is unavailable.              |
| `accent_hex`         | string  | `#RRGGBB`                     | Primary accent for the per-station theme.                                  |
| `codec`              | string  | `aac`, `mp3`, `ogg`, `flac`   | Decoder selector for symphonia.                                            |
| `bitrate_kbps`       | integer | 32..=512                      | Informational; affects buffer sizing heuristics.                           |
| `enabled`            | boolean | `true` / `false`              | Hides the station from the picker without deleting the entry.              |

The empty array in the frontmatter is intentional. Populate it only
to override individual stations from the compiled-in
`DEFAULT_STATIONS` registry (`src/station.rs`); every override URL is
re-validated against the `stream.nightride.fm` host allow-list before
the supervisor connects.
