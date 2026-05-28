use crate::{cli::RunArgs, config::EffectiveConfig};

#[cfg(target_os = "linux")]
mod linux_runtime {
    use std::{
        ffi::CString,
        fs::{self, File},
        net::{IpAddr, Ipv4Addr},
        os::fd::{IntoRawFd, OwnedFd},
        path::{Path, PathBuf},
        process::Command,
        thread,
        time::Duration,
    };

    use anyhow::{Context, anyhow, bail};
    use futures_util::stream::TryStreamExt;
    use nix::{
        fcntl::OFlag,
        mount::{MsFlags, mount},
        sched::{CloneFlags, setns, unshare},
        sys::wait::{WaitStatus, waitpid},
        unistd::{ForkResult, Pid, execvp, fork, pipe2, read, write},
    };
    use tun::AbstractDevice;
    use tun2proxy::{ArgDns, ArgProxy, ArgVerbosity, Args as Tun2ProxyArgs, ProxyType};

    use crate::{
        cli::{RunArgs, UdpMode},
        config::EffectiveConfig,
        diagnostics, dns_cache, linux,
    };

    const TUN_ADDR: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 33);
    const TUN_GATEWAY: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 1);
    const TUN_NETMASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);

    pub fn run(args: &RunArgs, config: &EffectiveConfig) -> anyhow::Result<()> {
        diagnostics::validate_config(config)?;
        linux::EnvironmentReport::collect().validate_for_run()?;

        let proxy = config
            .proxy
            .as_ref()
            .ok_or_else(|| anyhow!("proxy is required"))?;
        let proxy_arg = ArgProxy::try_from(proxy.to_string().as_str())
            .map_err(|err| anyhow!("failed to build proxy runtime config: {err}"))?;

        let resolv_path = create_resolv_conf()?;
        let (ready_read, ready_write) =
            pipe2(OFlag::O_CLOEXEC).context("failed to create child-ready pipe")?;
        let (go_read, go_write) =
            pipe2(OFlag::O_CLOEXEC).context("failed to create child-start pipe")?;

        match unsafe { fork() }.context("failed to fork target process")? {
            ForkResult::Child => child_main(args, &resolv_path, ready_write, go_read),
            ForkResult::Parent { child } => {
                drop(ready_write);
                drop(go_read);
                parent_main(
                    child,
                    ready_read,
                    go_write,
                    proxy_arg,
                    config,
                    args,
                    &resolv_path,
                )
            }
        }
    }

    fn parent_main(
        child: Pid,
        ready_read: OwnedFd,
        go_write: OwnedFd,
        proxy_arg: ArgProxy,
        config: &EffectiveConfig,
        run_args: &RunArgs,
        resolv_path: &Path,
    ) -> anyhow::Result<()> {
        wait_for_child_ready(&ready_read)?;

        let original_ns =
            File::open("/proc/self/ns/net").context("failed to open current network namespace")?;
        let child_ns_path = format!("/proc/{}/ns/net", child.as_raw());
        let child_ns = File::open(&child_ns_path)
            .with_context(|| format!("failed to open child network namespace {child_ns_path}"))?;

        setns(&child_ns, CloneFlags::CLONE_NEWNET).context("failed to enter child netns")?;
        let setup_result = setup_target_namespace(config, &proxy_arg);
        let restore_result = setns(&original_ns, CloneFlags::CLONE_NEWNET)
            .context("failed to restore original netns");
        let setup = setup_result?;
        restore_result?;

        let _ = fs::remove_file(resolv_path);

        let shutdown = tun2proxy::CancellationToken::new();
        let proxy_thread = start_proxy_thread(setup.tun_fd, proxy_arg, config, shutdown.clone())?;
        let dns_cache = match (setup.dns_socket, config.dns, config.proxy.clone()) {
            (Some(socket), Some(dns), Some(proxy)) => Some(dns_cache::DnsCacheHandle::spawn(
                socket,
                dns,
                proxy,
                config.quiet,
            )?),
            _ => None,
        };

        write(&go_write, &[1]).context("failed to release child process")?;
        drop(go_write);

        let session = linux::SessionRecord {
            pid: child.as_raw() as u32,
            command: run_args.command.join(" "),
            proxy: config
                .proxy
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            tun_name: config.tun_name.clone(),
        };
        let session_path = linux::write_session(&session)?;
        let status = waitpid(child, None).context("failed to wait for target process")?;

        if let Some(dns_cache) = dns_cache {
            dns_cache.shutdown();
        }
        shutdown.cancel();
        let proxy_result = proxy_thread
            .join()
            .map_err(|_| anyhow!("tun2proxy worker thread panicked"))?;
        linux::remove_session(&session_path);

        if let Err(err) = proxy_result {
            tracing::warn!("tun2proxy stopped with error: {err:#}");
        }

        exit_like_child(status);
    }

    fn child_main(args: &RunArgs, resolv_path: &Path, ready_write: OwnedFd, go_read: OwnedFd) -> ! {
        if let Err(err) = child_setup(resolv_path, &ready_write, &go_read) {
            eprintln!("ptun child setup failed: {err:#}");
            std::process::exit(126);
        }

        let cstrings = match args
            .command
            .iter()
            .map(|arg| CString::new(arg.as_bytes()))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(values) => values,
            Err(_) => {
                eprintln!("ptun: command arguments cannot contain NUL bytes");
                std::process::exit(126);
            }
        };

        let Some(program) = cstrings.first() else {
            eprintln!("ptun: missing command");
            std::process::exit(126);
        };

        match execvp(program, &cstrings) {
            Ok(_) => unreachable!(),
            Err(err) => {
                eprintln!("ptun: failed to exec {}: {err}", args.command[0]);
                std::process::exit(127);
            }
        }
    }

    fn child_setup(
        resolv_path: &Path,
        ready_write: &OwnedFd,
        go_read: &OwnedFd,
    ) -> anyhow::Result<()> {
        unshare(CloneFlags::CLONE_NEWNET | CloneFlags::CLONE_NEWNS)
            .context("failed to unshare network/mount namespaces")?;
        mount::<str, str, str, str>(None, "/", None, MsFlags::MS_REC | MsFlags::MS_PRIVATE, None)
            .context("failed to make mount namespace private")?;
        mount(
            Some(resolv_path),
            Path::new("/etc/resolv.conf"),
            Option::<&str>::None,
            MsFlags::MS_BIND,
            Option::<&str>::None,
        )
        .context("failed to bind ptun resolv.conf")?;

        write(ready_write, &[1]).context("failed to signal child readiness")?;
        let mut byte = [0_u8; 1];
        read(go_read, &mut byte).context("failed to wait for parent network setup")?;
        Ok(())
    }

    struct NamespaceSetup {
        tun_fd: i32,
        dns_socket: Option<std::net::UdpSocket>,
    }

    fn setup_target_namespace(
        config: &EffectiveConfig,
        proxy: &ArgProxy,
    ) -> anyhow::Result<NamespaceSetup> {
        let tun_fd = create_tun(config)?;
        configure_namespace_netlink(config)?;
        let dns_socket = bind_dns_cache_socket().ok();

        if should_block_udp(config, proxy.proxy_type) {
            add_udp_reject_rule()?;
        }

        Ok(NamespaceSetup { tun_fd, dns_socket })
    }

    fn create_tun(config: &EffectiveConfig) -> anyhow::Result<i32> {
        let mut tun_config = tun::Configuration::default();
        tun_config
            .tun_name(&config.tun_name)
            .address(IpAddr::V4(TUN_ADDR))
            .destination(IpAddr::V4(TUN_GATEWAY))
            .netmask(IpAddr::V4(TUN_NETMASK))
            .mtu(config.mtu)
            .up();
        tun_config.platform_config(|cfg| {
            #[allow(deprecated)]
            cfg.packet_information(true);
            cfg.ensure_root_privileges(true);
        });

        let device = tun::create(&tun_config).context("failed to create TUN device")?;
        let actual_name = device
            .tun_name()
            .context("failed to read created TUN device name")?;
        if actual_name != config.tun_name {
            bail!(
                "created TUN name {actual_name:?} did not match requested {:?}",
                config.tun_name
            );
        }
        Ok(device.into_raw_fd())
    }

    fn start_proxy_thread(
        tun_fd: i32,
        proxy: ArgProxy,
        config: &EffectiveConfig,
        shutdown: tun2proxy::CancellationToken,
    ) -> anyhow::Result<thread::JoinHandle<anyhow::Result<usize>>> {
        let args = build_tun2proxy_args(tun_fd, proxy, config);
        let mtu = config.mtu;
        let thread = thread::Builder::new()
            .name("ptun-tun2proxy".to_string())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .context("failed to build tokio runtime")?;
                rt.block_on(async move {
                    tun2proxy::general_run_async(args, mtu, false, shutdown)
                        .await
                        .map_err(|err| anyhow!("tun2proxy runtime failed: {err}"))
                })
            })
            .context("failed to start tun2proxy worker thread")?;
        thread::sleep(Duration::from_millis(100));
        Ok(thread)
    }

    fn build_tun2proxy_args(
        tun_fd: i32,
        proxy: ArgProxy,
        config: &EffectiveConfig,
    ) -> Tun2ProxyArgs {
        let mut args = Tun2ProxyArgs::default();
        args.proxy(proxy)
            .tun_fd(Some(tun_fd))
            .close_fd_on_drop(true)
            .setup(false)
            .dns(ArgDns::OverTcp)
            .ipv6_enabled(config.ipv6);
        args.mtu = config.mtu;
        args.dns_addr = config
            .dns
            .map(|addr| addr.ip())
            .unwrap_or(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)));
        args.verbosity = if config.quiet {
            ArgVerbosity::Off
        } else {
            match config.log_level.as_deref() {
                Some("error") => ArgVerbosity::Error,
                Some("warn") => ArgVerbosity::Warn,
                Some("debug") => ArgVerbosity::Debug,
                Some("trace") => ArgVerbosity::Trace,
                Some("off") => ArgVerbosity::Off,
                _ => ArgVerbosity::Info,
            }
        };
        args
    }

    fn should_block_udp(config: &EffectiveConfig, proxy_type: ProxyType) -> bool {
        matches!(proxy_type, ProxyType::Http) || matches!(config.udp, UdpMode::Off)
    }

    fn add_udp_reject_rule() -> anyhow::Result<()> {
        let status = Command::new("iptables")
            .args([
                "-w", "-A", "OUTPUT", "-p", "udp", "!", "--dport", "53", "-j", "REJECT",
            ])
            .status();
        match status {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => bail!("iptables failed while installing UDP fail-closed rule: {status}"),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                bail!("iptables is required to fail closed for HTTP proxy or udp=off")
            }
            Err(err) => Err(err).context("failed to run iptables"),
        }
    }

    fn configure_namespace_netlink(config: &EffectiveConfig) -> anyhow::Result<()> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("failed to build netlink runtime")?;
        rt.block_on(async {
            let (connection, handle, _) =
                rtnetlink::new_connection().context("failed to open rtnetlink connection")?;
            tokio::spawn(connection);

            let lo_index = link_index(&handle, "lo").await?;
            let tun_index = link_index(&handle, &config.tun_name).await?;

            set_link_up(&handle, lo_index).await?;
            add_addr_ignore_exists(&handle, lo_index, IpAddr::V4(TUN_GATEWAY), 32).await?;
            add_addr_ignore_exists(&handle, tun_index, IpAddr::V4(TUN_ADDR), 24).await?;
            set_link_up(&handle, tun_index).await?;
            add_default_route_ignore_exists(&handle, tun_index).await?;
            Ok::<_, anyhow::Error>(())
        })
    }

    async fn link_index(handle: &rtnetlink::Handle, name: &str) -> anyhow::Result<u32> {
        let link = handle
            .link()
            .get()
            .match_name(name.to_string())
            .execute()
            .try_next()
            .await
            .with_context(|| format!("failed to query link {name}"))?
            .ok_or_else(|| anyhow!("link not found: {name}"))?;
        Ok(link.header.index)
    }

    async fn set_link_up(handle: &rtnetlink::Handle, index: u32) -> anyhow::Result<()> {
        let msg = rtnetlink::LinkMessageBuilder::<rtnetlink::LinkUnspec>::new()
            .index(index)
            .up()
            .build();
        ignore_exists_or_execute(handle.link().set(msg).execute().await)
    }

    async fn add_addr_ignore_exists(
        handle: &rtnetlink::Handle,
        index: u32,
        addr: IpAddr,
        prefix_len: u8,
    ) -> anyhow::Result<()> {
        ignore_exists_or_execute(
            handle
                .address()
                .add(index, addr, prefix_len)
                .execute()
                .await,
        )
    }

    async fn add_default_route_ignore_exists(
        handle: &rtnetlink::Handle,
        tun_index: u32,
    ) -> anyhow::Result<()> {
        let route = rtnetlink::RouteMessageBuilder::<Ipv4Addr>::new()
            .destination_prefix(Ipv4Addr::UNSPECIFIED, 0)
            .output_interface(tun_index)
            .build();
        ignore_exists_or_execute(handle.route().add(route).execute().await)
    }

    fn ignore_exists_or_execute(result: Result<(), rtnetlink::Error>) -> anyhow::Result<()> {
        match result {
            Ok(()) => Ok(()),
            Err(err) if format!("{err}").contains("File exists") => Ok(()),
            Err(err) => Err(anyhow!("netlink operation failed: {err}")),
        }
    }

    fn bind_dns_cache_socket() -> anyhow::Result<std::net::UdpSocket> {
        std::net::UdpSocket::bind((TUN_GATEWAY, 53))
            .with_context(|| format!("failed to bind DNS cache on {TUN_GATEWAY}:53"))
    }

    fn wait_for_child_ready(fd: &OwnedFd) -> anyhow::Result<()> {
        let mut byte = [0_u8; 1];
        let read_len = read(fd, &mut byte).context("failed to read child readiness")?;
        if read_len != 1 {
            bail!("child exited before namespace setup completed");
        }
        Ok(())
    }

    fn create_resolv_conf() -> anyhow::Result<PathBuf> {
        let path = std::env::temp_dir().join(format!("ptun-resolv-{}", std::process::id()));
        fs::write(&path, resolv_conf_contents())
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(path)
    }

    fn resolv_conf_contents() -> &'static str {
        "nameserver 10.0.0.1\noptions timeout:1 attempts:1\n"
    }

    fn exit_like_child(status: WaitStatus) -> ! {
        let code = match status {
            WaitStatus::Exited(_, code) => code,
            WaitStatus::Signaled(_, signal, _) => 128 + signal as i32,
            _ => 1,
        };
        std::process::exit(code.clamp(0, 255));
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn resolv_conf_points_to_tun_gateway_not_tun_local_ip() {
            assert_eq!(
                super::resolv_conf_contents(),
                "nameserver 10.0.0.1\noptions timeout:1 attempts:1\n"
            );
        }
    }
}

#[cfg(target_os = "linux")]
pub fn run(args: &RunArgs, config: &EffectiveConfig) -> anyhow::Result<()> {
    linux_runtime::run(args, config)
}

#[cfg(not(target_os = "linux"))]
pub fn run(_args: &RunArgs, config: &EffectiveConfig) -> anyhow::Result<()> {
    crate::diagnostics::validate_config(config)?;
    anyhow::bail!("ptun run is only supported on Linux")
}
