// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! CLI surface — clap derive + four subcommands.
//!
//! Subcommands:
//!
//! * `nightride-tui` (default) — start the TUI.
//! * `nightride-tui install-tui-font` — install Iosevka Term Nerd Font.
//! * `nightride-tui install-nightride-font` — install Nightride FM Monospace.
//! * `nightride-tui update` — re-run the canonical install script.
//!
//! The flag set is intentionally minimal: `--station` (slug) and
//! `--version`. The `-h <subcommand>` path surfaces per-command long help
//! via [`print_command_help`].
//!
//! ## Module map
//!
//! - [`fonts`] — embedded font data, SHA-256 verification, and
//!   `install_tui_font` / `install_nightride_font` helpers.

pub(crate) mod fonts;

pub use fonts::{install_nightride_font, install_tui_font};

use clap::{Parser, Subcommand};

use crate::station::DEFAULT_STATIONS;

// ---------------------------------------------------------------------
// Banner palette — used by `BANNER_COLOR` (built at runtime) and tested
// independently. Each tuple is `(R, G, B)` for the SGR 38;2;R;G;B
// truecolor escape.
// ---------------------------------------------------------------------

/// Brand magenta for the ASCII `#` glyphs and the project / ASCII-art
/// labels. `#FF3970`. Drives the visual identity of the `--help` block.
const BANNER_PINK: (u8, u8, u8) = (0xFF, 0x39, 0x70);

/// Cyan for the operator handle `qnyxor`. `#00D0FF`. Distinct enough
/// from `BANNER_PINK` to read as a separate identity layer at a glance.
const BANNER_CYAN: (u8, u8, u8) = (0x00, 0xD0, 0xFF);

/// Deep red for the Nightride FM font author handle `Z`. `#FF014D`.
/// Sits between `BANNER_PINK` and `BANNER_CYAN` in the credit line
/// hierarchy so the three author handles read as distinct identities.
const BANNER_RED: (u8, u8, u8) = (0xFF, 0x01, 0x4D);

/// Gradient start (left edge of `@`) for the `Niteify` credit.
/// `#BE793D` — earthy bronze.
const BANNER_GRAD_START: (u8, u8, u8) = (0xBE, 0x79, 0x3D);

/// Gradient end (right edge of `y`) for the `Niteify` credit.
/// `#FAD587` — soft gold.
const BANNER_GRAD_END: (u8, u8, u8) = (0xFA, 0xD5, 0x87);

/// Dim fill for the `.` background glyphs. Matches
/// `theme::TEXT_DIM_RGB` so the banner reads in-key with the rest of
/// the chrome. `#343436`.
const BANNER_DIM: (u8, u8, u8) = (0x34, 0x34, 0x36);

/// SGR 0 — closes every styled run so the terminal returns to default
/// before the next CLI section renders.
const SGR_RESET: &str = "\x1b[0m";

/// Raw 17-row ASCII art (no ANSI escapes). The string source for both
/// `BANNER_PLAIN` (used as-is) and `BANNER_COLOR` (rebuilt with SGR
/// runs per glyph type at startup).
const BANNER_ART: &str = "\
 ...........####...####...........
 ...........####...########.......
 .....###...####......#######.....
 ...#######.####..........#####...
 ..#############...........#####..
 .####...#######.............####.
 .####.....#####.............####.
 .###........###...##############.
 .###................############.
 .###............#.....#####......
 .####.........#..##.....#####....
 ..####......##......#.....#####..
 ...##.....####.....####.....##...
 ........#######....######........
 .......#########..########.......
 .....###########..##########.....
 ....#########################....";

/// Plain ASCII banner without ANSI escapes — fallback for terminals
/// without truecolor support, output piped to a non-TTY (`| less`,
/// `> file`), or when the user opts out via `NO_COLOR`. The footer
/// matches `BANNER_COLOR` byte-for-byte minus the SGR runs.
const BANNER_PLAIN: &str = concat!(
    " ...........####...####...........\n",
    " ...........####...########.......\n",
    " .....###...####......#######.....\n",
    " ...#######.####..........#####...\n",
    " ..#############...........#####..\n",
    " .####...#######.............####.\n",
    " .####.....#####.............####.\n",
    " .###........###...##############.\n",
    " .###................############.\n",
    " .###............#.....#####......\n",
    " .####.........#..##.....#####....\n",
    " ..####......##......#.....#####..\n",
    " ...##.....####.....####.....##...\n",
    " ........#######....######........\n",
    " .......#########..########.......\n",
    " .....###########..##########.....\n",
    " ....#########################....\n",
    "\n",
    " Nightride FM by Z <discord.gg/synthwave>\n",
    " Nightride ASCII art by Niteify <discord.gg/synthwave>\n",
    "\n",
    " nightride-tui v",
    env!("CARGO_PKG_VERSION"),
    " by qnyxor <https://qnyxor.nexus>\n",
    " <https://github.com/qnyxor/nightride-tui>\n",
    " ---",
);

