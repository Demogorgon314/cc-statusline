//! Claude Code status line: stdin JSON, local transcript accounting and an interactive configurator.

mod appearance;
mod collect;
mod config;
mod install;
mod paths;
mod payload;
mod probe;
mod quota;
mod render;
mod session;
mod subagent;
mod themes;
mod tps;
mod tui;
mod update;

use clap::{Parser, Subcommand};
use config::Config;
use std::io::IsTerminal;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(
    name = "cc-statusline",
    version,
    about = "High-performance status line for Claude Code"
)]
struct Cli {
    /// Render with a theme instead of the saved config
    #[arg(short, long, global = true)]
    theme: Option<String>,
    /// Render width for status line mode (default: detect the terminal)
    #[arg(long, global = true)]
    width: Option<usize>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Interactive configurator (running with no arguments opens the menu)
    Config,
    /// Set this binary as statusLine.command in settings.json
    Install {
        /// Command to write instead of this binary's path
        #[arg(long)]
        command: Option<String>,
        /// Replace an existing non-cc-statusline command
        #[arg(long)]
        force: bool,
        /// Install the per-agent panel renderer instead of the main status line
        #[arg(long)]
        subagents: bool,
    },
    /// Remove our statusLine from settings.json
    Uninstall {
        /// Remove only our per-agent panel renderer
        #[arg(long)]
        subagents: bool,
    },
    /// Render subagentStatusLine JSON rows from a tasks array on stdin
    Subagents,
    /// Write the config file from a theme (default: claude)
    Init {
        #[arg(long)]
        force: bool,
    },
    /// List themes
    Themes,
    /// Fetch plan quota now and print it (5h / 7d)
    Quota,
    /// Check GitHub for a newer release and install it
    Update {
        /// Only report whether an update is available
        #[arg(long)]
        check: bool,
    },
    /// Background quota refresh, spawned by the status line
    #[command(hide = true)]
    FetchQuota,
    /// Render for the last observed session in a directory, without the TUI
    Preview {
        /// Working directory of the session (default: current dir)
        #[arg(long)]
        cwd: Option<String>,
        /// Session id (default: last observed session for --cwd)
        #[arg(long)]
        session: Option<String>,
        /// Render width (default: full line)
        #[arg(long)]
        width: Option<usize>,
    },
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Some(Cmd::Config) => tui::run_configurator(),
        Some(Cmd::Install {
            command,
            force,
            subagents,
        }) => {
            if subagents {
                install::install_subagents(command, force).map(|o| {
                    println!(
                        "subagentStatusLine.command = {:?}{}",
                        o.command,
                        if o.changed {
                            ""
                        } else {
                            " (already installed)"
                        }
                    );
                })
            } else {
                install_cmd(command, force)
            }
        }
        Some(Cmd::Uninstall { subagents }) => (if subagents {
            install::uninstall_subagents()
        } else {
            install::uninstall()
        })
        .map(|changed| {
            if changed {
                println!("Removed cc-statusline from settings.json; restart Claude Code to apply.");
            } else {
                println!("The selected status line is not cc-statusline; nothing to do.");
            }
        }),
        Some(Cmd::Subagents) => {
            let bytes = payload::read_bytes(Duration::from_millis(150));
            for row in subagent::render_rows(&bytes, &load_config(cli.theme.as_deref()), cli.width)
            {
                println!("{row}");
            }
            Ok(())
        }
        Some(Cmd::Init { force }) => init(cli.theme.as_deref(), force),
        Some(Cmd::Themes) => {
            for name in themes::list() {
                let desc = themes::BUILTIN
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map_or("saved theme", |(_, d)| d);
                println!("{name:24} {desc}");
            }
            Ok(())
        }
        Some(Cmd::Quota) => quota::fetch_and_store()
            .map(|q| {
                let show = |name: &str, e: &Option<quota::Entry>| match e {
                    Some(e) => println!(
                        "{name:8} {:>5.1}%   resets {}",
                        e.used_ratio * 100.0,
                        e.reset_at.as_deref().unwrap_or("-")
                    ),
                    None => println!("{name:8} -"),
                };
                show("5h", &q.limit_5h);
                show("7d", &q.limit_7d);
                show("spend", &q.spend);
            })
            .map_err(|e| match quota::last_error() {
                Some(last) if last != e => format!("{e} (previous: {last})"),
                _ => e,
            }),
        Some(Cmd::Update { check }) => update_cmd(check),
        Some(Cmd::FetchQuota) => {
            let _ = quota::fetch_and_store();
            Ok(())
        }
        Some(Cmd::Preview {
            cwd,
            session,
            width,
        }) => {
            let cwd = cwd.unwrap_or_else(current_dir);
            let payload = collect::sample_payload(&cwd, session);
            let ctx = collect::collect(payload, load_config(cli.theme.as_deref()), Instant::now());
            println!("{}", render::render(&ctx, width.or(cli.width)));
            Ok(())
        }
        None if std::io::stdin().is_terminal() => tui::run_menu(),
        None => {
            run_statusline(cli.theme.as_deref(), cli.width);
            Ok(())
        }
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn update_cmd(check_only: bool) -> Result<(), String> {
    let latest = update::check_now()?;
    if !update::is_newer(&latest, update::CURRENT) {
        println!("cc-statusline {} is up to date", update::CURRENT);
        return Ok(());
    }
    println!("Update available: {} → {latest}", update::CURRENT);
    if check_only {
        return Ok(());
    }
    let kind = update::install_kind();
    if let Some(how) = update::manual_instructions(&kind) {
        println!("This copy was {how}");
        return Ok(());
    }
    let update::InstallKind::Standalone(exe) = kind else {
        unreachable!()
    };
    let v = update::install(&latest, &exe)?;
    println!("Updated {} to {v} (checksum verified)", exe.display());
    println!("The status line uses it on its next refresh.");
    Ok(())
}

fn current_dir() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn load_config(theme: Option<&str>) -> Config {
    match theme {
        Some(t) => themes::get(t),
        None => Config::load(),
    }
}

fn install_cmd(command: Option<String>, force: bool) -> Result<(), String> {
    match install::install(command, force) {
        Ok(o) => {
            if o.changed {
                println!("Installed: statusLine.command = {:?}", o.command);
                println!("Restart Claude Code to apply.");
            } else {
                println!("Already installed.");
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn init(theme: Option<&str>, force: bool) -> Result<(), String> {
    let path = config::config_path();
    if path.exists() && !force {
        println!(
            "{} already exists (use --force to overwrite)",
            path.display()
        );
        return Ok(());
    }
    themes::get(theme.unwrap_or("claude")).save()?;
    println!("Wrote {}", path.display());
    Ok(())
}

/// Skip optional probes once the foreground rendering budget is exhausted.
const BUDGET: Duration = Duration::from_millis(200);

fn run_statusline(theme: Option<&str>, width_flag: Option<usize>) {
    let started = Instant::now();
    let payload = payload::read_stdin(Duration::from_millis(150));
    // stderr is discarded by Claude Code; keep the panic where the fallback
    // line points (written even without the debug flag)
    std::panic::set_hook(Box::new(|info| paths::log(&format!("panic: {info}"))));
    // any panic still prints a line: a nonzero exit would make the TUI
    // freeze on the previous output with no hint of what went wrong
    let line = std::panic::catch_unwind(|| {
        let config = load_config(theme);
        let width = if let Some(w) = width_flag {
            Some(w)
        } else if config.style.width > 0 {
            Some(config.style.width)
        } else if started.elapsed() < BUDGET {
            // the footer sits inside a one-column gutter on each side
            probe::terminal_width().map(|w| w.saturating_sub(2).max(1))
        } else {
            None
        };
        let ctx = collect::collect(payload, config, started);
        render::render(&ctx, width)
    })
    .unwrap_or_else(|_| "cc-statusline: error (see cc-statusline-debug.log)".into());
    println!("{line}");
    paths::debug(&format!("done in {}ms", started.elapsed().as_millis()));
    // after printing: housekeeping never delays the line
    let _ = std::panic::catch_unwind(paths::sweep_cache);
}
