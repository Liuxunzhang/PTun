# ptun

`ptun` is a Linux-first per-process transparent proxy launcher. It is designed to avoid the `LD_PRELOAD` limitations of tools like proxychains by routing a launched process through a managed network path instead of hooking libc calls.

Current implementation status: the CLI, TOML config loading, config precedence, and diagnostics are implemented. The Linux TUN/network-namespace data plane is scaffolded but not complete yet.

## Usage

```sh
ptun run -p socks5://127.0.0.1:1080 -- curl https://ifconfig.me
ptun run --proxy http://127.0.0.1:8080 -- curl https://example.com
ptun run -c ./ptun.toml -- curl https://example.com
ptun run -c ./ptun.toml --fail-closed -- curl https://example.com
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

Example:

```toml
proxy = "socks5://127.0.0.1:1080"
dns = "1.1.1.1:53"
tun_name = "ptun0"
udp = "auto"
fail_open = false
log_level = "info"
```

## Requirements

The intended `run` implementation requires Linux with root or `CAP_NET_ADMIN`, `/dev/net/tun`, and network namespace support.
