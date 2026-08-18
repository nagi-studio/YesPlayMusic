//! ypm — YesPlayMusic in the terminal.

mod action;
mod api;
mod app;
mod config;
mod cover_cache;
#[cfg(unix)]
mod ctl;
mod event;
mod i18n;
mod icons;
mod logo;
mod lyrics;
mod nerd_font;
pub mod pixel;
mod player;
#[cfg(unix)]
mod remote;
mod self_update;
mod spectrum;
mod store;
mod term;
mod terminal_background;
mod theme;
mod ui;
mod update;
mod yrc;

use anyhow::Result;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "ypm", version, about = "YesPlayMusic TUI")]
struct Args {
    /// Write a debug log to the state directory (never to the screen).
    #[arg(long)]
    debug: bool,
    /// Only talk to the desktop app.
    #[arg(long, global = true, conflicts_with = "tui")]
    gui: bool,
    /// Only talk to a running terminal player.
    #[arg(long, global = true)]
    tui: bool,
    /// Print results as JSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Ctl>,
}

/// Control a running player (GUI or TUI) without opening the interface.
#[derive(clap::Subcommand, Debug, Clone, Copy)]
enum Ctl {
    /// Show the current track.
    Status,
    /// Pause playback.
    Pause,
    /// Resume playback.
    Resume,
    /// Toggle between play and pause.
    Toggle,
    /// Skip to the next track.
    Next,
    /// Go back to the previous track.
    Prev,
    /// Update ypm itself to the newest release.
    Update,
}

#[cfg(unix)]
impl From<Ctl> for remote::Command {
    fn from(command: Ctl) -> Self {
        match command {
            Ctl::Status => Self::Status,
            Ctl::Pause => Self::Pause,
            Ctl::Resume => Self::Resume,
            Ctl::Toggle => Self::Toggle,
            Ctl::Next => Self::Next,
            Ctl::Prev => Self::Prev,
            // Handled in main() before this conversion runs.
            Ctl::Update => unreachable!("update is not a remote command"),
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    if let Some(Ctl::Update) = args.command {
        // The updater prints a whole screen of its own, so it follows the
        // configured language just like the TUI does.
        let config = config::Config::load_with_metadata();
        i18n::init(i18n::Lang::from_config(&config.config.language));
        let runtime = tokio::runtime::Runtime::new()?;
        return runtime.block_on(self_update::run());
    }
    if let Some(command) = args.command {
        #[cfg(unix)]
        {
            let forced = if args.gui {
                Some(ctl::Target::Gui)
            } else if args.tui {
                Some(ctl::Target::Tui)
            } else {
                None
            };
            let runtime = tokio::runtime::Runtime::new()?;
            return runtime.block_on(ctl::run(command.into(), forced, args.json));
        }
        #[cfg(not(unix))]
        {
            let _ = command;
            anyhow::bail!("远程控制命令暂不支持此平台");
        }
    }
    let _log_guard = init_logging(args.debug)?;

    let config = config::Config::load_with_metadata();
    i18n::init(i18n::Lang::from_config(&config.config.language));
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(app::run(config))
}

/// Logs go to a file: stdout/stderr belong to the TUI.
fn init_logging(debug: bool) -> Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    if !debug {
        return Ok(None);
    }
    let dir = config::state_dir();
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::File::create(dir.join("ypm.log"))?;
    let (writer, guard) = tracing_appender::non_blocking(file);
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,ypm=debug".into()),
        )
        .init();
    Ok(Some(guard))
}
