//! Catalog of prebuilt guest cloud disks (`pertisk-cloud-{arch}*.qcow2`).
//!
//! Mgmt does not compile images. Cluster create (`--skip-build`) reads this
//! directory. Operators upload via the UI or copy files into `images_dir`.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use serde::Serialize;

/// QCOW2 magic (`QFI\xfb`).
const QCOW_MAGIC: [u8; 4] = [0x51, 0x46, 0x49, 0xfb];

#[derive(Debug, Clone, Serialize)]
pub struct CloudImage {
    pub name: String,
    pub arch: String,
    pub size_bytes: u64,
    /// `base`, `50g`, … when the name is `pertisk-cloud-{arch}[-{role}].qcow2`.
    pub role: String,
    pub is_default: bool,
    /// File birth time when the FS has it, otherwise mtime (RFC 3339).
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Catalog {
    pub dir: String,
    pub images: Vec<CloudImage>,
    pub ready: Ready,
}

#[derive(Debug, Clone, Serialize)]
pub struct Ready {
    pub amd64: bool,
    pub arm64: bool,
}

pub fn missing_message(arch: &str) -> String {
    format!(
        "missing cloud image for arch={arch}; upload pertisk-cloud-{arch}.qcow2 on Images \
         (or copy it into the mgmt images directory)"
    )
}

pub fn list(dir: &Path) -> Catalog {
    let _ = fs::create_dir_all(dir);
    let mut images = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for ent in rd.flatten() {
            let path = ent.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if !name.to_ascii_lowercase().ends_with(".qcow2") {
                continue;
            }
            if name.starts_with('.') {
                continue;
            }
            let meta = ent.metadata().ok();
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let created_at = meta.as_ref().and_then(file_created_at);
            let arch =
                crate::os_upgrade::infer_arch_from_name(name).unwrap_or_else(|| "amd64".into());
            let role = role_from_name(name, &arch);
            let is_default = name.eq_ignore_ascii_case(&format!("pertisk-cloud-{arch}.qcow2"));
            images.push(CloudImage {
                name: name.to_string(),
                arch,
                size_bytes: size,
                role,
                is_default,
                created_at,
            });
        }
    }
    images.sort_by(|a, b| a.arch.cmp(&b.arch).then(a.name.cmp(&b.name)));
    let ready = Ready {
        amd64: find_for_arch(dir, "amd64").is_some(),
        arm64: find_for_arch(dir, "arm64").is_some(),
    };
    Catalog {
        dir: dir.display().to_string(),
        images,
        ready,
    }
}

/// Lab-up prefers `pertisk-cloud-{arch}.qcow2`, then any `pertisk-cloud-{arch}*.qcow2`.
pub fn find_for_arch(dir: &Path, arch: &str) -> Option<PathBuf> {
    let default = dir.join(format!("pertisk-cloud-{arch}.qcow2"));
    if default.is_file() {
        return Some(default);
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return None;
    };
    let mut matches: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|n| {
                    let l = n.to_ascii_lowercase();
                    l.starts_with(&format!("pertisk-cloud-{arch}")) && l.ends_with(".qcow2")
                })
                .unwrap_or(false)
        })
        .collect();
    matches.sort();
    matches.into_iter().next()
}

pub fn sanitize_filename(name: &str) -> anyhow::Result<String> {
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        bail!("invalid image filename");
    }
    let base = Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .trim();
    if base.is_empty() || base.starts_with('.') {
        bail!("invalid image filename");
    }
    if !base.to_ascii_lowercase().ends_with(".qcow2") {
        bail!("file must be a .qcow2 image");
    }
    if !base
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        bail!("filename must be alphanumeric with . _ -");
    }
    Ok(base.to_string())
}

/// Keep `pertisk-cloud-*` names (including role-sized `*-50g.qcow2`); otherwise
/// store as the default disk for `arch`.
pub fn dest_name(original: &str, arch: &str) -> String {
    let lower = original.to_ascii_lowercase();
    if lower.starts_with("pertisk-cloud-") && lower.ends_with(".qcow2") {
        return original.to_string();
    }
    format!("pertisk-cloud-{arch}.qcow2")
}

pub fn verify_qcow2(path: &Path) -> anyhow::Result<()> {
    let mut f = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic)
        .context("image is too small to be qcow2")?;
    if magic != QCOW_MAGIC {
        bail!("file is not a qcow2 image (missing QFI magic)");
    }
    Ok(())
}

pub(crate) fn role_from_name(name: &str, arch: &str) -> String {
    let stem = name
        .strip_suffix(".qcow2")
        .or_else(|| name.strip_suffix(".QCOW2"))
        .unwrap_or(name);
    let prefix = format!("pertisk-cloud-{arch}");
    if stem.eq_ignore_ascii_case(&prefix) {
        return "base".into();
    }
    if let Some(rest) = stem
        .to_ascii_lowercase()
        .strip_prefix(&format!("{prefix}-"))
    {
        if !rest.is_empty() {
            return rest.to_string();
        }
    }
    "other".into()
}

pub(crate) fn file_created_at(meta: &fs::Metadata) -> Option<String> {
    let t = meta.created().or_else(|_| meta.modified()).ok()?;
    Some(chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn dest_keeps_pertisk_names() {
        assert_eq!(
            dest_name("pertisk-cloud-arm64-75g.qcow2", "amd64"),
            "pertisk-cloud-arm64-75g.qcow2"
        );
        assert_eq!(
            dest_name("disk.qcow2", "arm64"),
            "pertisk-cloud-arm64.qcow2"
        );
    }

    #[test]
    fn sanitize_rejects_paths() {
        assert!(sanitize_filename("../x.qcow2").is_err());
        assert!(sanitize_filename("foo.img").is_err());
        assert_eq!(
            sanitize_filename("pertisk-cloud-amd64.qcow2").unwrap(),
            "pertisk-cloud-amd64.qcow2"
        );
    }

    #[test]
    fn find_prefers_default_then_role_sized() {
        let dir = tempfile_dir();
        assert!(find_for_arch(&dir, "amd64").is_none());
        fs::write(dir.join("pertisk-cloud-amd64-50g.qcow2"), b"x").unwrap();
        let found = find_for_arch(&dir, "amd64").unwrap();
        assert!(found.ends_with("pertisk-cloud-amd64-50g.qcow2"));
        fs::write(dir.join("pertisk-cloud-amd64.qcow2"), b"y").unwrap();
        let found = find_for_arch(&dir, "amd64").unwrap();
        assert!(found.ends_with("pertisk-cloud-amd64.qcow2"));
        assert!(find_for_arch(&dir, "arm64").is_none());
        let catalog = list(&dir);
        let img = catalog
            .images
            .iter()
            .find(|i| i.name == "pertisk-cloud-amd64.qcow2")
            .unwrap();
        assert!(img.created_at.as_deref().is_some_and(|s| s.contains('T')));
    }

    #[test]
    fn verify_qcow2_magic() {
        let dir = tempfile_dir();
        let path = dir.join("bad.qcow2");
        fs::write(&path, b"not-qcow").unwrap();
        assert!(verify_qcow2(&path).is_err());
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(&QCOW_MAGIC).unwrap();
        f.write_all(&[0u8; 16]).unwrap();
        drop(f);
        verify_qcow2(&path).unwrap();
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pertisk-img-{}", std::process::id()));
        let dir = dir.join(format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
