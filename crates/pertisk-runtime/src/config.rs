//! Minimal containerd config (cgroupfs — no systemd on Pertisk).

use std::fs;
use std::io::Write;

use crate::paths::RuntimePaths;
use crate::process::RuntimeError;

pub fn write_containerd_config(paths: &RuntimePaths) -> Result<(), RuntimeError> {
    paths.ensure_dirs()?;
    let body = format!(
        r#"version = 2
root = "{root}"
state = "{state}"

[grpc]
  address = "{socket}"

[plugins]
  [plugins."io.containerd.grpc.v1.cri"]
    sandbox_image = "registry.k8s.io/pause:3.10"
    [plugins."io.containerd.grpc.v1.cri".containerd]
      [plugins."io.containerd.grpc.v1.cri".containerd.runtimes]
        [plugins."io.containerd.grpc.v1.cri".containerd.runtimes.runc]
          runtime_type = "io.containerd.runc.v2"
          [plugins."io.containerd.grpc.v1.cri".containerd.runtimes.runc.options]
            SystemdCgroup = false
            BinaryName = "/usr/local/bin/runc"
            # Host root is initramfs (rootfs); pivot_root(2) returns EINVAL there.
            NoPivotRoot = true
"#,
        root = paths.root.display(),
        state = paths.state.display(),
        socket = paths.socket.display(),
    );

    let mut f = fs::File::create(&paths.config)?;
    f.write_all(body.as_bytes())?;
    tracing::info!(path = %paths.config.display(), "wrote containerd config");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn writes_config_under_prefix() {
        let dir = tempfile_dir();
        let paths = RuntimePaths::with_prefix(&dir);
        write_containerd_config(&paths).unwrap();
        let text = fs::read_to_string(&paths.config).unwrap();
        assert!(text.contains("SystemdCgroup = false"));
        assert!(text.contains("BinaryName"));
        assert!(text.contains("NoPivotRoot = true"));
        assert!(text.contains("version = 2"));
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pertisk-runtime-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
