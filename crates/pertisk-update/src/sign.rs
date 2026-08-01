//! Ed25519 signing helpers for upgrade manifests.

use std::fs;
use std::path::Path;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum KeyError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid key: {0}")]
    Invalid(String),
    #[error("signature verification failed")]
    VerifyFailed,
}

/// Generate a new Ed25519 keypair; writes 32-byte seeds/keys as hex files.
pub fn generate_keypair(secret_path: &Path, public_path: &Path) -> Result<(), KeyError> {
    let signing = SigningKey::generate(&mut OsRng);
    let verifying = signing.verifying_key();
    if let Some(parent) = secret_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = public_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(secret_path, hex::encode(signing.to_bytes()))?;
    fs::write(public_path, hex::encode(verifying.to_bytes()))?;
    Ok(())
}

pub fn load_signing_key(path: &Path) -> Result<SigningKey, KeyError> {
    let hex_str = fs::read_to_string(path)?;
    let bytes = hex::decode(hex_str.trim()).map_err(|e| KeyError::Invalid(e.to_string()))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| KeyError::Invalid("signing key must be 32 bytes".into()))?;
    Ok(SigningKey::from_bytes(&arr))
}

pub fn load_verifying_key(path: &Path) -> Result<VerifyingKey, KeyError> {
    let hex_str = fs::read_to_string(path)?;
    let bytes = hex::decode(hex_str.trim()).map_err(|e| KeyError::Invalid(e.to_string()))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| KeyError::Invalid("public key must be 32 bytes".into()))?;
    VerifyingKey::from_bytes(&arr).map_err(|e| KeyError::Invalid(e.to_string()))
}

/// Sign canonical manifest JSON bytes; returns hex signature.
pub fn sign_manifest(signing_key: &SigningKey, manifest_json: &[u8]) -> String {
    let sig = signing_key.sign(manifest_json);
    hex::encode(sig.to_bytes())
}

pub fn verify_manifest(
    verifying_key: &VerifyingKey,
    manifest_json: &[u8],
    signature_hex: &str,
) -> Result<(), KeyError> {
    let bytes = hex::decode(signature_hex.trim()).map_err(|e| KeyError::Invalid(e.to_string()))?;
    let arr: [u8; 64] = bytes
        .try_into()
        .map_err(|_| KeyError::Invalid("signature must be 64 bytes".into()))?;
    let sig = Signature::from_bytes(&arr);
    verifying_key
        .verify(manifest_json, &sig)
        .map_err(|_| KeyError::VerifyFailed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn sign_and_verify() {
        let dir = tempdir().unwrap();
        let sk = dir.path().join("key.sk");
        let pk = dir.path().join("key.pk");
        generate_keypair(&sk, &pk).unwrap();
        let signing = load_signing_key(&sk).unwrap();
        let verifying = load_verifying_key(&pk).unwrap();
        let msg = br#"{"version":"1.0.0"}"#;
        let sig = sign_manifest(&signing, msg);
        verify_manifest(&verifying, msg, &sig).unwrap();
        assert!(verify_manifest(&verifying, br#"{"version":"nope"}"#, &sig).is_err());
    }
}
