//! ypm — YesPlayMusic in the terminal.

mod action;
mod api;
mod app;
mod config;
mod cover_cache;
mod event;
mod i18n;
mod icons;
mod lyrics;
mod nerd_font;
pub mod pixel;
mod player;
mod spectrum;
mod store;
mod terminal_background;
mod theme;
mod ui;
mod update;
mod yrc;

use anyhow::Result;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "ypm", version, about = "YesPlayMusic 终端版")]
struct Args {
    /// Write a debug log to the state directory (never to the screen).
    #[arg(long)]
    debug: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
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
