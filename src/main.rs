// ---
// SPDX-FileCopyrightText: (c) 2026 QNYXOR <qnyxor@pm.me> <https://qnyxor.nexus>
// SPDX-License-Identifier: Apache-2.0
// SPDX-FileComment: Part of nightride-tui at <https://github.com/qnyxor/nightride-tui>
// ---

//! Thin binary shell for NightRideTUI.
//!
//! Owns the root `CancellationToken`, installs a panic hook that cancels
//! the tree, parses CLI args, and dispatches:
//!
//! - `install-tui-font` and `install-nightride-font` go directly to
//!   `cli` helpers (each font has its own subcommand so the credit +
//!   post-install note belong to the right author).
//! - `run` (default) validates the optional `--station` slug and
//!   delegates to `nightride_tui::run`. An unknown slug short-circuits
//!   to the friendly station-list output instead of dying with a raw
//!   validation error.

use clap::builder::Resettable;
use clap::error::ErrorKind;
use clap::{CommandFactory, FromArgMatches};
use nightride_tui::cli::{self, CliArgs, Command};
use nightride_tui::station;
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    // Override the banner at runtime so terminals without truecolor
    // (or invocations piped through `less` / `NO_COLOR`) get the plain
    // ASCII version. We also reset `about` to `None` so clap renders
    // no description block between the banner and `Usage:` — the
    // banner's project tail already carries the version, repo URL,
    // and `---` divider. Help-flag text is replaced with a punchier
    // "Print this!!" so the auto-generated `-h, --help` row reads in
    // the project's voice.
    let mut cmd = CliArgs::command()
        .before_help(cli::select_banner())
        .about(Resettable::Reset);
    let matches = match cmd.clone().try_get_matches() {
        Ok(matches) => matches,
        Err(err) => {
            if matches!(
                err.kind(),
                ErrorKind::InvalidSubcommand | ErrorKind::UnknownArgument
            ) {
                println!();
                cmd.print_long_help().expect("write root help");
                println!();
                std::process::exit(2);
            }
            if matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                println!();
                err.print().expect("write clap display output");
                std::process::exit(0);
            }
            err.exit();
        }
    };
    let args = match CliArgs::from_arg_matches(&matches) {
        Ok(args) => args,
        Err(err) => err.exit(),
    };
    if let Some(help_target) = args.help.as_deref() {
        if help_target.is_empty() {
            println!();
            cmd.print_long_help().expect("write root help");
            println!();
            std::process::exit(0);
        }
        println!();
        if cli::print_command_help(help_target) {
            std::process::exit(0);
        }
        cmd.print_long_help().expect("write root help");
        println!();
        std::process::exit(2);
    }
    let exit = match cli::dispatch(&args) {
        Command::InstallTuiFont => {
            println!();
            match cli::install_tui_font() {
                Ok(()) => 0,
                Err(err) => {
                    eprintln!("install-tui-font failed: {err}");
                    1
                }
            }
        }
        Command::InstallNightrideFont => {
            println!();
            match cli::install_nightride_font() {
                Ok(()) => 0,
                Err(err) => {
                    eprintln!("install-nightride-font failed: {err}");
                    1
                }
            }
        }
        Command::Update => {
            println!();
            match cli::run_update() {
                Ok(()) => 0,
                Err(err) => {
                    eprintln!("[!] update failed: {err}");
                    1
                }
            }
        }
        Command::Run => {
            // Friendly fast-path for `-s <unknown>`: rather than
            // booting the full audio + UI pipeline only to fail at
            // config validation, surface the registry table so the
            // user can copy-paste a valid slug.
            if let Some(slug) = args.station.as_deref() {
                if slug.is_empty() || station::by_slug(slug).is_none() {
                    const BOLD: &str = "\x1b[1m";
                    const UNDER: &str = "\x1b[4m";
                    const RESET: &str = "\x1b[0m";
                    const STATION_USAGE_CMD: &str = "nightride-tui";
                    const STATION_USAGE_FLAGS: &str = "[-s, --station]";
                    const STATION_USAGE_ARG: &str = "<SLUG>";
                    eprintln!();
                    if slug.is_empty() {
                        eprintln!(
                            "{BOLD}{UNDER}Usage{RESET}: {BOLD}{STATION_USAGE_CMD}{RESET} {STATION_USAGE_FLAGS} {STATION_USAGE_ARG}"
                        );
                    } else {
                        eprintln!("nightride: unknown station `{slug}`.");
                        eprintln!(
                            "{BOLD}{UNDER}Usage{RESET}: {BOLD}{STATION_USAGE_CMD}{RESET} {STATION_USAGE_FLAGS} {STATION_USAGE_ARG}"
                        );
                    }
                    eprintln!();
                    cli::list_stations();
                    std::process::exit(2);
                }
            }
            let root = CancellationToken::new();
            install_panic_hook(root.clone());
            match nightride_tui::run(args, root).await {
                Ok(()) => 0,
                Err(err) => {
                    eprintln!("nightride: {err}");
                    1
                }
            }
        }
    };
    std::process::exit(exit);
}

fn install_panic_hook(root: CancellationToken) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Log + cancel before delegating to the default hook so the
        // tree unwinds cleanly even if a task panics deep inside an
        // async block.
        eprintln!("nightride panic: {info}");
        root.cancel();
        default(info);
    }));
}
