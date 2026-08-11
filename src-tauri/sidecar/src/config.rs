use std::path::PathBuf;

use thiserror::Error;

pub const DEFAULT_API_PORT: u16 = 12_754;
pub const DEFAULT_WEB_PORT: u16 = 28_232;
pub const DEFAULT_PROXY_RELAY_PORT: u16 = 27_233;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SidecarConfig {
    pub api_port: u16,
    pub web_port: u16,
    pub renderer_dir: Option<PathBuf>,
    pub api_only: bool,
    pub proxy_relay_port: u16,
    pub upstream_proxy: Option<String>,
    pub parent_pid: Option<u32>,
}

impl Default for SidecarConfig {
    fn default() -> Self {
        Self {
            api_port: DEFAULT_API_PORT,
            web_port: DEFAULT_WEB_PORT,
            renderer_dir: None,
            api_only: false,
            proxy_relay_port: DEFAULT_PROXY_RELAY_PORT,
            upstream_proxy: None,
            parent_pid: None,
        }
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum ConfigError {
    #[error("unknown argument: {0}")]
    UnknownArgument(String),
    #[error("{0} requires a value")]
    MissingValue(String),
    #[error("{0} must be an integer from 1 through 65535")]
    InvalidPort(String),
    #[error("--parent-pid must be a positive integer")]
    InvalidParentPid,
    #[error("--renderer-dir is required outside API-only mode")]
    MissingRendererDirectory,
    #[error("--proxy-relay-port requires --upstream-proxy")]
    ProxyPortWithoutUpstream,
}

fn next_value(args: &[String], index: usize, flag: &str) -> Result<String, ConfigError> {
    let value = args
        .get(index + 1)
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| ConfigError::MissingValue(flag.to_owned()))?;
    Ok(value.clone())
}

fn parse_port(value: &str, flag: &str) -> Result<u16, ConfigError> {
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| ConfigError::InvalidPort(flag.to_owned()))
}

impl SidecarConfig {
    pub fn parse(args: &[String]) -> Result<Self, ConfigError> {
        let mut config = Self::default();
        let mut proxy_port_was_set = false;
        let mut index = 0;
        while index < args.len() {
            let flag = &args[index];
            match flag.as_str() {
                "--api-port" => {
                    let value = next_value(args, index, flag)?;
                    config.api_port = parse_port(&value, flag)?;
                    index += 2;
                }
                "--web-port" => {
                    let value = next_value(args, index, flag)?;
                    config.web_port = parse_port(&value, flag)?;
                    index += 2;
                }
                "--renderer-dir" => {
                    config.renderer_dir = Some(PathBuf::from(next_value(args, index, flag)?));
                    index += 2;
                }
                "--api-only" => {
                    config.api_only = true;
                    index += 1;
                }
                "--proxy-relay-port" => {
                    let value = next_value(args, index, flag)?;
                    config.proxy_relay_port = parse_port(&value, flag)?;
                    proxy_port_was_set = true;
                    index += 2;
                }
                "--upstream-proxy" => {
                    config.upstream_proxy = Some(next_value(args, index, flag)?);
                    index += 2;
                }
                "--parent-pid" => {
                    config.parent_pid = Some(
                        next_value(args, index, flag)?
                            .parse::<u32>()
                            .ok()
                            .filter(|pid| *pid != 0)
                            .ok_or(ConfigError::InvalidParentPid)?,
                    );
                    index += 2;
                }
                _ => return Err(ConfigError::UnknownArgument(flag.clone())),
            }
        }

        if !config.api_only && config.renderer_dir.is_none() {
            return Err(ConfigError::MissingRendererDirectory);
        }
        if proxy_port_was_set && config.upstream_proxy.is_none() {
            return Err(ConfigError::ProxyPortWithoutUpstream);
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn parses_dev_and_release_contracts() {
        let dev = SidecarConfig::parse(&args(&[
            "--api-only",
            "--api-port",
            "12754",
            "--parent-pid",
            "42",
        ]))
        .unwrap();
        assert!(dev.api_only);
        assert_eq!(dev.api_port, 12_754);
        assert_eq!(dev.parent_pid, Some(42));

        let release = SidecarConfig::parse(&args(&[
            "--renderer-dir",
            "/tmp/renderer",
            "--upstream-proxy",
            "http://proxy.example:8080",
            "--proxy-relay-port",
            "27233",
        ]))
        .unwrap();
        assert_eq!(release.renderer_dir, Some(PathBuf::from("/tmp/renderer")));
        assert_eq!(release.proxy_relay_port, 27_233);
    }

    #[test]
    fn rejects_incomplete_listener_configuration() {
        assert_eq!(
            SidecarConfig::parse(&[]),
            Err(ConfigError::MissingRendererDirectory)
        );
        assert_eq!(
            SidecarConfig::parse(&args(&["--api-only", "--proxy-relay-port", "27233"])),
            Err(ConfigError::ProxyPortWithoutUpstream)
        );
    }
}
