//! Signed A/B OS bundle helpers for mgmt (upload + hostPath staging).

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use anyhow::{anyhow, bail, Context};
use serde::Deserialize;

/// Guest path where the staging pod writes the bundle (EPHEMERAL `/var`).
pub const HOST_BUNDLE_DIR: &str = "/var/lib/pertisk-os-upgrade";

/// Guest path for the Ed25519 public trust key (STATE).
pub const HOST_TRUST_PK: &str = "/system/state/secrets/os-trust.pk";

pub const REQUIRED_FILES: &[&str] = &["kernel", "initramfs", "manifest.json", "manifest.sig"];
pub const TRUST_PK_NAME: &str = "os-trust.pk";

#[derive(Debug, Deserialize)]
struct ManifestFile {
    version: String,
}

/// Read `version` from `manifest.json` in a bundle directory.
pub fn bundle_version(dir: &Path) -> anyhow::Result<String> {
    let raw = fs::read_to_string(dir.join("manifest.json")).context("read manifest.json")?;
    let m: ManifestFile = serde_json::from_str(&raw).context("parse manifest.json")?;
    let v = m.version.trim();
    if v.is_empty() {
        bail!("manifest.json version is empty");
    }
    Ok(v.to_string())
}

/// Ensure the four signed-bundle files exist and are non-empty.
pub fn validate_bundle_dir(dir: &Path) -> anyhow::Result<String> {
    for name in REQUIRED_FILES {
        let p = dir.join(name);
        let meta = fs::metadata(&p).with_context(|| format!("missing {name}"))?;
        if !meta.is_file() || meta.len() == 0 {
            bail!("{name} is empty");
        }
    }
    bundle_version(dir)
}

/// Map an archive member basename onto a required bundle filename.
pub fn canonical_bundle_name(basename: &str) -> Option<&'static str> {
    let b = basename.trim();
    if b.eq_ignore_ascii_case("kernel") || b.eq_ignore_ascii_case("bzImage") {
        return Some("kernel");
    }
    if b.eq_ignore_ascii_case("initramfs")
        || b.eq_ignore_ascii_case("initramfs.cpio.gz")
        || b.starts_with("initramfs-")
    {
        return Some("initramfs");
    }
    if b.eq_ignore_ascii_case("manifest.json") {
        return Some("manifest.json");
    }
    if b.eq_ignore_ascii_case("manifest.sig") {
        return Some("manifest.sig");
    }
    if b.eq_ignore_ascii_case("os-trust.pk") || b.eq_ignore_ascii_case("os-trust.pub") {
        return Some(TRUST_PK_NAME);
    }
    None
}

/// Extract a zip of a signed bundle (files may sit in a subdirectory).
pub fn extract_bundle_zip(zip_path: &Path, dest: &Path) -> anyhow::Result<String> {
    fs::create_dir_all(dest)?;
    let file = File::open(zip_path).context("open zip")?;
    let mut archive = zip::ZipArchive::new(file).context("read zip")?;
    let dest_canon = dest
        .canonicalize()
        .with_context(|| format!("canonicalize {}", dest.display()))?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if !entry.is_file() {
            continue;
        }
        let Some(enclosed) = entry.enclosed_name() else {
            continue;
        };
        let Some(base) = enclosed.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(canon) = canonical_bundle_name(base) else {
            continue;
        };
        let out_path = dest.join(canon);
        let parent = out_path.parent().unwrap_or(dest);
        fs::create_dir_all(parent)?;
        let out_canon = parent.canonicalize().unwrap_or_else(|_| dest_canon.clone());
        if !out_canon.starts_with(&dest_canon) {
            bail!("refusing zip path outside dest");
        }
        let mut out = File::create(&out_path)?;
        io::copy(&mut entry, &mut out)?;
        out.flush()?;
    }
    validate_bundle_dir(dest)
}

