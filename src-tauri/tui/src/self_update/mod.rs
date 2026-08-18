//! `ypm update`: move this install to the newest release it can reach.
//!
//! Two shapes, picked by how ypm got here. A standalone binary follows the
//! rustup/bun path: verify a Minisign signature before anything touches
//! disk, then swap atomically (same-directory temp file + rename; Windows
//! renames the old exe away first because a running exe cannot be
//! overwritten in place). A Homebrew keg is never touched directly —
//! [`brew`] drives brew itself, so brew's receipts stay true.
//!
//! [`install`] is the whole standalone pipeline behind a stage callback, so
//! the CLI can render it as an animated progress line and the TUI can drive
//! the same bytes through its status bar.

mod brew;
mod progress;

pub(crate) use brew::{refresh as brew_refresh, upgrade as brew_upgrade};

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::{i18n, update};

const DOWNLOAD_BASE: &str = "https://github.com/nagi-studio/YesPlayMusic/releases/download";
/// Injected by CI from the repo's updater key infrastructure; a dev build
/// without it refuses to update rather than skipping verification.
const UPDATER_PUBKEY: Option<&str> = option_env!("TAURI_UPDATER_PUBKEY");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    Downloading { done: u64, total: Option<u64> },
    Verifying,
    Installing,
}

fn asset_name() -> Result<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Ok("ypm-macos-aarch64")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Ok("ypm-linux-x64")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Ok("ypm-windows-x64.exe")
    } else {
        bail!(i18n::t_update_no_prebuilt())
    }
}

/// Everything that can refuse an update before a byte is fetched. Returns the
/// embedded public key so the caller cannot reach the download without one.
pub(crate) fn preflight() -> Result<&'static str> {
    if update::installed_via_brew() {
        bail!(i18n::t_update_use_brew());
    }
    let Some(pubkey) = UPDATER_PUBKEY else {
        bail!(i18n::t_update_no_pubkey());
    };
    asset_name()?;
    Ok(pubkey)
}

/// Downloads, verifies and swaps in `tag`, reporting each stage. Returns the
/// path that now holds the new binary.
pub(crate) async fn install(tag: &str, on_stage: &mut impl FnMut(Stage)) -> Result<PathBuf> {
    let pubkey = preflight()?;
    let asset = asset_name()?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .user_agent(concat!("ypm/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let binary = download(
        &client,
        &format!("{DOWNLOAD_BASE}/{tag}/{asset}"),
        |done, total| {
            on_stage(Stage::Downloading { done, total });
        },
    )
    .await?;

    on_stage(Stage::Verifying);
    let signature = fetch_bytes(&client, &format!("{DOWNLOAD_BASE}/{tag}/{asset}.sig")).await?;
    verify(pubkey, &binary, &signature)?;

    on_stage(Stage::Installing);
    let target = std::env::current_exe().context("cannot locate the running binary")?;
    swap_in(&target, &binary)?;
    Ok(target)
}

pub(crate) async fn run() -> Result<()> {
    // A keg skips preflight entirely: the signature and asset checks guard
    // the binary swap, and a keg never reaches one.
    let brew = update::installed_via_brew();
    if !brew {
        preflight()?;
    }
    let current = env!("CARGO_PKG_VERSION");
    let mut reporter = progress::Reporter::new();

    // The intro owns the screen, so anything printed before it would be
    // wiped; a terminal too small for the mark gets the wordmark instead.
    let style = reporter.style();
    let version = format!("v{current}");
    if !crate::logo::play(style, &version, || false).await {
        print!("{}", crate::logo::wordmark(style, &version));
    }

    let checking = i18n::t_update_checking();
    let found = progress::spin(&mut reporter, checking, update::check(current)).await;
    let Some(tag) = found else {
        reporter.abort();
        reporter.mark(i18n::t_update_up_to_date(), &format!("v{current}"));
        return Ok(());
    };
    reporter.abort();
    reporter.header(&format!("v{current}"), &tag);
    if brew {
        // Each step gets its own spinner: `brew update` alone can sit silent
        // for half a minute, and a frozen line reads as a hang.
        let outcome = async {
            progress::spin(
                &mut reporter,
                i18n::t_update_brew_refreshing(),
                brew::refresh(),
            )
            .await?;
            progress::spin(
                &mut reporter,
                i18n::t_update_brew_upgrading(),
                brew::upgrade(),
            )
            .await
        }
        .await;
        return match outcome {
            Ok(()) => {
                reporter.settle();
                reporter.mark(i18n::t_update_brew_installed(), &tag);
                reporter.tail(&i18n::t_update_restart(&tag));
                Ok(())
            }
            Err(error) => {
                reporter.abort();
                Err(error)
            }
        };
    }

    let asset = asset_name()?;
    let downloading = i18n::t_update_downloading(asset);
    let verifying = i18n::t_update_verifying();
    let installing = i18n::t_update_installing();
    let outcome = install(&tag, &mut |stage| match stage {
        Stage::Downloading { done, total } => reporter.phase(&downloading, Some((done, total))),
        Stage::Verifying => reporter.phase(verifying, None),
        Stage::Installing => reporter.phase(installing, None),
    })
    .await;

    match outcome {
        Ok(target) => {
            // The install stage is instantaneous; its checkmark carries the
            // path instead, which is the line worth keeping on screen.
            reporter.abort();
            reporter.mark(i18n::t_update_installed(), &target.display().to_string());
            reporter.tail(&i18n::t_update_restart(&tag));
            Ok(())
        }
        Err(error) => {
            reporter.abort();
            Err(error)
        }
    }
}

/// Streams the body so the caller can draw a real percentage; GitHub sends a
/// Content-Length, but a missing one only costs the determinate bar.
async fn download(
    client: &reqwest::Client,
    url: &str,
    mut on_bytes: impl FnMut(u64, Option<u64>),
) -> Result<Vec<u8>> {
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| i18n::t_update_download_failed(url))?;
    let total = response.content_length();
    let mut body = Vec::with_capacity(total.unwrap_or(0) as usize);
    on_bytes(0, total);
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| i18n::t_update_download_failed(url))?
    {
        body.extend_from_slice(&chunk);
        on_bytes(body.len() as u64, total);
    }
    Ok(body)
}

