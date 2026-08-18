//! Upgrading a Homebrew keg.
//!
//! ypm never replaces a binary brew holds a receipt for — that desyncs
//! `brew list --versions` and the next `brew upgrade` reverts it. It drives
//! brew instead, which is the pattern codex CLI ships. [`refresh`] exists
//! because codex shipped it *without* one and collected stale-index reports
//! (openai/codex#6253): `brew upgrade` only auto-updates once a day, so a
//! release published an hour ago is invisible without an explicit sync.

use anyhow::{bail, Context, Result};

const FORMULA: &str = "ypm";
/// Enough of a failure to diagnose it, short enough for a status bar.
const ERROR_LINES: usize = 8;

/// `brew update` — syncs the tap so the next upgrade sees today's formula.
pub(crate) async fn refresh() -> Result<()> {
    run(&["update"]).await
}

/// `brew upgrade ypm` — auto-update is off here because [`refresh`] just ran.
pub(crate) async fn upgrade() -> Result<()> {
    run(&["upgrade", FORMULA]).await
}

async fn run(args: &[&str]) -> Result<()> {
    let output = tokio::process::Command::new("brew")
        .args(args)
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("HOMEBREW_NO_COLOR", "1")
        .env("HOMEBREW_NO_ENV_HINTS", "1")
        .output()
        .await
        .context(crate::i18n::t_update_brew_missing())?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "brew {}: {}",
        args.join(" "),
        tail(&output.stderr, &output.stdout)
    )
}

/// brew reports the useful part last, and prints some failures on stdout.
fn tail(stderr: &[u8], stdout: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let stdout = String::from_utf8_lossy(stdout);
    let source = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    let lines: Vec<&str> = source
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect();
    let start = lines.len().saturating_sub(ERROR_LINES);
    lines[start..].join(" / ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_report_the_last_lines_and_prefer_stderr() {
        let noise: String = (1..=12).map(|n| format!("line {n}\n")).collect();
        let summary = tail(noise.as_bytes(), b"stdout instead");
        assert!(summary.starts_with("line 5"), "{summary}");
        assert!(summary.ends_with("line 12"), "{summary}");
        assert_eq!(summary.split(" / ").count(), ERROR_LINES);
    }

    #[test]
    fn an_empty_stderr_falls_back_to_stdout() {
        assert_eq!(
            tail(b"   \n", b"Error: No available formula"),
            "Error: No available formula"
        );
    }
}