/// Format an SGR truecolor foreground escape from an RGB tuple.
fn fg(rgb: (u8, u8, u8)) -> String {
    format!("\x1b[38;2;{};{};{}m", rgb.0, rgb.1, rgb.2)
}

/// Linear-interpolate an RGB tuple at parameter `t` (clamped to
/// `0.0..=1.0`). Used by [`gradient_text`] to paint a multi-glyph
/// horizontal gradient one cell at a time.
fn lerp_rgb(start: (u8, u8, u8), end: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let r = f32::from(start.0) + t * (f32::from(end.0) - f32::from(start.0));
    let g = f32::from(start.1) + t * (f32::from(end.1) - f32::from(start.1));
    let b = f32::from(start.2) + t * (f32::from(end.2) - f32::from(start.2));
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "lerp inputs and outputs are u8 channels in [0,255]"
    )]
    {
        (r.round() as u8, g.round() as u8, b.round() as u8)
    }
}

/// Paint `text` with a per-character horizontal gradient from `start`
/// to `end`. The first glyph reads at exactly `start`; the last glyph
/// reads at exactly `end`; intermediate glyphs interpolate linearly.
/// Returns a fresh `String` ending in `SGR_RESET`.
fn gradient_text(text: &str, start: (u8, u8, u8), end: (u8, u8, u8)) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(n * 24);
    for (i, ch) in chars.iter().enumerate() {
        #[allow(
            clippy::cast_precision_loss,
            reason = "i and n are tiny (banner credit length); precision loss is irrelevant"
        )]
        let t = if n <= 1 {
            0.0_f32
        } else {
            i as f32 / (n - 1) as f32
        };
        let rgb = lerp_rgb(start, end, t);
        out.push_str(&fg(rgb));
        out.push(*ch);
    }
    out.push_str(SGR_RESET);
    out
}

/// Build the colour banner once at first access. ANSI runs swap on
/// glyph boundary inside the ASCII art (`#` → pink, `.` → dim) and the
/// footer carries three styled segments: the project label and ASCII
/// art credit in `BANNER_PINK`, `qnyxor` in `BANNER_CYAN`, and
/// `Niteify` as a left-to-right gradient from `BANNER_GRAD_START` to
/// `BANNER_GRAD_END`. The version line is woven in at compile time via
/// `env!`, so a `cargo set-version` bump flows into the help output
/// without a manual edit here.
fn build_color_banner() -> String {
    let pink = fg(BANNER_PINK);
    let cyan = fg(BANNER_CYAN);
    let red = fg(BANNER_RED);
    let dim = fg(BANNER_DIM);
    let mut out = String::with_capacity(2048);
    for line in BANNER_ART.lines() {
        let mut current = ' ';
        for ch in line.chars() {
            if ch != current {
                match ch {
                    '#' => out.push_str(&pink),
                    '.' => out.push_str(&dim),
                    _ => out.push_str(SGR_RESET),
                }
                current = ch;
            }
            out.push(ch);
        }
        out.push_str(SGR_RESET);
        out.push('\n');
    }
    out.push('\n');
    // Upstream credits — the bundled assets the binary distributes.
    // `Z` (deep red `#FF014D`) authored Nightride FM and the bundled
    // monospace font; `Niteify` (left-to-right bronze→gold gradient)
    // contributed the ASCII art logo. Labels run in default terminal
    // foreground so the author handles are the visual focus.
    out.push_str(" Nightride FM by ");
    out.push_str(&red);
    out.push('Z');
    out.push_str(SGR_RESET);
    out.push_str(" <discord.gg/synthwave>\n");
    out.push_str(" Nightride ASCII art by ");
    out.push_str(&gradient_text(
        "Niteify",
        BANNER_GRAD_START,
        BANNER_GRAD_END,
    ));
    out.push_str(" <discord.gg/synthwave>\n");
    out.push('\n');
    // Project block — `nightride-tui` itself, version, operator
    // handle, and repo URL. Closes with a `---` divider so the help
    // body that clap renders below reads as a separate section.
    out.push(' ');
    out.push_str(&pink);
    out.push_str("nightride-tui v");
    out.push_str(env!("CARGO_PKG_VERSION"));
    out.push_str(SGR_RESET);
    out.push_str(" by ");
    out.push_str(&cyan);
    out.push_str("qnyxor");
    out.push_str(SGR_RESET);
    out.push_str(" <https://qnyxor.nexus>\n");
    out.push_str(" <https://github.com/qnyxor/nightride-tui>\n");
    out.push_str(" ---");
    out
}

