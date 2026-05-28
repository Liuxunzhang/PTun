# ptun

`ptun` is a Linux-first per-process transparent proxy launcher. It is designed to avoid the `LD_PRELOAD` limitations of tools like proxychains by routing a launched process through a managed network path instead of hooking libc calls.

Current implementation status: `ptun run` creates a private Linux network and mount namespace, mounts a per-process DNS config, creates a TUN device, and runs an embedded `tun2proxy` data plane in the parent namespace.

## Usage

```sh
ptun run -p socks5://127.0.0.1:1080 -- curl https://ifconfig.me
ptun run --proxy http://127.0.0.1:8080 -- curl https://example.com
ptun run -c ./ptun.toml -- curl https://example.com
ptun run -c ./ptun.toml --fail-closed -- curl https://example.com
ptun run -c ./ptun.toml --no-ipv6 -- curl https://example.com
ptun run -q -- curl https://example.com
```

Diagnostics:

```sh
ptun check
ptun status
ptun doctor
```

## Config

By default, `ptun` loads:

```text
~/.config/ptun/config.toml
```

Use `-c, --config` to specify a config file, or `--no-config` to disable default config loading.

Command-line flags override config file values.

If `dns` is omitted, `ptun` loads the first usable `nameserver` from the host `/etc/resolv.conf`; if none is available it falls back to `1.1.1.1:53`. The target process still sees a private per-namespace resolver, so DNS requests enter the TUN path instead of leaking directly.

Example:

```toml
proxy = "socks5://127.0.0.1:1080"
dns = "1.1.1.1:53"
tun_name = "ptun0"
mtu = 1500
ipv6 = false
udp = "auto"
fail_open = false
quiet = false
log_level = "info"
```

## Runtime Notes

`ptun run` requires Linux with root privileges, `/dev/net/tun`, network namespace support, mount namespace support, and the `ip` command from iproute2.

SOCKS5 mode supports TCP and UDP when the upstream proxy supports SOCKS5 UDP. HTTP proxy mode is TCP-only; non-DNS UDP is rejected with an iptables rule so traffic fails closed instead of leaking.
