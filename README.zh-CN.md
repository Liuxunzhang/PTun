# ptun

`ptun` 是一个 Linux 优先的按进程透明代理工具。它不依赖 `LD_PRELOAD`，而是通过 network namespace、mount namespace 和 TUN 设备把目标进程的网络流量导入代理链路。

## 功能

- 按进程透明代理，不影响全局系统流量。
- 可覆盖 Go 程序、静态编译二进制、直接 syscall、TCP、UDP 和 DNS。
- 内嵌 `tun2proxy` 数据面。
- 支持 SOCKS5 和 HTTP 上游代理。
- 自动发现当前服务器 DNS：`resolvectl`、systemd-resolved、NetworkManager、`/etc/resolv.conf`。
- 在目标 namespace 内提供 `10.0.0.1:53` DNS 缓存，减少重复 DNS 查询延迟。
- 使用 netlink 配置网卡、地址和路由，不再依赖 `ip` 命令。
- 支持 TOML 配置文件，命令行参数优先级高于配置文件。

## 使用

```sh
sudo ptun run -p socks5://127.0.0.1:1080 -- curl https://ifconfig.me
sudo ptun run --proxy http://127.0.0.1:8080 -- curl https://example.com
sudo ptun run -c ./ptun.toml -- curl https://example.com
sudo ptun run -q -- curl https://example.com
```

诊断命令：

```sh
ptun check
ptun status
ptun doctor
```

## 配置文件

默认配置路径：

```text
~/.config/ptun/config.toml
```

可以用 `-c, --config` 指定配置文件，也可以用 `--no-config` 禁用默认配置。命令行参数会覆盖配置文件中的同名配置。

示例：

```toml
proxy = "socks5://127.0.0.1:1080"
# dns = "1.1.1.1:53" # 可选；不写则自动发现当前服务器 DNS
tun_name = "ptun0"
mtu = 1500
ipv6 = false
udp = "auto"
fail_open = false
quiet = false
log_level = "info"
```

## DNS 行为

如果没有配置 `dns`，`ptun` 会按顺序自动发现上游 DNS：

1. `resolvectl dns`
2. `/run/systemd/resolve/resolv.conf`
3. `/run/NetworkManager/no-stub-resolv.conf`
4. `/etc/resolv.conf`
5. 兜底使用 `1.1.1.1:53`

`127.0.0.53`、`127.0.0.1`、`0.0.0.0` 这类本地地址不会作为远端代理上游 DNS 使用，因为代理服务器无法访问你本机的 loopback DNS。

目标进程看到的 DNS 是 `10.0.0.1`。`ptun` 会在目标 namespace 内监听这个地址并提供 DNS 缓存；缓存未命中时，通过配置的代理连接到自动发现的上游 DNS。

## 常用选项

- `-p, --proxy <url>`：临时指定代理。
- `-d, --dns <addr>`：临时指定 DNS，例如 `100.100.2.136:53`。
- `-q, --quiet`：静默 ptun 和 tun2proxy 日志，不影响目标命令输出。
- `--no-quiet`：覆盖配置文件里的 `quiet = true`。
- `-6, --ipv6` / `--no-ipv6`：开启或关闭 IPv6。
- `-u, --udp <auto|on|off>`：控制 UDP 策略。

## 运行要求

`ptun run` 需要：

- Linux
- root 权限
- `/dev/net/tun`
- network namespace 支持
- mount namespace 支持

HTTP 代理模式只支持 TCP。SOCKS5 模式在上游代理支持 SOCKS5 UDP 时可以转发 UDP。默认策略是 fail closed，避免不支持的流量绕过代理泄漏。
