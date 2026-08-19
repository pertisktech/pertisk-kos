//! TPM PCR attestation via Linux sysfs (lab path; no libtss2).
//!
//! Reads `/sys/class/tpm/<dev>/pcr-sha256/{N}` for firmware/UKI-relevant indices.

use std::fs;
use std::path::{Path, PathBuf};

/// PCR indices reported by Attest (firmware 0–7 + UKI stub 11 when present).
pub const ATTEST_PCR_INDICES: &[u32] = &[0, 1, 2, 3, 4, 5, 6, 7, 11];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcrDigest {
    pub index: u32,
    pub algo: String,
    pub digest_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestSnapshot {
    pub available: bool,
    pub message: String,
    pub pcrs: Vec<PcrDigest>,
    /// Sysfs TPM device directory used (e.g. `/sys/class/tpm/tpm0`), if any.
    pub tpm_sysfs: Option<PathBuf>,
}

/// Read SHA-256 PCRs from a sysfs class root (normally `/sys/class/tpm`).
pub fn read_pcrs_from_sysfs(tpm_class: &Path) -> AttestSnapshot {
    let Some(dev) = find_tpm_device(tpm_class) else {
        return AttestSnapshot {
            available: false,
            message: format!("no TPM under {}", tpm_class.display()),
            pcrs: Vec::new(),
            tpm_sysfs: None,
        };
    };

    let bank = dev.join("pcr-sha256");
    if !bank.is_dir() {
        return AttestSnapshot {
            available: false,
            message: format!(
                "TPM at {} has no pcr-sha256 bank (kernel too old?)",
                dev.display()
            ),
            pcrs: Vec::new(),
            tpm_sysfs: Some(dev),
        };
    }

    let mut pcrs = Vec::new();
    for &index in ATTEST_PCR_INDICES {
        let path = bank.join(index.to_string());
        match fs::read_to_string(&path) {
            Ok(raw) => {
                let digest_hex = normalize_digest(&raw);
                if digest_hex.is_empty() {
                    continue;
                }
                pcrs.push(PcrDigest {
                    index,
                    algo: "sha256".into(),
                    digest_hex,
                });
            }
            Err(_) => continue,
        }
    }

    if pcrs.is_empty() {
        return AttestSnapshot {
            available: false,
            message: format!(
                "TPM at {} present but no PCR digests readable",
                dev.display()
            ),
            pcrs,
            tpm_sysfs: Some(dev),
        };
    }

    AttestSnapshot {
        available: true,
        message: format!("read {} SHA-256 PCR(s) from {}", pcrs.len(), bank.display()),
        pcrs,
        tpm_sysfs: Some(dev),
    }
}

/// Host default: `/sys/class/tpm`.
pub fn read_host_pcrs() -> AttestSnapshot {
    read_pcrs_from_sysfs(Path::new("/sys/class/tpm"))
}

fn find_tpm_device(tpm_class: &Path) -> Option<PathBuf> {
    if !tpm_class.is_dir() {
        return None;
    }
    let preferred = tpm_class.join("tpm0");
    if preferred.is_dir() {
        return Some(preferred);
    }
    let mut entries: Vec<_> = fs::read_dir(tpm_class)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("tpm"))
                && p.is_dir()
        })
        .collect();
    entries.sort();
    entries.into_iter().next()
}

fn normalize_digest(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn missing_tpm_class() {
        let dir = tempdir().unwrap();
        let snap = read_pcrs_from_sysfs(&dir.path().join("missing"));
        assert!(!snap.available);
        assert!(snap.pcrs.is_empty());
        assert!(snap.message.contains("no TPM"));
    }

    #[test]
    fn reads_selected_pcrs() {
        let dir = tempdir().unwrap();
        let tpm0 = dir.path().join("tpm0");
        let bank = tpm0.join("pcr-sha256");
        fs::create_dir_all(&bank).unwrap();
        // 32-byte zero digest hex
        let zero = "0".repeat(64);
        fs::write(bank.join("0"), format!("{zero}\n")).unwrap();
        fs::write(bank.join("7"), format!(" AABBCCDD {}\n", &zero[8..])).unwrap();
        fs::write(bank.join("11"), &zero).unwrap();
        // Not in ATTEST_PCR_INDICES — ignored
        fs::write(bank.join("16"), &zero).unwrap();

        let snap = read_pcrs_from_sysfs(dir.path());
        assert!(snap.available);
        assert_eq!(snap.pcrs.len(), 3);
        assert_eq!(snap.pcrs[0].index, 0);
        assert_eq!(snap.pcrs[0].algo, "sha256");
        assert_eq!(snap.pcrs[0].digest_hex, zero);
        assert_eq!(snap.pcrs[1].index, 7);
        assert!(snap.pcrs[1].digest_hex.starts_with("aabbccdd"));
        assert_eq!(snap.pcrs[2].index, 11);
    }

    #[test]
    fn prefers_tpm0() {
        let dir = tempdir().unwrap();
        let bank1 = dir.path().join("tpm1").join("pcr-sha256");
        let bank0 = dir.path().join("tpm0").join("pcr-sha256");
        fs::create_dir_all(&bank1).unwrap();
        fs::create_dir_all(&bank0).unwrap();
        let digest = "a".repeat(64);
        fs::write(bank1.join("0"), "b".repeat(64)).unwrap();
        fs::write(bank0.join("0"), &digest).unwrap();
        let snap = read_pcrs_from_sysfs(dir.path());
        assert_eq!(snap.pcrs[0].digest_hex, digest);
        assert!(snap.tpm_sysfs.unwrap().ends_with("tpm0"));
    }
}
