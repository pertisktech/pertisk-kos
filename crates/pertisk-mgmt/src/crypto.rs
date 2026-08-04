use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, Context};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rand::RngCore;

/// Encrypt plaintext with AES-256-GCM. Output: base64(nonce || ciphertext).
pub fn encrypt(key: &[u8], plaintext: &str) -> anyhow::Result<String> {
    let cipher = Aes256Gcm::new_from_slice(key).context("aes key")?;
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| anyhow!("encrypt: {e}"))?;
    let mut out = Vec::with_capacity(12 + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(B64.encode(out))
}

/// Decrypt value produced by [`encrypt`].
pub fn decrypt(key: &[u8], encoded: &str) -> anyhow::Result<String> {
    let raw = B64.decode(encoded).context("b64 decode")?;
    if raw.len() < 13 {
        anyhow::bail!("ciphertext too short");
    }
    let (nonce_bytes, ct) = raw.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(key).context("aes key")?;
    let nonce = Nonce::from_slice(nonce_bytes);
    let pt = cipher
        .decrypt(nonce, ct)
        .map_err(|e| anyhow!("decrypt: {e}"))?;
    String::from_utf8(pt).context("utf8")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn roundtrip() {
        let key = Sha256::digest(b"test-key").to_vec();
        let enc = encrypt(&key, "secret-token").unwrap();
        assert_ne!(enc, "secret-token");
        assert_eq!(decrypt(&key, &enc).unwrap(), "secret-token");
    }
}
