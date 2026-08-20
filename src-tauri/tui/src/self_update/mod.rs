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

use std::io::Write;
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

pub(crate) async fn run(show_intro: bool) -> Result<()> {
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
    if show_intro {
        let style = reporter.style();
        let version = format!("v{current}");
        if !crate::logo::play_interactive(style, &version).await? {
            print!("{}", crate::logo::wordmark(style, &version));
        }
    }

    let checking = i18n::t_update_checking();
    let found = progress::spin(&mut reporter, checking, update::check(current)).await;
    let found = match found {
        Ok(found) => found,
        Err(error) => {
            reporter.abort();
            return Err(error);
        }
    };
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
    let parent = target.parent().context("可执行文件没有父目录")?;
    let target_name = target.file_name().context("可执行文件路径缺少文件名")?;
    let prefix = format!(".{}.", target_name.to_string_lossy());
    let mut staged = tempfile::Builder::new()
        .prefix(&prefix)
        .suffix(".new")
        .tempfile_in(parent)
        .with_context(|| format!("无法在 {} 创建更新临时文件", parent.display()))?;
    staged
        .write_all(binary)
        .with_context(|| format!("写入 {} 失败", staged.path().display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        staged
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(windows)]
    {
        replace_windows(target, staged)
    }
    #[cfg(not(windows))]
    {
        staged
            .persist(target)
            .map(|_| ())
            .map_err(|error| error.error)
            .with_context(|| format!("替换 {} 失败", target.display()))
    }
}

#[cfg(any(windows, test))]
fn rollback_windows_replace(
    target: &Path,
    parked: &Path,
    replace_error: std::io::Error,
    rollback: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    match rollback(parked, target) {
        Ok(()) => Err(replace_error).context("替换可执行文件失败，旧版已恢复"),
        Err(rollback_error) => Err(anyhow::anyhow!(
            "替换可执行文件失败：{replace_error}；恢复旧版也失败：{rollback_error}；旧版位于 {}",
            parked.display()
        )),
    }
}

/// Built everywhere under `cfg(test)` on purpose: the body is plain `std::fs`,
/// and a `cfg(windows)`-only helper is invisible to clippy and the test suite
/// on every other platform. v0.9.3 shipped a `needless_return` here that six
/// green local gates could not see.
#[cfg(any(windows, test))]
fn replace_windows(target: &Path, staged: tempfile::NamedTempFile) -> Result<()> {
    let parked = target.with_extension("old.exe");
    match std::fs::remove_file(&parked) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("清理上一个旧版可执行文件失败"),
    }
    std::fs::rename(target, &parked).context("移开旧版可执行文件失败")?;
    match staged.persist(target) {
        Ok(_) => Ok(()),
        Err(error) => rollback_windows_replace(target, &parked, error.error, |from, to| {
            std::fs::rename(from, to)
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_windows_swap_parks_the_old_binary_before_installing_the_new_one() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("ypm.exe");
        std::fs::write(&target, b"old").unwrap();
        let mut staged = tempfile::Builder::new()
            .tempfile_in(directory.path())
            .unwrap();
        std::io::Write::write_all(&mut staged, b"new").unwrap();

        replace_windows(&target, staged).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        let parked = target.with_extension("old.exe");
        assert_eq!(std::fs::read(parked).unwrap(), b"old");
    }

    #[test]
    #[cfg(unix)]
    fn predictable_symlink_cannot_capture_the_update_bytes() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("ypm");
        let victim = directory.path().join("victim");
        let predictable = directory.path().join("ypm.new");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&victim, b"keep me").unwrap();
        symlink(&victim, &predictable).unwrap();

        swap_in(&target, b"new binary").unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"new binary");
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep me");
        assert_eq!(std::fs::read_link(&predictable).unwrap(), victim);
    }

    #[test]
    fn windows_replacement_rolls_back_when_installing_the_staged_file_fails() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("ypm.exe");
        let staged = directory.path().join("staged.exe");
        let parked = directory.path().join("ypm.old.exe");
        std::fs::write(&target, b"old binary").unwrap();
        std::fs::write(&staged, b"new binary").unwrap();
        std::fs::rename(&target, &parked).unwrap();

        let error = rollback_windows_replace(
            &target,
            &parked,
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "locked"),
            |from, to| std::fs::rename(from, to),
        )
        .unwrap_err();

        assert!(error.to_string().contains("旧版已恢复"));
        assert_eq!(std::fs::read(&target).unwrap(), b"old binary");
        assert_eq!(std::fs::read(&staged).unwrap(), b"new binary");
        assert!(!parked.exists());
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
