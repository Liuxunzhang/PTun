use std::{net::TcpStream, time::Duration};

use anyhow::{Context, anyhow};

use crate::{config::EffectiveConfig, linux};

pub fn check(config: &EffectiveConfig) -> anyhow::Result<()> {
    validate_config(config)?;
    linux::EnvironmentReport::collect().validate_for_run()?;
    check_proxy_reachable(config)?;
    println!("ptun check: ok");
    Ok(())
}

pub fn status(_config: &EffectiveConfig) -> anyhow::Result<()> {
    let sessions = linux::read_session_files()?;
    if sessions.is_empty() {
        println!("ptun status: no active sessions");
    } else {
        println!("ptun status: active sessions");
        for session in sessions {
            println!("{session}");
        }
    }
    Ok(())
}

pub fn doctor(config: &EffectiveConfig) -> anyhow::Result<()> {
    let report = linux::EnvironmentReport::collect();
    println!("ptun doctor");
    println!("  linux: {}", yes_no(report.is_linux));
    println!("  root: {}", yes_no(report.is_root));
    println!("  /dev/net/tun: {}", yes_no(report.tun_available));
    println!(
        "  user namespace: {}",
        yes_no(report.user_namespace_available)
    );
    println!(
        "  network namespace: {}",
        yes_no(report.network_namespace_available)
    );
    println!("  config path: {}", config_path(config));
    println!(
        "  proxy configured: {}",
        yes_no(config.proxy.as_ref().is_some())
    );
    if let Some(proxy) = &config.proxy {
        println!("  proxy scheme: {}", proxy.scheme());
    }
    println!(
        "  dns: {}",
        config
            .dns
            .map(|addr| addr.to_string())
            .unwrap_or_else(|| "system default".to_string())
    );
    println!("  tun name: {}", config.tun_name);
    println!("  udp mode: {:?}", config.udp);
    println!("  fail open: {}", yes_no(config.fail_open));

    if let Err(err) = validate_config(config) {
        println!("  config validation: failed: {err:#}");
    } else {
        println!("  config validation: ok");
    }

    if let Err(err) = check_proxy_reachable(config) {
        println!("  proxy reachability: failed: {err:#}");
    } else {
        println!("  proxy reachability: ok");
    }

    Ok(())
}

pub fn validate_config(config: &EffectiveConfig) -> anyhow::Result<()> {
    if config.proxy.is_none() {
        return Err(anyhow!(
            "proxy is required; set proxy in config or pass --proxy"
        ));
    }
    if config.tun_name.trim().is_empty() {
        return Err(anyhow!("tun_name cannot be empty"));
    }
    Ok(())
}

fn check_proxy_reachable(config: &EffectiveConfig) -> anyhow::Result<()> {
    let proxy = config
        .proxy
        .as_ref()
        .ok_or_else(|| anyhow!("proxy is required"))?;
    let host = proxy
        .host()
        .ok_or_else(|| anyhow!("proxy URL must include a host"))?;
    let port = proxy
        .port()
        .ok_or_else(|| anyhow!("proxy URL must include a port"))?;
    let addrs = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("failed to resolve proxy {host}:{port}"))?;

    let mut last_error = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
            Ok(_) => return Ok(()),
            Err(err) => last_error = Some(err),
        }
    }

    Err(anyhow!(
        "proxy is unreachable: {}",
        last_error
            .map(|err| err.to_string())
            .unwrap_or_else(|| "no resolved addresses".to_string())
    ))
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn config_path(config: &EffectiveConfig) -> String {
    config
        .config_path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "none".to_string())
}

use std::net::ToSocketAddrs;
