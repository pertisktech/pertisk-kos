//! Produce a Quote signed by the lab persistent ECC AK.

use crate::commands;
use crate::device::Device;
use crate::ek::{read_ek_certificate, EkCertificate, EkChainStatus};
use crate::error::{Error, Result};
use crate::verify::PcrDigest;
use crate::wire::AK_PERSISTENT_HANDLE;

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
    /// TPMT_PUBLIC bytes for the persistent AK.
    pub ak_public: Vec<u8>,
    pub device: Option<String>,
    /// Persistent handle used (e.g. 0x8100000A).
    pub ak_handle: u32,
    /// Manufacturer EK certificate (when present in TPM NV).
    pub ek: EkCertificate,
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
            ak_handle: 0,
            ek: EkCertificate {
                available: false,
                message: String::new(),
                nv_index: 0,
                der: Vec::new(),
                subject: String::new(),
                issuer: String::new(),
                fingerprint_sha256: String::new(),
                chain_status: EkChainStatus::Missing,
                chain_message: String::new(),
            },
        }
    }
}

/// Ensure persistent AK, Quote selected PCRs, attach EK cert. Soft-fails without TPM.
pub fn produce_quote(nonce: &[u8]) -> QuoteBundle {
    match produce_quote_inner(nonce) {
        Ok(b) => b,
        Err(Error::NoDevice) => {
            QuoteBundle::unavailable("no TPM device (/dev/tpmrm0 or /dev/tpm0)")
        }
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

    let ak = commands::ensure_persistent_ak(&mut dev)?;
    let (quoted, signature) = commands::quote(&mut dev, ak.handle, &nonce, QUOTE_PCR_INDICES)?;
    // Drop device before opening again inside EK read (exclusive /dev/tpmrm0).
    drop(dev);
    let ek = read_ek_certificate(None);

    let mut message = format!(
        "quoted {} PCR(s) via {path} (persistent AK handle 0x{AK_PERSISTENT_HANDLE:08x})",
        QUOTE_PCR_INDICES.len()
    );
    if ek.available {
        message.push_str(&format!(
            "; EK NV 0x{:08x} chain={}",
            ek.nv_index,
            ek.chain_status.as_str()
        ));
    } else if !ek.message.is_empty() {
        message.push_str(&format!("; EK: {}", ek.message));
    }

    Ok(QuoteBundle {
        available: true,
        message,
        nonce,
        quoted,
        signature,
        ak_public: ak.public,
        device: Some(path),
        ak_handle: ak.handle,
        ek,
    })
}

fn getrandom_nonce() -> Vec<u8> {
    let mut buf = vec![0u8; 32];
    if fill_random(&mut buf).is_err() {
        buf = (0..32)
            .map(|i| (i as u8).wrapping_mul(17).wrapping_add(0x5a))
            .collect();
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
