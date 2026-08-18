//! `ypm update`: replace the running binary with the newest release.
//!
//! Follows the rustup/bun shape: refuse package-manager installs, verify a
//! Minisign signature before anything touches disk, then swap atomically
//! (same-directory temp file + rename; Windows renames the old exe away
//! first because a running exe cannot be overwritten in place).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::update;

const DOWNLOAD_BASE: &str = "https://github.com/nagi-studio/YesPlayMusic/releases/download";
/// Injected by CI from the repo's updater key infrastructure; a dev build
/// without it refuses to update rather than skipping verification.
const UPDATER_PUBKEY: Option<&str> = option_env!("TAURI_UPDATER_PUBKEY");

fn asset_name() -> Result<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Ok("ypm-macos-aarch64")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Ok("ypm-linux-x64")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Ok("ypm-windows-x64.exe")
    } else {
        bail!("此平台没有预构建的 ypm，可从源码 cargo build");
    }
}

pub(crate) async fn run() -> Result<()> {
    if update::installed_via_brew() {
        bail!("这个 ypm 由 Homebrew 管理，请运行：brew upgrade ypm");
    }
    let Some(pubkey) = UPDATER_PUBKEY else {
        bail!("此构建未内嵌更新公钥（本地开发版），请重新构建或从 Releases 下载");
    };
    let current = env!("CARGO_PKG_VERSION");
    let Some(tag) = update::check(current).await else {
        println!("已是最新版本（v{current}）");
        return Ok(());
    };
    let asset = asset_name()?;
    println!("发现新版本 {tag}，下载 {asset} …");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .user_agent(concat!("ypm/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let binary = fetch_bytes(&client, &format!("{DOWNLOAD_BASE}/{tag}/{asset}")).await?;
    let signature = fetch_bytes(&client, &format!("{DOWNLOAD_BASE}/{tag}/{asset}.sig")).await?;
    verify(pubkey, &binary, &signature)?;

    let target = std::env::current_exe().context("cannot locate the running binary")?;
    swap_in(&target, &binary)?;
    println!("已更新到 {tag}（{}）", target.display());
    Ok(())
}

async fn fetch_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("下载失败：{url}"))?;
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