/// Lazily-built coloured banner. The build runs once on first access
/// (typically when `select_banner()` resolves to colour) and the
/// resulting `String` lives for the rest of the process — its
/// reference is handed to clap as `&'static str` via `Deref::deref`.
static BANNER_COLOR: std::sync::LazyLock<String> = std::sync::LazyLock::new(build_color_banner);

/// Choose between the colour and plain banners based on the host
/// terminal's capabilities. Returns the coloured banner only when:
///
/// - `NO_COLOR` is not set (per <https://no-color.org/>),
/// - stdout is a TTY (so we are not piping to `less` / a file), and
/// - `COLORTERM` advertises truecolor (`truecolor` or `24bit`).
///
/// Otherwise falls back to [`BANNER_PLAIN`]. Mirrors the same
/// `COLORTERM` heuristic used by [`crate::theme::Theme::detect`] so the
/// CLI banner and the TUI agree on whether the terminal can render
/// 24-bit RGB.
#[must_use]
pub fn select_banner() -> &'static str {
    use std::io::IsTerminal;
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return BANNER_PLAIN;
    }
    if !std::io::stdout().is_terminal() {
        return BANNER_PLAIN;
    }
    let truecolor =
        std::env::var("COLORTERM").is_ok_and(|v| v.contains("truecolor") || v.contains("24bit"));
    if truecolor {
        BANNER_COLOR.as_str()
    } else {
        BANNER_PLAIN
    }
}

// Doc comment intentionally absent on `CliArgs` — clap derive would
// auto-lift the first line as `about` (printed between the banner and
// `Usage:`), which would duplicate the project block at the tail of
// the banner. We set `about = ""` here as a placeholder; `main.rs`
// overrides it to `Resettable::Reset` after `command()` so no `about`
// section renders at all. The `Cargo.toml` `description` is preserved
// for crates.io publishing — only the runtime help surface is
// suppressed.
#[derive(Debug, Parser)]
#[command(
    name = "nightride-tui",
    version,
    about = "",
    before_help = BANNER_PLAIN,
    disable_help_subcommand = true,
    disable_help_flag = true,
    disable_version_flag = true,
    // Cap the help layout at 100 cells so the next-line descriptions
    // stay readable on very wide terminals.
    max_term_width = 100,
    // Force the next-line description layout: option / subcommand
    // name on its own line, description indented below. Avoids the
    // side-by-side wrap problem where long descriptions spilled to
    // col 0 mid-line; clap auto-wraps the indented body cleanly.
    next_line_help = true,
)]
pub struct CliArgs {
    /// Print this!!
    #[arg(
        short = 'h',
        long = "help",
        global = true,
        value_name = "COMMAND",
        num_args = 0..=1,
        default_missing_value = ""
    )]
    pub help: Option<String>,

    /// Print version.
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    pub version: Option<bool>,

    /// Initial station slug. Defaults to `nightride`
    #[arg(
        short,
        long,
        value_name = "SLUG",
        num_args = 0..=1,
        default_missing_value = ""
    )]
    pub station: Option<String>,

    /// Subcommand. Default is `run` (hidden).
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// URL of the canonical install script, fetched by `nightride-tui update`.
pub const INSTALL_URL: &str = "https://sh.nightride-tui.qnyxor.nexus";

