# PTun

Linux-first per-process transparent proxy launcher. PTun avoids the `LD_PRELOAD` limitations of proxychains by routing a launched process through a private network namespace and TUN device.

[中文首页](README.md)

## Features

- Per-process transparent proxying via Linux network and mount namespaces.
- Works for Go programs, static binaries, direct syscalls, TCP, UDP, and DNS traffic.
- Embedded `tun2proxy` data plane.
- SOCKS5 and HTTP upstream proxy support.
- Automatic DNS discovery via `resolvectl`, systemd-resolved, NetworkManager, and `/etc/resolv.conf`.
- In-namespace DNS cache on `10.0.0.1:53` to reduce repeated DNS latency.
- Netlink-based interface/address/route setup; no `ip` command is required.
- TOML config with command-line overrides.
- Quiet mode for suppressing PTun and tun2proxy logs.

## Usage

```sh
sudo ptun run -p socks5://127.0.0.1:1080 -- curl https://ifconfig.me
sudo ptun run --proxy http://127.0.0.1:8080 -- curl https://example.com
sudo ptun run -c ./ptun.toml -- curl https://example.com
sudo ptun run -q -- curl https://example.com
```

Diagnostics:

```sh
ptun check
ptun status
ptun doctor
```

## Build

```sh
cargo build --release
```

The release profile enables LTO, symbol stripping, `panic = "abort"`, and size-oriented optimization by default. The Linux release binary is about `3.9MB`. Use `target/release/ptun` for deployment; debug builds are not representative for binary size.

## Config

Default config path:

```text
~/.config/ptun/config.toml
```

Use `-c, --config` to specify a config file, or `--no-config` to disable default config loading. Command-line flags override config file values.

Example:

```toml
proxy = "socks5://127.0.0.1:1080"
# dns = "1.1.1.1:53" # optional; omit to auto-discover host DNS
tun_name = "ptun0"
mtu = 1500
ipv6 = false
udp = "auto"
fail_open = false
quiet = false
log_level = "info"
```

## DNS

If `dns` is omitted, PTun discovers an upstream DNS server in this order:

1. `resolvectl dns`
2. `/run/systemd/resolve/resolv.conf`
3. `/run/NetworkManager/no-stub-resolv.conf`
4. `/etc/resolv.conf`
5. fallback `1.1.1.1:53`

Loopback or unspecified DNS servers such as `127.0.0.53`, `127.0.0.1`, and `0.0.0.0` are ignored as upstream targets because they cannot be reached correctly through a remote proxy.

The target process sees a private resolver at `10.0.0.1`. PTun answers through an in-namespace DNS cache; cache misses are forwarded through the configured proxy to the selected upstream DNS server.

## Requirements

`ptun run` requires:

- Linux
- root privileges
- `/dev/net/tun`
- network namespace support
- mount namespace support

HTTP proxy mode is TCP-only. SOCKS5 mode supports UDP when the upstream proxy supports SOCKS5 UDP. Unsupported traffic fails closed by default.
