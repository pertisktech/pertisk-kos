//! OS upgrade bundle: manifest + artifact hashes + detached signature.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::sign::{load_verifying_key, verify_manifest, KeyError};

#[derive(Debug, Error)]
pub enum BundleError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Key(#[from] KeyError),
    #[error("hash mismatch for {name}: expected {expected}, got {actual}")]
    HashMismatch {
        name: String,
        expected: String,
        actual: String,
    },
    #[error("{0}")]
    Msg(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleManifest {
    pub version: String,
    /// Relative artifact paths → sha256 hex.
    pub artifacts: BTreeMap<String, String>,
}

impl BundleManifest {
    pub fn to_canonical_json(&self) -> Result<Vec<u8>, BundleError> {
        // BTreeMap keeps key order stable for signing.
        Ok(serde_json::to_vec(self)?)
    }
}

#[derive(Debug, Clone)]
pub struct VerifiedBundle {
    pub root: PathBuf,
    pub manifest: BundleManifest,
}

/// Build a manifest by hashing files under `bundle_dir` listed in `artifact_names`.
pub fn build_manifest(
    bundle_dir: &Path,
    version: &str,
    artifact_names: &[&str],
) -> Result<BundleManifest, BundleError> {
    let mut artifacts = BTreeMap::new();
    for name in artifact_names {
        let path = bundle_dir.join(name);
        let hash = sha256_file(&path)?;
        artifacts.insert((*name).to_string(), hash);
    }
    Ok(BundleManifest {
        version: version.to_string(),
        artifacts,
    })
}

pub fn sha256_file(path: &Path) -> Result<String, BundleError> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Load and verify a bundle directory containing `manifest.json` + `manifest.sig`.
pub fn verify_bundle(
    bundle_dir: &Path,
    public_key_path: &Path,
) -> Result<VerifiedBundle, BundleError> {
    let manifest_path = bundle_dir.join("manifest.json");
    let sig_path = bundle_dir.join("manifest.sig");
    let manifest_bytes = fs::read(&manifest_path)?;
    let signature = fs::read_to_string(&sig_path)?;
    let verifying = load_verifying_key(public_key_path)?;
    verify_manifest(&verifying, &manifest_bytes, signature.trim())?;

    let manifest: BundleManifest = serde_json::from_slice(&manifest_bytes)?;
    for (name, expected) in &manifest.artifacts {
        let path = bundle_dir.join(name);
        let actual = sha256_file(&path)?;
        if &actual != expected {
            return Err(BundleError::HashMismatch {
                name: name.clone(),
                expected: expected.clone(),
                actual,
            });
        }
    }

    Ok(VerifiedBundle {
        root: bundle_dir.to_path_buf(),
        manifest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign::{generate_keypair, load_signing_key, sign_manifest};
    use tempfile::tempdir;

    #[test]
    fn verify_signed_bundle() {
        let dir = tempdir().unwrap();
        let bundle = dir.path().join("bundle");
        fs::create_dir_all(&bundle).unwrap();
        fs::write(bundle.join("kernel"), b"vmlinuz-bytes").unwrap();
        fs::write(bundle.join("initramfs"), b"initrd-bytes").unwrap();

        let sk = dir.path().join("keys/os.sk");
        let pk = dir.path().join("keys/os.pk");
        generate_keypair(&sk, &pk).unwrap();

        let manifest = build_manifest(&bundle, "0.2.0", &["kernel", "initramfs"]).unwrap();
        let json = manifest.to_canonical_json().unwrap();
        fs::write(bundle.join("manifest.json"), &json).unwrap();
        let sig = sign_manifest(&load_signing_key(&sk).unwrap(), &json);
        fs::write(bundle.join("manifest.sig"), sig).unwrap();

        let verified = verify_bundle(&bundle, &pk).unwrap();
        assert_eq!(verified.manifest.version, "0.2.0");
    }
}
