<div align="center">
  <h1 align="center">PTun</h1>
  <p align="center">
    Linux 按进程透明代理工具。绕过 proxychains 的 LD_PRELOAD 限制，把目标进程的 TCP、UDP 和 DNS 流量导入代理链路。
  </p>
  <p align="center">
    <a href="README.en.md">English</a>
    ·
    <a href="#快速开始">快速开始</a>
    ·
    <a href="#配置文件">配置文件</a>
    ·
    <a href="#诊断">诊断</a>
  </p>
</div>

<p align="center">
  <img alt="Linux" src="https://img.shields.io/badge/Linux-required-111827?style=flat-square">
  <img alt="Proxy" src="https://img.shields.io/badge/proxy-SOCKS5%20%7C%20HTTP-2563eb?style=flat-square">
  <img alt="DNS Cache" src="https://img.shields.io/badge/DNS-cache-059669?style=flat-square">
  <img alt="Netlink" src="https://img.shields.io/badge/setup-netlink-7c3aed?style=flat-square">
  <img alt="Release Size" src="https://img.shields.io/badge/release-~3.9MB-f97316?style=flat-square">
</p>

## 为什么需要 PTun

`proxychains` 依赖 `LD_PRELOAD` hook libc 网络调用，遇到下面这些场景很容易失效：

- Go 程序有自己的网络栈，不一定经过 libc。
- 静态编译二进制不链接动态 libc。
- 直接 syscall 的程序会绕过 hook。
- SUID 程序会被动态链接器安全策略忽略 `LD_PRELOAD`。
- UDP、DNS、ICMP 等流量覆盖不完整。

PTun 不 hook 进程函数调用。它为目标命令创建独立 network namespace 和 mount namespace，通过 TUN 设备接管默认路由，并把流量交给内嵌数据面转发到上游代理。

## 核心能力

- **按进程透明代理**：只代理 `ptun run` 启动的命令，不污染全局系统网络。
- **覆盖面更完整**：适用于 Go、静态二进制、直接 syscall、TCP、UDP 和 DNS。
- **内嵌 tun2proxy 数据面**：无需单独运行 tun2socks 服务。
- **SOCKS5 / HTTP 上游代理**：命令行和 TOML 配置均支持。
- **DNS 自动发现**：适配 `resolvectl`、systemd-resolved、NetworkManager 和传统 `/etc/resolv.conf`。
- **DNS 缓存**：目标 namespace 内提供 `10.0.0.1:53`，减少重复解析延迟。
- **netlink 配置网络**：不依赖 `ip` 命令，降低不同发行版环境差异。
- **静默模式**：`-q` 或 `quiet = true` 可关闭 PTun 和 tun2proxy 日志。

## 快速开始

通过命令行临时指定代理：

```sh
sudo ptun run -p socks5://127.0.0.1:1080 -- curl https://ifconfig.me
```

使用默认配置文件：

```sh
mkdir -p ~/.config/ptun
cat > ~/.config/ptun/config.toml <<'EOF'
proxy = "socks5://127.0.0.1:1080"
quiet = false
EOF

sudo ptun run -- curl https://ifconfig.me
```

更多示例：

```sh
sudo ptun run --proxy http://127.0.0.1:8080 -- curl https://example.com
sudo ptun run -c ./ptun.toml -- curl https://example.com
sudo ptun run -q -- curl https://example.com
```

## 配置文件

默认配置路径：

```text
~/.config/ptun/config.toml
```

也可以用 `-c, --config` 指定配置文件，或用 `--no-config` 禁用默认配置。命令行参数优先级高于配置文件。

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

常用选项：

| 选项 | 说明 |
| --- | --- |
| `-p, --proxy <url>` | 临时指定代理，例如 `socks5://127.0.0.1:1080` |
| `-d, --dns <addr>` | 临时指定 DNS，例如 `100.100.2.136:53` |
| `-q, --quiet` | 静默 PTun 和 tun2proxy 日志，不影响目标命令输出 |
| `--no-quiet` | 覆盖配置文件中的 `quiet = true` |
| `-6, --ipv6` / `--no-ipv6` | 开启或关闭 IPv6 |
| `-u, --udp <mode>` | 控制 UDP 策略，取值为 <code>auto&#124;on&#124;off</code> |

## DNS 行为

不配置 `dns` 时，PTun 会按顺序自动发现上游 DNS：

1. `resolvectl dns`
2. `/run/systemd/resolve/resolv.conf`
3. `/run/NetworkManager/no-stub-resolv.conf`
4. `/etc/resolv.conf`
5. 兜底使用 `1.1.1.1:53`

`127.0.0.53`、`127.0.0.1`、`0.0.0.0` 这类本地地址不会作为上游 DNS 使用，因为远端代理无法访问你本机的 loopback DNS。

目标进程看到的 DNS 是 `10.0.0.1`。PTun 在目标 namespace 内监听这个地址并提供 DNS 缓存；缓存未命中时，通过配置的代理连接到自动发现的上游 DNS。

## 诊断

```sh
ptun check
ptun status
ptun doctor
```

- `check`：检查配置、权限、TUN 和代理可达性。
- `status`：展示当前 PTun 会话状态。
- `doctor`：输出更完整的环境诊断，包括 DNS 探测路径和 netlink 支持情况。

## 构建

```sh
cargo build --release
```

Release profile 默认开启 LTO、strip、`panic = "abort"` 和小体积优化。Linux release 二进制约 `3.9MB`，请使用 `target/release/ptun` 部署，不要用 debug 构建判断体积。

## 运行要求

`ptun run` 需要：

- Linux
- root 权限
- `/dev/net/tun`
- network namespace 支持
- mount namespace 支持

HTTP 代理模式只支持 TCP。SOCKS5 模式在上游代理支持 SOCKS5 UDP 时可以转发 UDP。默认策略是 fail closed，避免不支持的流量绕过代理泄漏。