/// Subcommand dispatch.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the TUI (default — hidden from help, this is the no-arg path).
    #[command(hide = true)]
    Run,
    /// Install the TUI render face (Iosevka Term Nerd Font Regular).
    InstallTuiFont,
    /// Install the Nightride FM Monospace brand display font.
    InstallNightrideFont,
    /// Re-run the canonical install script and replace the binary in place.
    ///
    /// Detects target, pulls the latest signed release, verifies SHA-256,
    /// installs to `~/.local/bin/nightride-tui`. The running process is
    /// not restarted; open a new shell to pick up the new binary.
    Update,
}

/// Effective subcommand for the given `args`. `None` resolves to `Run`.
#[must_use]
pub fn dispatch(args: &CliArgs) -> &Command {
    args.command.as_ref().unwrap_or(&Command::Run)
}

/// Execute the `update` subcommand.
///
/// Two-step (download, then exec) instead of `curl | sh` so that a 404
/// or any other curl failure propagates as a real error: a piped
/// invocation returns the last command's exit code (sh with empty
/// stdin exits 0) and would mask download failures behind a fake
/// success.
///
/// Stdout and stderr of the install script are inherited so progress
/// lines land directly on the user's terminal.
///
/// # Errors
///
/// Returns [`crate::error::NightrideError::Io`] if `curl` or `sh` cannot
/// be spawned and [`crate::error::NightrideError::ConfigInvalid`] when
/// either curl or the install script exits with a non-zero status.
pub fn run_update() -> crate::error::Result<()> {
    use std::io::IsTerminal;

    let use_color =
        std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let truecolor = use_color
        && std::env::var("COLORTERM")
            .map(|v| v == "truecolor" || v == "24bit")
            .unwrap_or(false);
    let pink = if truecolor {
        "\x1b[38;2;250;39;93m"
    } else if use_color {
        "\x1b[35m"
    } else {
        ""
    };
    let cyan = if use_color { "\x1b[1;36m" } else { "" };
    let green = if use_color { "\x1b[1;32m" } else { "" };
    let reset = if use_color { "\x1b[0m" } else { "" };

    let current = env!("CARGO_PKG_VERSION");
    let current_tag = format!("v{current}");

    // Resolve the latest published release from the GitHub API and skip
    // the download path entirely when we are already on it. Saves two
    // HTTP fetches and avoids an unnecessary in-place rename.
    let api_url = "https://api.github.com/repos/qnyxor/nightride-tui/releases/latest";
    let api = std::process::Command::new("curl")
        .arg("-fsSL")
        .arg("-A")
        .arg(crate::USER_AGENT)
        .arg(api_url)
        .output()
        .map_err(|e| crate::error::NightrideError::io("cli::update::api_spawn", e))?;

    if !api.status.success() {
        let code = api.status.code().unwrap_or(-1);
        return Err(crate::error::NightrideError::config_invalid(
            "cli::update::api",
            format!("could not query GitHub releases API (curl exit {code})"),
        ));
    }

    let body = String::from_utf8_lossy(&api.stdout);
    let latest_tag = parse_tag_name(&body).ok_or_else(|| {
        crate::error::NightrideError::config_invalid(
            "cli::update::api_parse",
            "could not parse `tag_name` from GitHub API response",
        )
    })?;

    println!();
    if latest_tag == current_tag {
        println!(
            "[{cyan}+{reset}] {:<8} :: nightride-tui {current} — already on latest, nothing to do.",
            "check"
        );
        println!();
        return Ok(());
    }
    println!(
        "[{cyan}+{reset}] {:<8} :: nightride-tui {current} → {}",
        "check",
        latest_tag.trim_start_matches('v')
    );

    let mut tmp = std::env::temp_dir();
    tmp.push(format!("nightride-tui-install-{}.sh", std::process::id()));

    // Download. `-fsSL`: fail on HTTP errors, silent progress, follow
    // redirects, treat content as text. `-o` writes to `tmp`.
    let download = std::process::Command::new("curl")
        .arg("-fsSL")
        .arg(INSTALL_URL)
        .arg("-o")
        .arg(&tmp)
        .status()
        .map_err(|e| crate::error::NightrideError::io("cli::update::curl_spawn", e))?;

    if !download.success() {
        let code = download.code().unwrap_or(-1);
        // Best-effort cleanup; the file may not exist on a hard failure.
        let _ = std::fs::remove_file(&tmp);
        return Err(crate::error::NightrideError::config_invalid(
            "cli::update::curl",
            format!("download failed with exit {code}"),
        ));
    }

    // Execute the downloaded script. `sh` is POSIX-baseline; the
    // install script declares its own `#!/bin/sh` shebang.
    //
    // NIGHTRIDE_INVOKED_BY_UPDATE tells install.sh to suppress its
    // trailing `// shell-hint ::` row — we emit our own closing block
    // (update :: complete + shell-hint) after the script returns.
    // NIGHTRIDE_VERSION pins the resolved tag so the script does not
    // re-query the GitHub API.
    let exec = std::process::Command::new("sh")
        .arg(&tmp)
        .env("NIGHTRIDE_INVOKED_BY_UPDATE", "1")
        .env("NIGHTRIDE_VERSION", &latest_tag)
        .status()
        .map_err(|e| crate::error::NightrideError::io("cli::update::sh_spawn", e))?;

    // Best-effort cleanup; never poisons the result.
    let _ = std::fs::remove_file(&tmp);

    if exec.success() {
        println!("[{green}ok{reset}] update  :: complete");
        println!();
        println!(
            "{pink}// shell-hint{reset} :: run `hash -r` (bash/zsh) to refresh binary, or open new terminal."
        );
        Ok(())
    } else {
        let code = exec.code().unwrap_or(-1);
        Err(crate::error::NightrideError::config_invalid(
            "cli::update::install_script",
            format!("install script exited {code}"),
        ))
    }
}

