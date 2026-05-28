use std::{
    env, fs,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    process::Command as ProcessCommand,
};

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
    pub quiet: Option<bool>,
    pub log_level: Option<String>,
}

#[derive(Clone, Debug)]
pub struct EffectiveConfig {
    pub config_path: Option<PathBuf>,
    pub proxy: Option<ProxyUrl>,
    pub dns: Option<SocketAddr>,
    pub dns_source: DnsSource,
    pub dns_probe_log: Vec<String>,
    pub tun_name: String,
    pub mtu: u16,
    pub ipv6: bool,
    pub udp: UdpMode,
    pub fail_open: bool,
    pub quiet: bool,
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

        let configured_dns = run
            .and_then(|r| r.dns.clone())
            .or(file_config.dns)
            .map(|value| parse_socket_addr("dns", &value))
            .transpose()?;
        let (dns, dns_source, dns_probe_log) = match configured_dns {
            Some(dns) => (
                Some(dns),
                DnsSource::Configured,
                vec![format!("configured dns: {dns}")],
            ),
            None => {
                let probe = discover_system_dns();
                (Some(probe.addr), probe.source, probe.log)
            }
        };

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

        let quiet = match (cli.quiet, cli.no_quiet) {
            (true, _) => true,
            (_, true) => false,
            _ => file_config.quiet.unwrap_or(false),
        };

        let log_level = cli
            .log_level
            .clone()
            .or(file_config.log_level)
            .or_else(|| quiet.then(|| "off".to_string()))
            .or_else(|| verbosity_log_level(cli.verbose).map(str::to_string));

        Ok(Self {
            config_path,
            proxy,
            dns,
            dns_source,
            dns_probe_log,
            tun_name,
            mtu,
            ipv6,
            udp,
            fail_open,
            quiet,
            log_level,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DnsSource {
    Configured,
    Resolvectl,
    SystemdResolved,
    NetworkManager,
    EtcResolvConf,
    Fallback,
}

impl std::fmt::Display for DnsSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DnsSource::Configured => write!(f, "configured"),
            DnsSource::Resolvectl => write!(f, "resolvectl"),
            DnsSource::SystemdResolved => write!(f, "/run/systemd/resolve/resolv.conf"),
            DnsSource::NetworkManager => write!(f, "/run/NetworkManager/no-stub-resolv.conf"),
            DnsSource::EtcResolvConf => write!(f, "/etc/resolv.conf"),
            DnsSource::Fallback => write!(f, "fallback"),
        }
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

#[derive(Debug)]
struct DnsProbe {
    addr: SocketAddr,
    source: DnsSource,
    log: Vec<String>,
}

fn discover_system_dns() -> DnsProbe {
    let mut log = Vec::new();

    if let Some(addr) = resolvectl_dns(&mut log) {
        return DnsProbe {
            addr,
            source: DnsSource::Resolvectl,
            log,
        };
    }

    for (path, source) in [
        (
            "/run/systemd/resolve/resolv.conf",
            DnsSource::SystemdResolved,
        ),
        (
            "/run/NetworkManager/no-stub-resolv.conf",
            DnsSource::NetworkManager,
        ),
        ("/etc/resolv.conf", DnsSource::EtcResolvConf),
    ] {
        match fs::read_to_string(path) {
            Ok(raw) => match parse_resolv_conf_dns(&raw) {
                Some(addr) => {
                    log.push(format!("{path}: selected {addr}"));
                    return DnsProbe { addr, source, log };
                }
                None => log.push(format!("{path}: no usable non-loopback nameserver")),
            },
            Err(err) => log.push(format!("{path}: unreadable ({err})")),
        }
    }

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 53);
    log.push(format!("fallback: selected {addr}"));
    DnsProbe {
        addr,
        source: DnsSource::Fallback,
        log,
    }
}

fn resolvectl_dns(log: &mut Vec<String>) -> Option<SocketAddr> {
    if !command_available("resolvectl") {
        log.push("resolvectl: not found in PATH".to_string());
        return None;
    }

    match ProcessCommand::new("resolvectl").arg("dns").output() {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            match parse_resolvectl_dns(&stdout) {
                Some(addr) => {
                    log.push(format!("resolvectl dns: selected {addr}"));
                    Some(addr)
                }
                None => {
                    log.push("resolvectl dns: no usable non-loopback DNS".to_string());
                    None
                }
            }
        }
        Ok(output) => {
            log.push(format!("resolvectl dns: exited with {}", output.status));
            None
        }
        Err(err) => {
            log.push(format!("resolvectl dns: failed ({err})"));
            None
        }
    }
}

