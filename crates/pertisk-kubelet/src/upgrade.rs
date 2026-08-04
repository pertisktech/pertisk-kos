//! Download / replace kubelet when `cluster.kubernetesVersion` changes.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use tracing::{info, warn};

use crate::paths::KubeletPaths;

/// Ensure `/usr/local/bin/kubelet` matches `want` (e.g. `v1.36.3`).
/// Returns `true` when the binary was replaced (caller should restart kubelet).
pub fn ensure_kubelet_version(paths: &KubeletPaths, want: &str) -> Result<bool> {
    let want = normalize_k8s_version(want);
    if let Some(cur) = read_kubelet_version(&paths.binary) {
        if cur == want {
            return Ok(false);
        }
        info!(current = %cur, target = %want, "kubelet version mismatch; downloading");
    } else {
        info!(target = %want, "kubelet version unknown; downloading");
    }

    let arch = host_arch();
    let url = format!("https://dl.k8s.io/release/{want}/bin/linux/{arch}/kubelet");
    let parent = paths
        .binary
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new("/usr/local/bin").to_path_buf());
    fs::create_dir_all(&parent)?;
    let tmp = parent.join(format!(".kubelet.{want}.tmp"));
    download_to(&url, &tmp)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&tmp)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&tmp, perms)?;
    }

    // Sanity-check the downloaded binary before swapping.
    if let Some(got) = read_kubelet_version(&tmp) {
        if got != want {
            let _ = fs::remove_file(&tmp);
            bail!("downloaded kubelet reports {got}, expected {want}");
        }
    } else {
        warn!("could not exec downloaded kubelet --version; installing anyway");
    }

    fs::rename(&tmp, &paths.binary)
        .with_context(|| format!("install kubelet → {}", paths.binary.display()))?;
    info!(path = %paths.binary.display(), version = %want, "kubelet binary upgraded");
    Ok(true)
}

pub fn read_kubelet_version(bin: &Path) -> Option<String> {
    let out = Command::new(bin).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    // "Kubernetes v1.36.2"
    s.split_whitespace()
        .find(|t| t.starts_with('v') && t.contains('.'))
        .map(|t| t.trim().to_string())
}

fn normalize_k8s_version(v: &str) -> String {
    let v = v.trim();
    if v.starts_with('v') {
        v.to_string()
    } else {
        format!("v{v}")
    }
}

fn host_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => {
            warn!(arch = other, "unknown arch; defaulting kubelet download to amd64");
            "amd64"
        }
    }
}

fn download_to(url: &str, dest: &Path) -> Result<()> {
    let resp = ureq::get(url)
        .timeout(std::time::Duration::from_secs(300))
        .call()
        .with_context(|| format!("GET {url}"))?;
    if resp.status() != 200 {
        bail!("GET {url} → HTTP {}", resp.status());
    }
    let mut reader = resp.into_reader();
    let mut file = fs::File::create(dest)
        .with_context(|| format!("create {}", dest.display()))?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).context("read download body")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
    }
    file.flush()?;
    Ok(())
}
