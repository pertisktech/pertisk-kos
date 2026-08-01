//! Hostname application.

use anyhow::Result;
use tracing::info;

pub fn set_hostname(name: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux_impl::set(name)
    }
    #[cfg(not(target_os = "linux"))]
    {
        info!(hostname = name, "hostname set (dev log only)");
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use super::*;
    use std::fs;
    use std::io::Write;

    use nix::unistd;

    pub fn set(name: &str) -> Result<()> {
        unistd::sethostname(name)?;
        // Best-effort persist for userspace tools that read the file.
        if let Ok(mut f) = fs::File::create("/etc/hostname") {
            let _ = writeln!(f, "{name}");
        }
        info!(hostname = name, "hostname applied");
        Ok(())
    }
}