/// Write one uploaded file into `dest` using a canonical bundle name.
pub fn write_bundle_file(dest: &Path, original_name: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let base = Path::new(original_name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(original_name);
    let name = canonical_bundle_name(base)
        .ok_or_else(|| anyhow!("unrecognized bundle file {original_name}"))?;
    fs::create_dir_all(dest)?;
    if bytes.is_empty() {
        bail!("{name} is empty");
    }
    fs::write(dest.join(name), bytes)?;
    Ok(())
}

/// Parse `pertiskctl upgrade-status` stdout for `version=`.
pub fn parse_upgrade_status_version(stdout: &str) -> Option<String> {
    for part in stdout.split_whitespace() {
        if let Some(v) = part.strip_prefix("version=") {
            let v = v.trim();
            if !v.is_empty() && v != "-" {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Normalize guest arch (`amd64` | `arm64`).
pub fn normalize_arch(raw: &str) -> anyhow::Result<String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "amd64" | "x86_64" | "x64" => Ok("amd64".into()),
        "arm64" | "aarch64" => Ok("arm64".into()),
        other if other.is_empty() => Ok("amd64".into()),
        other => bail!("arch must be amd64 or arm64 (got {other})"),
    }
}

/// Guess arch from a zip or original filename (`os-bundle-amd64-v0.2.87.zip`).
pub fn infer_arch_from_name(name: &str) -> Option<String> {
    let l = name.to_ascii_lowercase();
    if l.contains("arm64") || l.contains("aarch64") {
        return Some("arm64".into());
    }
    if l.contains("amd64") || l.contains("x86_64") {
        return Some("amd64".into());
    }
    None
}

/// Sum file sizes in a bundle directory (non-recursive).
pub fn dir_size_bytes(dir: &Path) -> u64 {
    let Ok(rd) = fs::read_dir(dir) else {
        return 0;
    };
    rd.filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

/// Copy signed-bundle files from `src` into `dest` (replaces dest contents).
pub fn copy_bundle_dir(src: &Path, dest: &Path) -> anyhow::Result<()> {
    validate_bundle_dir(src)?;
    if dest.exists() {
        fs::remove_dir_all(dest).with_context(|| format!("remove {}", dest.display()))?;
    }
    fs::create_dir_all(dest)?;
    for name in REQUIRED_FILES
        .iter()
        .copied()
        .chain(std::iter::once(TRUST_PK_NAME))
    {
        let from = src.join(name);
        if from.is_file() {
            fs::copy(&from, dest.join(name)).with_context(|| format!("copy {name}"))?;
        }
    }
    Ok(())
}

/// Optional `os-trust.pk` sitting next to the signed artifacts.
pub fn bundle_trust_pk(dir: &Path) -> Option<std::path::PathBuf> {
    let p = dir.join(TRUST_PK_NAME);
    match fs::metadata(&p) {
        Ok(m) if m.is_file() && m.len() > 0 => Some(p),
        _ => None,
    }
}

/// Ensure the bundle dir has `os-trust.pk` (from the zip, or a mgmt-host fallback).
pub fn ensure_trust_pk(
    bundle: &Path,
    fallbacks: &[std::path::PathBuf],
) -> anyhow::Result<std::path::PathBuf> {
    if let Some(p) = bundle_trust_pk(bundle) {
        return Ok(p);
    }
    let dest = bundle.join(TRUST_PK_NAME);
    for src in fallbacks {
        if src.is_file() {
            fs::copy(src, &dest)
                .with_context(|| format!("copy {} → {}", src.display(), dest.display()))?;
            return Ok(dest);
        }
    }
    bail!(
        "upgrade trust key missing: include os-trust.pk in the zip (`make os-bundle` adds it), \
         or copy the public key to the mgmt host (MGMT_OS_TRUST_PK / data_dir/secrets/os-trust.pk)"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    #[test]
    fn canonical_names() {
        assert_eq!(canonical_bundle_name("kernel"), Some("kernel"));
        assert_eq!(canonical_bundle_name("bzImage"), Some("kernel"));
        assert_eq!(
            canonical_bundle_name("initramfs-amd64.cpio.gz"),
            Some("initramfs")
        );
        assert_eq!(
            canonical_bundle_name("manifest.json"),
            Some("manifest.json")
        );
        assert_eq!(canonical_bundle_name("os-trust.pk"), Some("os-trust.pk"));
        assert_eq!(canonical_bundle_name("readme.txt"), None);
    }

    #[test]
    fn validate_and_version() {
        let dir = tempfile::tempdir().unwrap();
        for name in REQUIRED_FILES {
            fs::write(dir.path().join(name), b"x").unwrap();
        }
        fs::write(
            dir.path().join("manifest.json"),
            r#"{"version":"0.2.86","artifacts":{}}"#,
        )
        .unwrap();
        assert_eq!(validate_bundle_dir(dir.path()).unwrap(), "0.2.86");
    }

    #[test]
    fn extract_nested_zip() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("b.zip");
        {
            let f = File::create(&zip_path).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let opts = SimpleFileOptions::default();
            z.start_file("os-bundle/kernel", opts).unwrap();
            z.write_all(b"k").unwrap();
            z.start_file("os-bundle/initramfs", opts).unwrap();
            z.write_all(b"i").unwrap();
            z.start_file("os-bundle/manifest.json", opts).unwrap();
            z.write_all(br#"{"version":"1.2.3"}"#).unwrap();
            z.start_file("os-bundle/manifest.sig", opts).unwrap();
            z.write_all(b"sig").unwrap();
            z.start_file("os-bundle/os-trust.pk", opts).unwrap();
            z.write_all(b"pk").unwrap();
            z.finish().unwrap();
        }
        let dest = tmp.path().join("out");
        assert_eq!(extract_bundle_zip(&zip_path, &dest).unwrap(), "1.2.3");
        assert!(dest.join("kernel").is_file());
        assert_eq!(fs::read(dest.join("os-trust.pk")).unwrap(), b"pk");
    }

    #[test]
    fn parse_status_line() {
        let line =
            "active=A next=B previous_good=A boot_ok=true attempts=0 version=0.2.86 pending=";
        assert_eq!(
            parse_upgrade_status_version(line).as_deref(),
            Some("0.2.86")
        );
    }

    #[test]
    fn infer_arch_from_bundle_zip_name() {
        assert_eq!(
            infer_arch_from_name("os-bundle-amd64-v0.2.87.zip").as_deref(),
            Some("amd64")
        );
        assert_eq!(
            infer_arch_from_name("os-bundle-arm64-v0.2.87.zip").as_deref(),
            Some("arm64")
        );
        assert_eq!(infer_arch_from_name("bundle.zip"), None);
        assert_eq!(normalize_arch("x86_64").unwrap(), "amd64");
        assert_eq!(normalize_arch("aarch64").unwrap(), "arm64");
    }
}