async fn fetch_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| i18n::t_update_download_failed(url))?;
    Ok(response.bytes().await?.to_vec())
}

/// Tauri's signer emits base64-wrapped Minisign artifacts; nothing is
/// written to disk unless the signature matches.
fn verify(pubkey: &str, binary: &[u8], signature: &[u8]) -> Result<()> {
    use base64::Engine;
    let engine = base64::engine::general_purpose::STANDARD;
    let decoded_key = engine
        .decode(pubkey.trim())
        .context("更新公钥不是有效的 base64")?;
    let key_text = String::from_utf8(decoded_key).context("更新公钥内容无效")?;
    let public_key = minisign_verify::PublicKey::decode(&key_text).context("更新公钥格式无效")?;
    let decoded_sig = engine
        .decode(String::from_utf8_lossy(signature).trim())
        .context("签名不是有效的 base64")?;
    let sig_text = String::from_utf8(decoded_sig).context("签名内容无效")?;
    let signature = minisign_verify::Signature::decode(&sig_text).context("签名格式无效")?;
    public_key
        .verify(binary, &signature, false)
        .context("签名校验失败，二进制未被替换")
}

fn swap_in(target: &Path, binary: &[u8]) -> Result<()> {
    let staged = staged_path(target);
    std::fs::write(&staged, binary).with_context(|| format!("写入 {} 失败", staged.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(windows)]
    {
        // A running exe cannot be replaced, but it can be renamed away.
        let parked = target.with_extension("old.exe");
        let _ = std::fs::remove_file(&parked);
        std::fs::rename(target, &parked).context("移开旧版可执行文件失败")?;
    }
    std::fs::rename(&staged, target).with_context(|| format!("替换 {} 失败", target.display()))
}

/// Same directory as the target so the final rename never crosses devices.
fn staged_path(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| "ypm".into());
    name.push(".new");
    target.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_stays_next_to_the_target_binary() {
        let staged = staged_path(Path::new("/home/user/.local/bin/ypm"));
        assert_eq!(staged, Path::new("/home/user/.local/bin/ypm.new"));
    }

    #[test]
    fn garbage_signatures_never_pass() {
        // A syntactically valid minisign pubkey (generated for this test's
        // shape check only) against a garbage signature must error, not
        // panic — the swap must stay unreachable.
        let pubkey = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            "untrusted comment: test\nRWTg1dcEjWG4mSkya8w0jVAtaYDBGzB3jvHcNi6vLZbTIx3jerf1DVjK\n",
        );
        assert!(verify(&pubkey, b"binary", b"not-base64!").is_err());
        let bogus_sig = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            "untrusted comment: sig\nRUTg1dcEjWG4mSm\n",
        );
        assert!(verify(&pubkey, b"binary", bogus_sig.as_bytes()).is_err());
    }
}
