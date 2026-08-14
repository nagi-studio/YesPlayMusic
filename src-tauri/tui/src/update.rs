//! Silent release check against GitHub. Never blocks startup, never
//! self-updates: brew installs upgrade through brew, manual installs
//! download from the Releases page.

use std::time::Duration;

use serde::Deserialize;

const RELEASES_URL: &str = "https://api.github.com/repos/nagi-studio/YesPlayMusic/releases";
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
fn newer_release(current: &str, releases: &[Release]) -> Option<String> {
    let current_version = parse_version(current)?;
    releases
        .iter()
        .filter(|release| !release.draft && (!release.prerelease || !current_version.stable))
        .filter_map(|release| {
            let version = parse_version(&release.tag_name)?;
            (version > current_version).then(|| (version, release.tag_name.clone()))
        })
        .max()
        .map(|(_, tag)| tag)
}

/// One quiet request; any failure means "no news". Stable builds ask the
/// dedicated `latest` endpoint (prereleases can crowd a page of the plain
/// list and hide the newest stable); canary builds scan the recent list.
pub(crate) async fn check(current: &'static str) -> Option<String> {
    let current_version = parse_version(current)?;
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("ypm/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    if current_version.stable {
        let body = fetch(&client, &format!("{RELEASES_URL}/latest")).await?;
        let release: Release = serde_json::from_str(&body).ok()?;
        let version = parse_version(&release.tag_name)?;
        (version > current_version).then_some(release.tag_name)
    } else {
        let body = fetch(&client, &format!("{RELEASES_URL}?per_page=15")).await?;
        let releases: Vec<Release> = serde_json::from_str(&body).ok()?;
        newer_release(current, &releases)
    }
}

async fn fetch(client: &reqwest::Client, url: &str) -> Option<String> {
    client
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .await
        .ok()
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
            newer_release("0.8.0-canary.2", &releases).as_deref(),
            Some("v0.8.0-canary.3")
        );
        // The stable build ignores the canary and the draft outright.
        assert_eq!(newer_release("0.6.0", &releases).as_deref(), Some("v0.7.0"));
        assert_eq!(newer_release("0.7.0", &releases), None);
    }
}
