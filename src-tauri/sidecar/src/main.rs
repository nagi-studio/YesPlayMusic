#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{env, io, process::ExitCode};

use tokio::io::BufReader;
use tracing_subscriber::EnvFilter;
use yesplaymusic_sidecar::{
    config::SidecarConfig,
    health::{read_health_token, HealthState},
    server,
    unm::run_hermetic_executor_smoke,
};

const RUST_SIDECAR_MARKER: &str = "YPM_RUST_SIDECAR_V1";

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("warn,yesplaymusic_sidecar=info"))
        .with_target(false)
        .without_time()
        .try_init();
}

async fn unm_smoke_test() -> Result<(), String> {
    let sources = run_hermetic_executor_smoke().await?;
    println!(
        "[sidecar][UNM] Rust providers ready: {}",
        sources.join(", ")
    );
    println!("[sidecar][UNM] production executor search->retrieve passed");
    Ok(())
}

async fn execute() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.as_slice() == ["--identity"] {
        println!("{RUST_SIDECAR_MARKER}");
        return Ok(());
    }
    if args.as_slice() == ["--unm-smoke-test"] {
        unm_smoke_test().await.map_err(io::Error::other)?;
        return Ok(());
    }
    let config = SidecarConfig::parse(&args)?;
    let mut input = BufReader::new(tokio::io::stdin());
    let token = read_health_token(&mut input).await?;
    let health = HealthState::new(token).map_err(std::io::Error::other)?;
    server::run(config, health, input).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    match execute().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[sidecar] {error}");
            ExitCode::FAILURE
        }
    }
}
