use std::{fs, net::SocketAddr, path::PathBuf};

use anyhow::{Context, anyhow};
use serde::Deserialize;

use crate::{
    cli::{Cli, Command, UdpMode},
    proxy::ProxyUrl,
};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    pub proxy: Option<String>,
    pub dns: Option<String>,
    pub tun_name: Option<String>,
    pub mtu: Option<u16>,
    pub ipv6: Option<bool>,
    pub udp: Option<UdpMode>,
    pub fail_open: Option<bool>,
    pub log_level: Option<String>,
}

#[derive(Clone, Debug)]
pub struct EffectiveConfig {
    pub config_path: Option<PathBuf>,
    pub proxy: Option<ProxyUrl>,
    pub dns: Option<SocketAddr>,
    pub tun_name: String,
    pub mtu: u16,
    pub ipv6: bool,
    pub udp: UdpMode,
    pub fail_open: bool,
    pub log_level: Option<String>,
}

impl EffectiveConfig {
    pub fn load(cli: &Cli) -> anyhow::Result<Self> {
        let config_path = config_path(cli);
        let file_config = match &config_path {
            Some(path) if path.exists() => {
                let raw = fs::read_to_string(path)
                    .with_context(|| format!("failed to read config {}", path.display()))?;
                toml::from_str::<FileConfig>(&raw)
                    .with_context(|| format!("failed to parse config {}", path.display()))?
            }
            Some(path) if cli.config.is_some() => {
                return Err(anyhow!("config file does not exist: {}", path.display()));
            }
            _ => FileConfig::default(),
        };

        let run = match &cli.command {
            Command::Run(run) => Some(run),
            _ => None,
        };

        let proxy = run
            .and_then(|r| r.proxy.clone())
            .or(file_config.proxy)
            .map(|value| value.parse())
            .transpose()?;

        let dns = run
            .and_then(|r| r.dns.clone())
            .or(file_config.dns)
            .map(|value| parse_socket_addr("dns", &value))
            .transpose()?;

        let tun_name = run
            .and_then(|r| r.tun_name.clone())
            .or(file_config.tun_name)
            .unwrap_or_else(|| "ptun0".to_string());

        let mtu = run
            .and_then(|r| r.mtu)
            .or(file_config.mtu)
            .unwrap_or(tun::DEFAULT_MTU);

        let ipv6 = match run {
            Some(run) if run.ipv6 => true,
            Some(run) if run.no_ipv6 => false,
            _ => file_config.ipv6.unwrap_or(false),
        };

        let udp = run
            .and_then(|r| r.udp)
            .or(file_config.udp)
            .unwrap_or(UdpMode::Auto);

        let fail_open = match run {
            Some(run) if run.fail_open => true,
            Some(run) if run.fail_closed => false,
            _ => file_config.fail_open.unwrap_or(false),
        };

        let log_level = cli
            .log_level
            .clone()
            .or(file_config.log_level)
            .or_else(|| verbosity_log_level(cli.verbose).map(str::to_string));

        Ok(Self {
            config_path,
            proxy,
            dns,
            tun_name,
            mtu,
            ipv6,
            udp,
            fail_open,
            log_level,
        })
    }
}

fn config_path(cli: &Cli) -> Option<PathBuf> {
    if cli.no_config {
        return None;
    }

    cli.config.clone().or_else(|| {
        dirs::config_dir()
            .map(|dir| dir.join("ptun").join("config.toml"))
            .filter(|path| path.exists())
    })
}

fn parse_socket_addr(field: &str, value: &str) -> anyhow::Result<SocketAddr> {
    value
        .parse()
        .with_context(|| format!("{field} must be host:port socket address, got {value:?}"))
}

fn verbosity_log_level(verbose: u8) -> Option<&'static str> {
    match verbose {
        0 => None,
        1 => Some("info"),
        2 => Some("debug"),
        _ => Some("trace"),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::cli::Cli;

    use super::*;

    #[test]
    fn cli_overrides_config_values() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        fs::write(
            temp.path(),
            r#"
proxy = "socks5://127.0.0.1:1080"
dns = "1.1.1.1:53"
tun_name = "from-config"
mtu = 1400
ipv6 = true
udp = "off"
fail_open = false
log_level = "warn"
"#,
        )
        .unwrap();

        let cli = Cli::parse_from([
            "ptun",
            "-c",
            temp.path().to_str().unwrap(),
            "run",
            "-p",
            "http://127.0.0.1:8080",
            "-d",
            "8.8.8.8:53",
            "-t",
            "from-cli",
            "-u",
            "on",
            "--fail-open",
            "--",
            "curl",
            "https://example.com",
        ]);

        let config = EffectiveConfig::load(&cli).unwrap();
        assert_eq!(config.proxy.unwrap().scheme(), "http");
        assert_eq!(config.dns.unwrap().to_string(), "8.8.8.8:53");
        assert_eq!(config.tun_name, "from-cli");
        assert_eq!(config.mtu, 1400);
        assert!(config.ipv6);
        assert_eq!(config.udp, UdpMode::On);
        assert!(config.fail_open);
    }

    #[test]
    fn config_is_loaded_from_explicit_path() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        fs::write(temp.path(), r#"proxy = "socks5://127.0.0.1:1080""#).unwrap();

        let cli = Cli::parse_from(["ptun", "-c", temp.path().to_str().unwrap(), "check"]);

        let config = EffectiveConfig::load(&cli).unwrap();
        assert_eq!(config.proxy.unwrap().scheme(), "socks5");
    }

    #[test]
    fn cli_can_override_fail_open_to_false() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        fs::write(
            temp.path(),
            r#"
proxy = "socks5://127.0.0.1:1080"
fail_open = true
"#,
        )
        .unwrap();

        let cli = Cli::parse_from([
            "ptun",
            "-c",
            temp.path().to_str().unwrap(),
            "run",
            "--fail-closed",
            "--",
            "curl",
            "https://example.com",
        ]);

        let config = EffectiveConfig::load(&cli).unwrap();
        assert!(!config.fail_open);
    }

    #[test]
    fn cli_can_override_ipv6_to_false() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        fs::write(
            temp.path(),
            r#"
proxy = "socks5://127.0.0.1:1080"
ipv6 = true
"#,
        )
        .unwrap();

        let cli = Cli::parse_from([
            "ptun",
            "-c",
            temp.path().to_str().unwrap(),
            "run",
            "--no-ipv6",
            "--",
            "curl",
            "https://example.com",
        ]);

        let config = EffectiveConfig::load(&cli).unwrap();
        assert!(!config.ipv6);
    }
}