fn parse_resolvectl_dns(raw: &str) -> Option<SocketAddr> {
    for line in raw.lines() {
        let Some((_, right)) = line.split_once(':') else {
            continue;
        };
        for token in right.split_whitespace() {
            let token = token.trim_matches(|ch| ch == '[' || ch == ']');
            let Ok(ip) = token.parse::<IpAddr>() else {
                continue;
            };
            if !is_loopback_or_unspecified(ip) {
                return Some(SocketAddr::new(ip, 53));
            }
        }
    }
    None
}

fn parse_resolv_conf_dns(raw: &str) -> Option<SocketAddr> {
    for line in raw.lines() {
        let line = line.split_once('#').map(|(left, _)| left).unwrap_or(line);
        let mut parts = line.split_whitespace();
        if parts.next() != Some("nameserver") {
            continue;
        }
        let Some(value) = parts.next() else {
            continue;
        };
        let Ok(ip) = value.parse::<IpAddr>() else {
            continue;
        };
        let addr = SocketAddr::new(ip, 53);
        if !is_loopback_or_unspecified(ip) {
            return Some(addr);
        }
    }
    None
}

fn is_loopback_or_unspecified(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_unspecified(),
        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unspecified(),
    }
}

fn command_available(command: &str) -> bool {
    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|path| path.join(command).exists()))
        .unwrap_or(false)
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
quiet = false
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
        assert_eq!(config.dns_source, DnsSource::Configured);
        assert_eq!(config.dns_probe_log, ["configured dns: 8.8.8.8:53"]);
        assert_eq!(config.tun_name, "from-cli");
        assert_eq!(config.mtu, 1400);
        assert!(config.ipv6);
        assert_eq!(config.udp, UdpMode::On);
        assert!(config.fail_open);
        assert!(!config.quiet);
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

    #[test]
    fn quiet_sets_log_level_off() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        fs::write(
            temp.path(),
            r#"
proxy = "socks5://127.0.0.1:1080"
quiet = true
"#,
        )
        .unwrap();

        let cli = Cli::parse_from([
            "ptun",
            "-c",
            temp.path().to_str().unwrap(),
            "run",
            "--",
            "curl",
            "https://example.com",
        ]);

        let config = EffectiveConfig::load(&cli).unwrap();
        assert!(config.quiet);
        assert_eq!(config.log_level.as_deref(), Some("off"));
    }

    #[test]
    fn cli_can_override_quiet_to_false() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        fs::write(
            temp.path(),
            r#"
proxy = "socks5://127.0.0.1:1080"
quiet = true
"#,
        )
        .unwrap();

        let cli = Cli::parse_from([
            "ptun",
            "-c",
            temp.path().to_str().unwrap(),
            "--no-quiet",
            "run",
            "--",
            "curl",
            "https://example.com",
        ]);

        let config = EffectiveConfig::load(&cli).unwrap();
        assert!(!config.quiet);
        assert_eq!(config.log_level, None);
    }

    #[test]
    fn parses_resolv_conf_dns_preferring_non_loopback() {
        let dns = parse_resolv_conf_dns(
            r#"
nameserver 127.0.0.53
nameserver 192.168.0.1
nameserver 1.1.1.1
"#,
        )
        .unwrap();
        assert_eq!(dns.to_string(), "192.168.0.1:53");
    }

    #[test]
    fn ignores_loopback_resolv_conf_dns() {
        let dns = parse_resolv_conf_dns("nameserver 127.0.0.53\n");
        assert_eq!(dns, None);
    }

    #[test]
    fn parses_systemd_resolved_uplink_dns() {
        let dns = parse_resolv_conf_dns(
            r#"
nameserver 100.100.2.136
nameserver 100.100.2.138
search .
"#,
        )
        .unwrap();
        assert_eq!(dns.to_string(), "100.100.2.136:53");
    }

    #[test]
    fn parses_resolvectl_dns_output() {
        let dns = parse_resolvectl_dns(
            r#"
Global:
Link 2 (eth0): 100.100.2.136 100.100.2.138
"#,
        )
        .unwrap();
        assert_eq!(dns.to_string(), "100.100.2.136:53");
    }

    #[test]
    fn resolvectl_dns_output_ignores_loopback() {
        let dns = parse_resolvectl_dns("Global: 127.0.0.53\nLink 2 (eth0): 10.0.0.2\n").unwrap();
        assert_eq!(dns.to_string(), "10.0.0.2:53");
    }
}