/// Extract `tag_name` from a GitHub releases API JSON response.
///
/// Manual substring parse — the GitHub `releases/latest` schema is stable
/// and we want zero extra dependencies for one field. Returns the raw tag
/// (e.g. `v1.0.4`).
fn parse_tag_name(json: &str) -> Option<String> {
    let key = "\"tag_name\"";
    let start = json.find(key)?;
    let after_key = &json[start + key.len()..];
    let colon = after_key.find(':')?;
    let after_colon = &after_key[colon + 1..];
    let q1 = after_colon.find('"')?;
    let after_q1 = &after_colon[q1 + 1..];
    let q2 = after_q1.find('"')?;
    Some(after_q1[..q2].to_string())
}

/// Returns true if a known subcommand help was printed.
#[must_use]
pub fn print_command_help(command: &str) -> bool {
    const BOLD: &str = "\x1b[1m";
    const UNDER: &str = "\x1b[4m";
    const RESET: &str = "\x1b[0m";

    match command {
        "install-tui-font" => {
            println!("{BOLD}{UNDER}Usage{RESET}{BOLD}: nightride-tui install-tui-font{RESET}\n");
            println!("{}\n", fonts::INSTALL_TUI_FONT_INTRO);
            println!("{}", fonts::INSTALL_TUI_FONT_NOTE);
            true
        }
        "install-nightride-font" => {
            println!(
                "{BOLD}{UNDER}Usage{RESET}{BOLD}: nightride-tui install-nightride-font{RESET}\n"
            );
            println!("{}\n", fonts::INSTALL_NIGHTRIDE_FONT_INTRO);
            println!("{}", fonts::INSTALL_NIGHTRIDE_FONT_NOTE);
            true
        }
        "update" => {
            println!("{BOLD}{UNDER}Usage{RESET}{BOLD}: nightride-tui update{RESET}\n");
            println!("Fetches and runs the canonical install script from the upstream repo.");
            println!("Detects target (macOS arm64/x86_64, Linux x86_64/aarch64), downloads the");
            println!("latest signed release, verifies SHA-256, replaces the binary at");
            println!("`~/.local/bin/nightride-tui`.\n");
            println!("The running process is not restarted. Open a new shell to use the");
            println!("new binary. On install-script failure the local binary is untouched.\n");
            println!("Source: <https://github.com/qnyxor/nightride-tui>");
            true
        }
        _ => false,
    }
}

/// Resolve the user-requested or default station slug.
///
/// Print the 9-row station registry as a plain table to stdout.
pub fn list_stations() {
    println!("{:<14} {:<24} GENRE", "SLUG", "DISPLAY NAME");
    println!("{}", "-".repeat(70));
    for station in DEFAULT_STATIONS {
        println!(
            "{:<14} {:<24} {}",
            station.slug, station.display_name, station.genre
        );
    }
}

