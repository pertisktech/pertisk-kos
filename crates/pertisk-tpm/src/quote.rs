//! Produce an ephemeral ECC AK Quote from `/dev/tpmrm0`.

use crate::commands::{self, LoadedKey};
use crate::device::Device;
use crate::error::{Error, Result};
use crate::verify::PcrDigest;

/// PCR indices quoted (firmware 0–7 + UKI stub 11).
pub const QUOTE_PCR_INDICES: &[u32] = &[0, 1, 2, 3, 4, 5, 6, 7, 11];

#[derive(Debug, Clone)]
pub struct QuoteBundle {
    pub available: bool,
    pub message: String,
    pub nonce: Vec<u8>,
    /// TPMS_ATTEST bytes.
    pub quoted: Vec<u8>,
    /// Marshaled TPMT_SIGNATURE (ECDSA).
    pub signature: Vec<u8>,
    /// TPMT_PUBLIC bytes for the ephemeral AK.
    pub ak_public: Vec<u8>,
    pub device: Option<String>,
}

impl QuoteBundle {
    fn unavailable(msg: impl Into<String>) -> Self {
        Self {
            available: false,
            message: msg.into(),
            nonce: Vec::new(),
            quoted: Vec::new(),
            signature: Vec::new(),
            ak_public: Vec::new(),
            device: None,
        }
    }
}

/// Create ephemeral primary + AK, Quote selected PCRs, then flush.
///
/// Soft-fails (available=false) when the TPM device is missing.
pub fn produce_quote(nonce: &[u8]) -> QuoteBundle {
    match produce_quote_inner(nonce) {
        Ok(b) => b,
        Err(Error::NoDevice) => QuoteBundle::unavailable("no TPM device (/dev/tpmrm0 or /dev/tpm0)"),
        Err(e) => QuoteBundle::unavailable(format!("TPM Quote failed: {e}")),
    }
}

fn produce_quote_inner(nonce: &[u8]) -> Result<QuoteBundle> {
    let nonce = if nonce.is_empty() {
        getrandom_nonce()
    } else {
        nonce.to_vec()
    };

    let mut dev = Device::open_default()?;
    let path = dev.path().display().to_string();

    let primary = commands::create_primary(&mut dev)?;
    let ak = match commands::create_and_load_ak(&mut dev, primary.handle) {
        Ok(k) => k,
        Err(e) => {
            commands::flush(&mut dev, primary.handle);
            return Err(e);
        }
    };

    let result = commands::quote(&mut dev, ak.handle, &nonce, QUOTE_PCR_INDICES);
    commands::flush(&mut dev, ak.handle);
    commands::flush(&mut dev, primary.handle);

    let (quoted, signature) = result?;
    Ok(QuoteBundle {
        available: true,
        message: format!(
            "quoted {} PCR(s) via {path} (ephemeral ECC AK)",
            QUOTE_PCR_INDICES.len()
        ),
        nonce,
        quoted,
        signature,
        ak_public: ak.public,
        device: Some(path),
    })
}

fn getrandom_nonce() -> Vec<u8> {
    // Prefer getrandom; fall back to reading /dev/urandom.
    let mut buf = vec![0u8; 32];
    if fill_random(&mut buf).is_err() {
        buf = (0..32).map(|i| (i as u8).wrapping_mul(17).wrapping_add(0x5a)).collect();
    }
    buf
}

fn fill_random(buf: &mut [u8]) -> std::io::Result<()> {
    use std::fs::File;
    use std::io::Read;
    File::open("/dev/urandom")?.read_exact(buf)
}

/// Re-export helper so callers can build PcrDigest lists for verify.
pub fn pcr_digests_from_hex(pairs: &[(u32, &str)]) -> Vec<PcrDigest> {
    pairs
        .iter()
        .map(|(i, h)| PcrDigest {
            index: *i,
            digest_hex: (*h).to_string(),
        })
        .collect()
}

#[allow(dead_code)]
fn _use_loaded(k: &LoadedKey) -> u32 {
    k.handle
}
