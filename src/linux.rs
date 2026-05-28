use std::{fs, path::Path};

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

pub fn read_session_files() -> anyhow::Result<Vec<String>> {
    let dir = runtime_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            sessions.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    sessions.sort();
    Ok(sessions)
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