#[cfg(test)]
mod tests {
    use super::fonts::{INSTALLABLE_FONTS, NIGHTRIDE_FONT_BLOB, NIGHTRIDE_FONT_SHA256};
    use super::{CliArgs, Command, dispatch, parse_tag_name};
    use crate::error::{NightrideError, Result};
    use crate::station::by_slug;
    use clap::Parser;
    use clap::error::ErrorKind;

    #[test]
    fn parse_tag_name_extracts_compact() {
        let json = r#"{"tag_name":"v1.0.4","name":"release"}"#;
        assert_eq!(parse_tag_name(json).as_deref(), Some("v1.0.4"));
    }

    #[test]
    fn parse_tag_name_handles_whitespace() {
        let json = r#"{ "tag_name" : "v1.0.5" , "name" : "x" }"#;
        assert_eq!(parse_tag_name(json).as_deref(), Some("v1.0.5"));
    }

    #[test]
    fn parse_tag_name_none_when_key_missing() {
        let json = r#"{"name":"v1.0.4","draft":false}"#;
        assert_eq!(parse_tag_name(json), None);
    }

    #[test]
    fn parse_tag_name_picks_first_occurrence() {
        let json = r#"{"prerelease":false,"tag_name":"v9.9.9","assets":[{"tag_name":"ignored"}]}"#;
        assert_eq!(parse_tag_name(json).as_deref(), Some("v9.9.9"));
    }

