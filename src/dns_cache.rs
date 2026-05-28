#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{IpAddr, SocketAddr, TcpStream, UdpSocket},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, anyhow, bail};
use url::Url;

use crate::proxy::ProxyUrl;

const CACHE_TTL: Duration = Duration::from_secs(300);

pub struct DnsCacheHandle {
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl DnsCacheHandle {
    pub fn spawn(
        socket: UdpSocket,
        upstream_dns: SocketAddr,
        proxy: ProxyUrl,
        quiet: bool,
    ) -> anyhow::Result<Self> {
        socket
            .set_read_timeout(Some(Duration::from_millis(500)))
            .context("failed to set DNS cache socket timeout")?;
        let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shutdown_thread = shutdown.clone();
        let proxy = proxy.to_string();
        let thread = thread::Builder::new()
            .name("ptun-dns-cache".to_string())
            .spawn(move || run_dns_cache(socket, upstream_dns, &proxy, shutdown_thread, quiet))
            .context("failed to start DNS cache thread")?;
        Ok(Self {
            shutdown,
            thread: Some(thread),
        })
    }

    pub fn shutdown(mut self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_dns_cache(
    socket: UdpSocket,
    upstream_dns: SocketAddr,
    proxy: &str,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
    quiet: bool,
) {
    let mut cache: HashMap<Vec<u8>, (Vec<u8>, Instant)> = HashMap::new();
    let mut buf = [0_u8; 4096];

    while !shutdown.load(std::sync::atomic::Ordering::SeqCst) {
        let (len, peer) = match socket.recv_from(&mut buf) {
            Ok(value) => value,
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(err) => {
                if !quiet {
                    tracing::warn!("DNS cache recv failed: {err}");
                }
                continue;
            }
        };

        let query = buf[..len].to_vec();
        let now = Instant::now();
        cache.retain(|_, (_, expires_at)| *expires_at > now);

        let response = match cache.get(&query) {
            Some((response, _)) => response.clone(),
            None => match proxy_dns_query(proxy, upstream_dns, &query) {
                Ok(response) => {
                    cache.insert(query, (response.clone(), now + CACHE_TTL));
                    response
                }
                Err(err) => {
                    if !quiet {
                        tracing::warn!("DNS cache upstream query failed: {err:#}");
                    }
                    continue;
                }
            },
        };

        if let Err(err) = socket.send_to(&response, peer)
            && !quiet
        {
            tracing::warn!("DNS cache send failed: {err}");
        }
    }
}

fn proxy_dns_query(proxy: &str, upstream_dns: SocketAddr, query: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut stream = proxy_connect(proxy, upstream_dns)?;
    let len = u16::try_from(query.len()).context("DNS query too large")?;
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(query)?;
    stream.flush()?;

    let mut len_buf = [0_u8; 2];
    stream.read_exact(&mut len_buf)?;
    let response_len = u16::from_be_bytes(len_buf) as usize;
    let mut response = vec![0_u8; response_len];
    stream.read_exact(&mut response)?;
    Ok(response)
}

fn proxy_connect(proxy: &str, dst: SocketAddr) -> anyhow::Result<TcpStream> {
    let url = Url::parse(proxy).context("invalid proxy URL for DNS cache")?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("proxy URL must include a host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow!("proxy URL must include a port"))?;
    let mut stream = TcpStream::connect((host, port))
        .with_context(|| format!("failed to connect proxy {host}:{port}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;

    match url.scheme() {
        "socks5" => socks5_connect(&mut stream, &url, dst)?,
        "http" => http_connect(&mut stream, &url, dst)?,
        scheme => bail!("unsupported proxy scheme for DNS cache: {scheme}"),
    }

    Ok(stream)
}

fn socks5_connect(stream: &mut TcpStream, url: &Url, dst: SocketAddr) -> anyhow::Result<()> {
    let has_auth = !url.username().is_empty() || url.password().is_some();
    if has_auth {
        stream.write_all(&[0x05, 0x02, 0x00, 0x02])?;
    } else {
        stream.write_all(&[0x05, 0x01, 0x00])?;
    }
    let mut method = [0_u8; 2];
    stream.read_exact(&mut method)?;
    if method[0] != 0x05 {
        bail!("invalid SOCKS5 greeting response");
    }
    match method[1] {
        0x00 => {}
        0x02 => socks5_auth(stream, url)?,
        0xff => bail!("SOCKS5 proxy rejected authentication methods"),
        method => bail!("SOCKS5 proxy selected unsupported auth method {method:#x}"),
    }

    let mut req = vec![0x05, 0x01, 0x00];
    match dst.ip() {
        IpAddr::V4(ip) => {
            req.push(0x01);
            req.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            req.push(0x04);
            req.extend_from_slice(&ip.octets());
        }
    }
    req.extend_from_slice(&dst.port().to_be_bytes());
    stream.write_all(&req)?;

    let mut head = [0_u8; 4];
    stream.read_exact(&mut head)?;
    if head[0] != 0x05 || head[1] != 0x00 {
        bail!("SOCKS5 connect failed with code {:#x}", head[1]);
    }
    match head[3] {
        0x01 => read_discard(stream, 4 + 2)?,
        0x03 => {
            let mut len = [0_u8; 1];
            stream.read_exact(&mut len)?;
            read_discard(stream, len[0] as usize + 2)?;
        }
        0x04 => read_discard(stream, 16 + 2)?,
        atyp => bail!("SOCKS5 response used unsupported address type {atyp:#x}"),
    }
    Ok(())
}

fn socks5_auth(stream: &mut TcpStream, url: &Url) -> anyhow::Result<()> {
    let username = url.username().as_bytes();
    let password = url.password().unwrap_or("").as_bytes();
    let user_len = u8::try_from(username.len()).context("SOCKS5 username too long")?;
    let pass_len = u8::try_from(password.len()).context("SOCKS5 password too long")?;
    let mut req = vec![0x01, user_len];
    req.extend_from_slice(username);
    req.push(pass_len);
    req.extend_from_slice(password);
    stream.write_all(&req)?;
    let mut resp = [0_u8; 2];
    stream.read_exact(&mut resp)?;
    if resp != [0x01, 0x00] {
        bail!("SOCKS5 username/password authentication failed");
    }
    Ok(())
}

fn http_connect(stream: &mut TcpStream, url: &Url, dst: SocketAddr) -> anyhow::Result<()> {
    if !url.username().is_empty() || url.password().is_some() {
        bail!("HTTP proxy authentication is not supported by DNS cache yet");
    }
    let authority = match dst.ip() {
        IpAddr::V4(_) => dst.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]:{}", dst.port()),
    };
    write!(
        stream,
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n"
    )?;
    let mut response = Vec::with_capacity(256);
    let mut byte = [0_u8; 1];
    while response.len() < 8192 {
        stream.read_exact(&mut byte)?;
        response.push(byte[0]);
        if response.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let status_line = response
        .split(|b| *b == b'\n')
        .next()
        .and_then(|line| std::str::from_utf8(line).ok())
        .unwrap_or("");
    if !status_line.contains(" 200 ") {
        bail!("HTTP CONNECT failed: {}", status_line.trim());
    }
    Ok(())
}

fn read_discard(stream: &mut TcpStream, len: usize) -> anyhow::Result<()> {
    let mut buf = vec![0_u8; len];
    stream.read_exact(&mut buf)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_ttl_is_nonzero() {
        assert!(CACHE_TTL.as_secs() > 0);
    }

    #[test]
    fn parses_proxy_url_for_socks() {
        let dst: SocketAddr = "1.1.1.1:53".parse().unwrap();
        assert_eq!(dst.port(), 53);
        assert!(Url::parse("socks5://127.0.0.1:1080").is_ok());
    }
}
