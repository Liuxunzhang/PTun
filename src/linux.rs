use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, anyhow};

#[derive(Debug)]
pub struct EnvironmentReport {
    pub is_linux: bool,
    pub is_root: bool,
    pub tun_available: bool,
    pub user_namespace_available: bool,
    pub network_namespace_available: bool,
}

impl EnvironmentReport {
    pub fn collect() -> Self {
        Self {
            is_linux: cfg!(target_os = "linux"),
            is_root: unsafe { libc_geteuid() == 0 },
            tun_available: Path::new("/dev/net/tun").exists(),
            user_namespace_available: Path::new("/proc/self/ns/user").exists(),
            network_namespace_available: Path::new("/proc/self/ns/net").exists(),
        }
    }

    pub fn validate_for_run(&self) -> anyhow::Result<()> {
        if !self.is_linux {
            return Err(anyhow!("ptun run is only supported on Linux"));
        }
        if !self.is_root {
            return Err(anyhow!(
                "ptun run requires root or CAP_NET_ADMIN; capability-only execution is not implemented yet"
            ));
        }
        if !self.tun_available {
            return Err(anyhow!("/dev/net/tun is not available"));
        }
        if !self.network_namespace_available {
            return Err(anyhow!("network namespaces are not available"));
        }
        Ok(())
    }
}

pub fn runtime_dir() -> anyhow::Result<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return Ok(std::path::PathBuf::from(dir).join("ptun"));
    }
    Ok(std::path::PathBuf::from("/run/ptun"))
}

#[derive(Clone, Debug)]
pub struct SessionRecord {
    pub pid: u32,
    pub command: String,
    pub proxy: String,
    pub tun_name: String,
}

impl SessionRecord {
    pub fn render(&self) -> String {
        format!(
            "pid={} tun={} proxy={} command={}",
            self.pid, self.tun_name, self.proxy, self.command
        )
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn write_session(record: &SessionRecord) -> anyhow::Result<PathBuf> {
    let dir = runtime_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join(format!("{}.session", record.pid));
    fs::write(
        &path,
        format!(
            "pid={}\ncommand={}\nproxy={}\ntun_name={}\n",
            record.pid, record.command, record.proxy, record.tun_name
        ),
    )
    .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn remove_session(path: &Path) {
    let _ = fs::remove_file(path);
}

pub fn read_session_files() -> anyhow::Result<Vec<SessionRecord>> {
    let dir = runtime_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            let raw = fs::read_to_string(entry.path())
                .with_context(|| format!("failed to read {}", entry.path().display()))?;
            if let Some(session) = parse_session(&raw)
                && process_alive(session.pid)
            {
                sessions.push(session);
            }
        }
    }
    sessions.sort_by_key(|session| session.pid);
    Ok(sessions)
}

fn parse_session(raw: &str) -> Option<SessionRecord> {
    let mut pid = None;
    let mut command = None;
    let mut proxy = None;
    let mut tun_name = None;
    for line in raw.lines() {
        let (key, value) = line.split_once('=')?;
        match key {
            "pid" => pid = value.parse().ok(),
            "command" => command = Some(value.to_string()),
            "proxy" => proxy = Some(value.to_string()),
            "tun_name" => tun_name = Some(value.to_string()),
            _ => {}
        }
    }
    Some(SessionRecord {
        pid: pid?,
        command: command?,
        proxy: proxy?,
        tun_name: tun_name?,
    })
}

fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

#[cfg(unix)]
unsafe fn libc_geteuid() -> u32 {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    unsafe { geteuid() }
}

#[cfg(not(unix))]
unsafe fn libc_geteuid() -> u32 {
    1
}