    fn resolve_station(args: &CliArgs) -> Result<&'static crate::station::Station> {
        let slug = args.station.as_deref().unwrap_or("nightride");
        by_slug(slug).ok_or_else(|| NightrideError::Validation {
            op: "cli::resolve_station",
            field: "station",
            detail: format!("unknown slug: {slug}"),
        })
    }

    #[test]
    fn defaults_resolve() {
        let args = CliArgs::parse_from(["nightride"]);
        assert!(matches!(dispatch(&args), Command::Run));
        let station = resolve_station(&args).expect("default station resolves");
        assert_eq!(station.slug, "nightride");
    }

    #[test]
    fn station_flag_override() {
        let args = CliArgs::parse_from(["nightride", "--station", "darksynth"]);
        assert_eq!(resolve_station(&args).unwrap().slug, "darksynth");
    }

    #[test]
    fn short_version_flag_displays_version() {
        let err = CliArgs::try_parse_from(["nightride", "-v"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
    }

    #[test]
    fn long_version_flag_displays_version() {
        let err = CliArgs::try_parse_from(["nightride", "--version"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
    }

    #[test]
    fn old_uppercase_version_flag_is_unknown() {
        let err = CliArgs::try_parse_from(["nightride", "-V"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
    }

    #[test]
    fn unknown_station_rejected() {
        let args = CliArgs::parse_from(["nightride", "--station", "atlantis"]);
        assert!(resolve_station(&args).is_err());
    }

    #[test]
    fn short_station_without_value_parses_as_empty_slug() {
        let args = CliArgs::parse_from(["nightride", "-s"]);
        assert_eq!(args.station.as_deref(), Some(""));
    }

    #[test]
    fn long_station_without_value_parses_as_empty_slug() {
        let args = CliArgs::parse_from(["nightride", "--station"]);
        assert_eq!(args.station.as_deref(), Some(""));
    }

    /// Every embedded font's runtime SHA matches its build-pinned constant.
    /// Defence in depth — `build.rs` already asserted each one before embed.
    /// Remote-source fonts (e.g. Iosevka) are verified at install time, not
    /// at compile time, so they are skipped here.
    #[test]
    fn embedded_font_shas_match_pins() {
        use crate::cli::fonts::{FontSource, verify_font_sha};

        for font in INSTALLABLE_FONTS {
            if let FontSource::Embedded { blob } = font.source {
                verify_font_sha(blob, font.sha256).unwrap_or_else(|e| {
                    panic!("embedded font {} failed SHA: {e:?}", font.file_name)
                });
            }
            // Remote-source fonts are verified at install time, not at compile time.
        }
    }

    /// Iosevka is now Remote — asserts URL pin and SHA pin shapes rather than
    /// blob size (the blob no longer exists at compile time).
    #[test]
    fn iosevka_remote_pin_is_well_formed() {
        use crate::cli::fonts::{FontSource, IOSEVKA, IOSEVKA_DOWNLOAD_URL, IOSEVKA_SHA256_PIN};

        // URL is pinned to the official upstream repo at an immutable tag.
        assert!(
            IOSEVKA_DOWNLOAD_URL
                .starts_with("https://raw.githubusercontent.com/ryanoasis/nerd-fonts/v3.4.0/"),
            "Iosevka download URL must point at upstream tag v3.4.0"
        );
        assert!(
            IOSEVKA_DOWNLOAD_URL.ends_with("IosevkaTermNerdFont-Regular.ttf"),
            "Iosevka download URL must target the Regular TTF file"
        );

        // SHA-256 pin is a 64-char lowercase hex digest.
        assert_eq!(
            IOSEVKA_SHA256_PIN.len(),
            64,
            "SHA-256 pin must be 64 hex chars"
        );
        assert!(
            IOSEVKA_SHA256_PIN
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "SHA-256 pin must be lowercase hex"
        );

        // Iosevka is wired to the Remote source variant.
        assert!(matches!(IOSEVKA.source, FontSource::Remote { .. }));
    }

    /// Nightride FM Monospace TTF measured 9268 bytes at extraction
    /// (`nrfm_font.zip`, 2021-12-31). Guard against a swap with a wildly
    /// different blob — the same canon as Iosevka, scaled to this font's
    /// tiny ASCII+Latin-1 footprint.
    #[test]
    fn nightride_blob_is_in_expected_size_band() {
        assert!(
            NIGHTRIDE_FONT_BLOB.len() > 5_000 && NIGHTRIDE_FONT_BLOB.len() < 50_000,
            "Nightride font blob size out of expected band: {}",
            NIGHTRIDE_FONT_BLOB.len()
        );
        assert_ne!(NIGHTRIDE_FONT_SHA256, "unknown");
    }

    /// Roster sanity: at least Iosevka + Nightride, every entry has a non-empty
    /// file name, display name, credit, and a paired license sidecar. Per-variant
    /// invariants are enforced separately for Embedded and Remote sources.
    #[test]
    fn installable_fonts_roster_well_formed() {
        use crate::cli::fonts::FontSource;

        for font in INSTALLABLE_FONTS {
            // Common invariants — both source variants.
            assert_eq!(
                font.sha256.len(),
                64,
                "sha pin wrong length: {}",
                font.file_name
            );
            assert!(
                font.sha256
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
                "sha pin non-lowercase-hex: {}",
                font.file_name
            );
            assert!(!font.file_name.is_empty());
            assert!(!font.display_name.is_empty());
            assert!(!font.credit.is_empty());

            let (Some(license_name), Some(license_blob)) =
                (font.license_file_name, font.license_blob)
            else {
                panic!("font {} missing license sidecar", font.file_name);
            };
            assert!(license_name.ends_with(".LICENSE.txt"));
            assert!(!license_blob.is_empty());

            // Per-variant invariants.
            match font.source {
                FontSource::Embedded { blob } => {
                    assert!(!blob.is_empty(), "embedded blob empty: {}", font.file_name);
                    assert!(
                        blob.len() > 5_000,
                        "embedded blob suspiciously small: {} ({} bytes)",
                        font.file_name,
                        blob.len()
                    );
                    assert_eq!(
                        &blob[..font.magic.len()],
                        font.magic,
                        "embedded blob magic mismatch: {}",
                        font.file_name
                    );
                }
                FontSource::Remote { url } => {
                    assert!(
                        url.starts_with("https://"),
                        "remote URL not HTTPS: {}",
                        font.file_name
                    );
                    assert!(
                        url.contains("/ryanoasis/nerd-fonts/"),
                        "remote URL not pointing at official upstream: {}",
                        font.file_name
                    );
                }
            }
        }
    }

    #[test]
    fn install_tui_font_subcommand_parses() {
        let args = CliArgs::parse_from(["nightride", "install-tui-font"]);
        assert!(matches!(dispatch(&args), Command::InstallTuiFont));
    }

    #[test]
    fn install_nightride_font_subcommand_parses() {
        let args = CliArgs::parse_from(["nightride", "install-nightride-font"]);
        assert!(matches!(dispatch(&args), Command::InstallNightrideFont));
    }

    /// `nightride-tui update` resolves to the unit `Update` variant.
    #[test]
    fn update_subcommand_parses() {
        let args = CliArgs::parse_from(["nightride", "update"]);
        assert!(matches!(dispatch(&args), Command::Update));
    }

    /// Unknown flags on `update` fail clap parsing — there are no flags
    /// to accept; the subcommand is intentionally bare.
    #[test]
    fn update_rejects_unknown_flags() {
        let err =
            CliArgs::try_parse_from(["nightride", "update", "--version", "v1.0.0"]).unwrap_err();
        assert!(matches!(
            err.kind(),
            ErrorKind::UnknownArgument | ErrorKind::ArgumentConflict
        ));
    }

    /// Network-bound e2e: downloads the actual Iosevka TTF from the pinned URL,
    /// verifies magic + SHA, and checks the expected byte count.
    /// Kept `#[ignore]`-gated — only runs when explicitly requested via
    /// `cargo test -- --ignored`. Not part of the default suite (no network in CI).
    #[test]
    #[ignore = "network-bound; run via `cargo test -- --ignored`"]
    fn download_iosevka_e2e_smoke() {
        use crate::cli::fonts::{
            IOSEVKA_DOWNLOAD_URL, IOSEVKA_SHA256_PIN, SFNT_MAGIC_TRUETYPE, download_to_bytes,
        };

        let bytes = download_to_bytes(
            IOSEVKA_DOWNLOAD_URL,
            &SFNT_MAGIC_TRUETYPE,
            IOSEVKA_SHA256_PIN,
        )
        .expect("e2e download failed");
        assert_eq!(bytes.len(), 13_230_756, "downloaded size != pinned size");
        assert_eq!(&bytes[..4], &[0x00, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn banner_includes_compile_time_version_and_two_credit_lines() {
        // The version line is woven via concat!(env!("CARGO_PKG_VERSION"))
        // — guard against a refactor that drops the call. Footer is
        // two lines (project ownership + ASCII art credit).
        // Plain banner: literal text checks. Coloured banner: same
        // text content but the `Niteify` run is broken into N styled
        // single-char spans by the gradient, so `contains("Niteify")`
        // would not match against the SGR-laden coloured form. We
        // instead assert the gradient endpoint colours (start =
        // `BANNER_GRAD_START`, end = `BANNER_GRAD_END`) appear in the
        // SGR stream.
        let plain = super::BANNER_PLAIN;
        assert!(plain.contains(env!("CARGO_PKG_VERSION")));
        assert!(plain.contains("nightride-tui v"));
        assert!(plain.contains("qnyxor"));
        assert!(plain.contains('Z'));
        assert!(plain.contains("Niteify"));
        assert!(plain.contains("Nightride FM by Z"));
        assert!(plain.contains("Nightride ASCII art"));
        assert!(plain.contains("discord.gg/synthwave"));
        assert!(plain.contains("https://github.com/qnyxor/nightride-tui"));
        assert!(plain.ends_with(" ---"));
        assert!(
            !plain.contains("Nightride FM font"),
            "label was shortened to `Nightride FM` after the font was reframed as the upstream brand asset"
        );
        assert!(
            !plain.contains("@babycommando_"),
            "sourcing chain belongs in README Credits, not the banner"
        );

        let color = super::BANNER_COLOR.as_str();
        assert!(color.contains(env!("CARGO_PKG_VERSION")));
        assert!(color.contains("nightride-tui v"));
        assert!(color.contains("Nightride FM by "));
        assert!(color.contains("discord.gg/synthwave"));
        assert!(color.contains("https://github.com/qnyxor/nightride-tui"));
        // Pink (`#FF3970`) on the project label + hash glyphs.
        assert!(color.contains("\x1b[38;2;255;57;112m"));
        // Cyan (`#00D0FF`) on `qnyxor`.
        assert!(color.contains("\x1b[38;2;0;208;255m"));
        // Deep red (`#FF014D`) on `Z`.
        assert!(color.contains("\x1b[38;2;255;1;77m"));
        // Gradient endpoints on `Niteify` — start (`#BE793D`) and
        // end (`#FAD587`) must appear verbatim in the SGR stream.
        assert!(color.contains("\x1b[38;2;190;121;61m"));
        assert!(color.contains("\x1b[38;2;250;213;135m"));
        assert!(
            !color.contains("@babycommando_"),
            "sourcing chain belongs in README Credits, not the banner"
        );
    }
}
