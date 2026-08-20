//! Release check against GitHub. The background caller discards errors while
//! explicit update commands report them. Homebrew installs upgrade through
//! brew; standalone installs download from the Releases page.

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

const RELEASES_URL: &str = "https://api.github.com/repos/nagi-studio/YesPlayMusic/releases";
/// The tap is bumped by hand after a release, so it lags GitHub by anywhere
/// from minutes to days. Announcing a tag `brew upgrade` cannot deliver yet
/// is exactly how gh (cli/cli#6949) and codex (openai/codex#6436) collected
/// "already installed" bug reports — a keg asks the formula, not the tag.
const TAP_FORMULA_URL: &str =
    "https://raw.githubusercontent.com/nagi-studio/homebrew-ypm/HEAD/Formula/ypm.rb";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}

/// `x.y.z` plus an optional canary number; a stable build outranks every
/// canary of the same triple.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    stable: bool,
    canary: u64,
}

fn parse_version(raw: &str) -> Option<Version> {
    let raw = raw.trim().trim_start_matches('v');
    let (triple, pre) = match raw.split_once('-') {
        Some((triple, pre)) => (triple, Some(pre)),
        None => (raw, None),
    };
    let mut parts = triple.splitn(3, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    let canary = match pre {
        None => 0,
        Some(pre) => pre.strip_prefix("canary.")?.parse().ok()?,
    };
    Some(Version {
        major,
        minor,
        patch,
        stable: pre.is_none(),
        canary,
    })
}

/// The newest visible tag that outranks `current`. Canary builds see
/// prereleases; stable builds only ever hear about stable releases.
fn newer_release(current: &str, releases: &[Release]) -> Result<Option<String>> {
    let current_version = parse_version(current)
        .with_context(|| format!("unsupported current ypm version `{current}`"))?;
    let mut newest = None;
    for release in releases
        .iter()
        .filter(|release| !release.draft && (!release.prerelease || !current_version.stable))
    {
        let version = parse_version(&release.tag_name)
            .with_context(|| format!("unsupported release tag `{}`", release.tag_name))?;
        if version > current_version
            && newest
                .as_ref()
                .is_none_or(|(candidate, _): &(Version, String)| version > *candidate)
        {
            newest = Some((version, release.tag_name.clone()));
        }
    }
    Ok(newest.map(|(_, tag)| tag))
}

/// Stable builds ask the dedicated `latest` endpoint (prereleases can crowd
/// a page of the plain list and hide the newest stable); canary builds scan
/// the recent list. Callers decide whether a failed check is user-visible.
pub(crate) async fn check(current: &str) -> Result<Option<String>> {
    let current_version = parse_version(current)
        .with_context(|| format!("unsupported current ypm version `{current}`"))?;
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("ypm/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("cannot initialize the update HTTP client")?;
    if installed_via_brew() {
        let formula = fetch(&client, TAP_FORMULA_URL).await?;
        let published =
            formula_version(&formula).context("Homebrew formula does not declare a version")?;
        let version = parse_version(&published)
            .with_context(|| format!("unsupported Homebrew formula version `{published}`"))?;
        return Ok((version > current_version).then(|| format!("v{published}")));
    }
    if current_version.stable {
        let body = fetch(&client, &format!("{RELEASES_URL}/latest")).await?;
        let release: Release = serde_json::from_str(&body)
            .context("GitHub returned an invalid latest-release response")?;
        let version = parse_version(&release.tag_name)
            .with_context(|| format!("unsupported release tag `{}`", release.tag_name))?;
        Ok((version > current_version).then_some(release.tag_name))
    } else {
        let body = fetch(&client, &format!("{RELEASES_URL}?per_page=15")).await?;
        let releases: Vec<Release> =
            serde_json::from_str(&body).context("GitHub returned an invalid releases response")?;
        newer_release(current, &releases)
    }
}

async fn fetch(client: &reqwest::Client, url: &str) -> Result<String> {
    client
        .get(url)
        .send()
        .await
        .with_context(|| format!("update request failed: {url}"))?
        .error_for_status()
        .with_context(|| format!("update server rejected the request: {url}"))?
        .text()
        .await
        .with_context(|| format!("cannot read update response: {url}"))
}

/// Pulls `version "x.y.z"` out of a formula. The template ships an all-zero
/// placeholder that a real release replaces, and 0.0.0 outranks nothing, so
/// an unreleased tap simply reports no update.
fn formula_version(formula: &str) -> Option<String> {
    formula.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("version ")?;
        let quoted = rest.trim().strip_prefix('"')?;
        let (version, _) = quoted.split_once('"')?;
        Some(version.to_owned())
    })
}

/// Homebrew keg installs live under a Cellar path; that decides whether
/// the hint says `brew upgrade ypm` or points at the Releases page.
pub(crate) fn installed_via_brew() -> bool {
    std::env::current_exe()
        .map(|path| path.to_string_lossy().contains("/Cellar/"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, prerelease: bool) -> Release {
        Release {
            tag_name: tag.into(),
            draft: false,
            prerelease,
        }
    }

    #[test]
    fn stable_outranks_canary_and_ordering_is_semver_like() {
        assert!(parse_version("0.8.0").unwrap() > parse_version("0.8.0-canary.9").unwrap());
        assert!(parse_version("0.8.1-canary.1").unwrap() > parse_version("0.8.0").unwrap());
        assert!(parse_version("v0.10.0").unwrap() > parse_version("0.9.9").unwrap());
        assert!(
            parse_version("0.8.0-canary.3").unwrap() > parse_version("0.8.0-canary.2").unwrap()
        );
        assert_eq!(parse_version("0.8.0-rc.1"), None);
    }

    #[test]
    fn the_tap_formula_yields_the_version_brew_would_install() {
        let formula = "class Ypm < Formula\n  desc \"x\"\n\n  version \"0.9.1\"\n\n  on_macos do\n    url \"https://example/v0.9.1/ypm\"\n  end\nend\n";
        assert_eq!(formula_version(formula).as_deref(), Some("0.9.1"));
        // The unreleased template placeholder outranks nothing, so a tap that
        // has never been bumped simply reports no update.
        let template = "  version \"0.0.0\"\n";
        let placeholder = parse_version(&formula_version(template).unwrap()).unwrap();
        assert!(parse_version("0.8.0").unwrap() > placeholder);
        assert_eq!(formula_version("class Ypm < Formula\nend"), None);
    }

    #[test]
    fn canary_builds_see_prereleases_but_stable_builds_do_not() {
        let releases = vec![
            release("v0.8.0-canary.3", true),
            release("v0.7.0", false),
            Release {
                tag_name: "v0.9.0".into(),
                draft: true,
                prerelease: false,
            },
        ];
        assert_eq!(
            newer_release("0.8.0-canary.2", &releases)
                .unwrap()
                .as_deref(),
            Some("v0.8.0-canary.3")
        );
        // The stable build ignores the canary and the draft outright.
        assert_eq!(
            newer_release("0.6.0", &releases).unwrap().as_deref(),
            Some("v0.7.0")
        );
        assert_eq!(newer_release("0.7.0", &releases).unwrap(), None);
    }

    #[test]
    fn malformed_versions_fail_instead_of_looking_up_to_date() {
        let releases = vec![release("v0.9.3-rc.1", true)];
        assert!(newer_release("not-a-version", &[]).is_err());
        assert!(newer_release("0.9.2-canary.1", &releases).is_err());
    }
}
