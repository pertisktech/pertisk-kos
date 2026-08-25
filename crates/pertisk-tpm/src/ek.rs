//! Endorsement Key (EK) certificate read + manufacturer CA chain verify.
//!
//! Reads TCG NV indexes for the EK certificate (ECC preferred, then RSA),
//! parses the X.509 leaf, and optionally verifies it against PEM trust
//! anchors under `PERTISK_TPM_EK_CAS` or a caller-supplied directory.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use x509_parser::prelude::*;

use crate::commands;
use crate::device::Device;
use crate::error::{Error, Result};
use crate::wire::{NV_EK_CERT_ECC_P256, NV_EK_CERT_ECC_P384, NV_EK_CERT_RSA};

/// Preferred NV indexes for EK certificates (TCG EK Credential Profile).
pub const EK_CERT_NV_INDEXES: &[u32] = &[NV_EK_CERT_ECC_P256, NV_EK_CERT_ECC_P384, NV_EK_CERT_RSA];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EkChainStatus {
    /// Leaf present; no CA directory configured.
    Unverified,
    /// Chain verified to a configured manufacturer CA.
    Ok,
    /// Leaf present but failed to chain to any configured CA.
    Failed(String),
    /// No EK certificate found in NV (common on swtpm / unprovisioned TPM).
    Missing,
}

impl EkChainStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::Ok => "ok",
            Self::Failed(_) => "failed",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone)]
pub struct EkCertificate {
    pub available: bool,
    pub message: String,
    /// NV index that held the cert (`0` if none).
    pub nv_index: u32,
    /// Raw DER bytes.
    pub der: Vec<u8>,
    pub subject: String,
    pub issuer: String,
    /// SHA-256 of DER, lowercase hex (full 64 chars).
    pub fingerprint_sha256: String,
    pub chain_status: EkChainStatus,
    /// Short chain detail (CA subject or error).
    pub chain_message: String,
}

impl EkCertificate {
    fn missing(msg: impl Into<String>) -> Self {
        Self {
            available: false,
            message: msg.into(),
            nv_index: 0,
            der: Vec::new(),
            subject: String::new(),
            issuer: String::new(),
            fingerprint_sha256: String::new(),
            chain_status: EkChainStatus::Missing,
            chain_message: String::new(),
        }
    }

    /// Truncated fingerprint for UI (first 16 hex chars).
    pub fn fingerprint_short(&self) -> String {
        self.fingerprint_sha256.chars().take(16).collect()
    }
}

/// Resolve manufacturer CA directory: explicit override, then `PERTISK_TPM_EK_CAS`, then defaults.
pub fn resolve_ca_dir(override_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = override_dir {
        if p.is_dir() {
            return Some(p.to_path_buf());
        }
    }
    if let Ok(env) = std::env::var("PERTISK_TPM_EK_CAS") {
        let p = PathBuf::from(env.trim());
        if p.is_dir() {
            return Some(p);
        }
    }
    let defaults = [
        Path::new("/system/state/secrets/tpm-ek-cas"),
        Path::new("/etc/pertisk/tpm-ek-cas"),
    ];
    defaults
        .iter()
        .find(|p| p.is_dir())
        .map(|p| (*p).to_path_buf())
}

/// Read EK cert from TPM NV (and optionally verify against manufacturer CAs).
pub fn read_ek_certificate(ca_dir: Option<&Path>) -> EkCertificate {
    match read_ek_inner(ca_dir) {
        Ok(c) => c,
        Err(Error::NoDevice) => EkCertificate::missing("no TPM device (/dev/tpmrm0 or /dev/tpm0)"),
        Err(e) => EkCertificate::missing(format!("EK certificate read failed: {e}")),
    }
}

fn read_ek_inner(ca_dir: Option<&Path>) -> Result<EkCertificate> {
    let mut dev = Device::open_default()?;
    let (nv_index, raw) = read_ek_from_nv(&mut dev)?;
    let der = normalize_to_der(&raw)?;
    let (subject, issuer) = parse_subject_issuer(&der)?;
    let fingerprint_sha256 = hex_sha256(&der);

    let cas = resolve_ca_dir(ca_dir);
    let (chain_status, chain_message) = match &cas {
        None => (
            EkChainStatus::Unverified,
            "no manufacturer CA dir (set PERTISK_TPM_EK_CAS)".into(),
        ),
        Some(dir) => match verify_ek_chain(&der, dir) {
            Ok(ca_subject) => (EkChainStatus::Ok, format!("chained to {ca_subject}")),
            Err(e) => (EkChainStatus::Failed(e.clone()), e),
        },
    };

    Ok(EkCertificate {
        available: true,
        message: format!(
            "EK certificate from NV 0x{nv_index:08x} ({} bytes, chain={})",
            der.len(),
            chain_status.as_str()
        ),
        nv_index,
        der,
        subject,
        issuer,
        fingerprint_sha256,
        chain_status,
        chain_message,
    })
}

fn read_ek_from_nv(dev: &mut Device) -> Result<(u32, Vec<u8>)> {
    let mut last_err = Error::Parse("no EK certificate NV index found".into());
    for &idx in EK_CERT_NV_INDEXES {
        match commands::nv_read_all(dev, idx) {
            Ok(data) if !data.is_empty() => return Ok((idx, data)),
            Ok(_) => {
                last_err = Error::Parse(format!("NV 0x{idx:08x} empty"));
            }
            Err(e) => {
                last_err = e;
            }
        }
    }
    Err(last_err)
}

/// Accept DER or PEM-wrapped certificate bytes from NV.
pub fn normalize_to_der(raw: &[u8]) -> Result<Vec<u8>> {
    let trimmed = trim_nulls(raw);
    if trimmed.is_empty() {
        return Err(Error::Parse("empty EK certificate".into()));
    }
    if trimmed.windows(10).any(|w| w == b"-----BEGIN") {
        return pem_to_der(trimmed);
    }
    if trimmed[0] == 0x30 {
        return Ok(trimmed.to_vec());
    }
    Err(Error::Parse(format!(
        "EK NV data is not DER/PEM (first byte 0x{:02x}, {} bytes)",
        trimmed[0],
        trimmed.len()
    )))
}

fn trim_nulls(raw: &[u8]) -> &[u8] {
    let end = raw
        .iter()
        .rposition(|&b| b != 0)
        .map(|i| i + 1)
        .unwrap_or(0);
    &raw[..end]
}

fn pem_to_der(pem: &[u8]) -> Result<Vec<u8>> {
    let certs = split_certs(pem)?;
    certs
        .into_iter()
        .next()
        .ok_or_else(|| Error::Parse("EK PEM missing body".into()))
}

fn parse_subject_issuer(der: &[u8]) -> Result<(String, String)> {
    let (_, cert) =
        X509Certificate::from_der(der).map_err(|e| Error::Parse(format!("EK X.509 parse: {e}")))?;
    Ok((cert.subject().to_string(), cert.issuer().to_string()))
}

fn hex_sha256(data: &[u8]) -> String {
    let dig = Sha256::digest(data);
    dig.iter().map(|b| format!("{b:02x}")).collect()
}

struct OwnedCa {
    der: Vec<u8>,
    subject: String,
}

/// Verify leaf DER against PEM/DER certificates in `ca_dir` (`*.pem` / `*.crt` / `*.der`).
///
/// Supports leaf→CA and leaf→intermediate→root when intermediates are also present.
pub fn verify_ek_chain(leaf_der: &[u8], ca_dir: &Path) -> std::result::Result<String, String> {
    let anchors = load_ca_certs(ca_dir).map_err(|e| e.to_string())?;
    if anchors.is_empty() {
        return Err(format!("no PEM certificates in {}", ca_dir.display()));
    }
    let (_, leaf) = X509Certificate::from_der(leaf_der).map_err(|e| format!("leaf parse: {e}"))?;

    for ca in &anchors {
        let (_, ca_cert) =
            X509Certificate::from_der(&ca.der).map_err(|e| format!("CA parse: {e}"))?;
        if leaf.subject() == ca_cert.subject() && leaf_der == ca.der.as_slice() {
            return Ok(ca.subject.clone());
        }
        if *leaf.issuer() == *ca_cert.subject()
            && leaf.verify_signature(Some(ca_cert.public_key())).is_ok()
        {
            return Ok(ca.subject.clone());
        }
    }

    for mid in &anchors {
        let (_, mid_cert) =
            X509Certificate::from_der(&mid.der).map_err(|e| format!("CA parse: {e}"))?;
        if *leaf.issuer() != *mid_cert.subject() {
            continue;
        }
        if leaf.verify_signature(Some(mid_cert.public_key())).is_err() {
            continue;
        }
        for root in &anchors {
            let (_, root_cert) =
                X509Certificate::from_der(&root.der).map_err(|e| format!("CA parse: {e}"))?;
            if *mid_cert.issuer() != *root_cert.subject() {
                continue;
            }
            if mid_cert
                .verify_signature(Some(root_cert.public_key()))
                .is_ok()
            {
                return Ok(root.subject.clone());
            }
        }
        // Intermediate listed as trust material is enough for lab endorsement.
        return Ok(mid.subject.clone());
    }

    Err(format!(
        "EK issuer [{}] not chained to any of {} CA(s) in {}",
        leaf.issuer(),
        anchors.len(),
        ca_dir.display()
    ))
}

fn load_ca_certs(dir: &Path) -> Result<Vec<OwnedCa>> {
    let mut out = Vec::new();
    let rd = fs::read_dir(dir).map_err(Error::Io)?;
    for ent in rd.flatten() {
        let path = ent.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(ext.as_str(), "pem" | "crt" | "cer" | "der") {
            continue;
        }
        let raw = fs::read(&path).map_err(Error::Io)?;
        for der in split_certs(&raw)? {
            let (_, cert) = X509Certificate::from_der(&der)
                .map_err(|e| Error::Parse(format!("{}: {e}", path.display())))?;
            out.push(OwnedCa {
                subject: cert.subject().to_string(),
                der,
            });
        }
    }
    Ok(out)
}

fn split_certs(raw: &[u8]) -> Result<Vec<Vec<u8>>> {
    if raw.windows(10).any(|w| w == b"-----BEGIN") {
        let text =
            std::str::from_utf8(raw).map_err(|e| Error::Parse(format!("CA PEM utf8: {e}")))?;
        let mut out = Vec::new();
        let mut b64 = String::new();
        let mut in_body = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("-----BEGIN") {
                in_body = true;
                b64.clear();
                continue;
            }
            if line.starts_with("-----END") {
                if !b64.is_empty() {
                    out.push(b64_decode(&b64)?);
                }
                in_body = false;
                continue;
            }
            if in_body {
                b64.push_str(line);
            }
        }
        if out.is_empty() {
            return Err(Error::Parse("CA PEM contained no certificates".into()));
        }
        return Ok(out);
    }
    if raw.first() == Some(&0x30) {
        return Ok(vec![raw.to_vec()]);
    }
    Err(Error::Parse("CA file is not PEM or DER".into()))
}

fn b64_decode(s: &str) -> Result<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !bytes.len().is_multiple_of(4) {
        return Err(Error::Parse("base64 length".into()));
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let (a, b, c, d) = (chunk[0], chunk[1], chunk[2], chunk[3]);
        let pad = usize::from(c == b'=') + usize::from(d == b'=');
        let av = val(a).ok_or_else(|| Error::Parse("base64".into()))?;
        let bv = val(b).ok_or_else(|| Error::Parse("base64".into()))?;
        let cv = if c == b'=' {
            0
        } else {
            val(c).ok_or_else(|| Error::Parse("base64".into()))?
        };
        let dv = if d == b'=' {
            0
        } else {
            val(d).ok_or_else(|| Error::Parse("base64".into()))?
        };
        let n =
            (u32::from(av) << 18) | (u32::from(bv) << 12) | (u32::from(cv) << 6) | u32::from(dv);
        out.push(((n >> 16) & 0xff) as u8);
        if pad < 2 {
            out.push(((n >> 8) & 0xff) as u8);
        }
        if pad < 1 {
            out.push((n & 0xff) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertificateParams, KeyPair};

    fn self_signed_der() -> (Vec<u8>, String) {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(vec!["tpm-ek-test.example".into()]).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let cert = params.self_signed(&key).unwrap();
        let der = cert.der().to_vec();
        let subject = {
            let (_, c) = X509Certificate::from_der(&der).unwrap();
            c.subject().to_string()
        };
        (der, subject)
    }

    fn der_to_pem(der: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::from("-----BEGIN CERTIFICATE-----\n");
        for chunk in der.chunks(3) {
            let mut n = u32::from(chunk[0]) << 16;
            if chunk.len() > 1 {
                n |= u32::from(chunk[1]) << 8;
            }
            if chunk.len() > 2 {
                n |= u32::from(chunk[2]);
            }
            s.push(T[((n >> 18) & 63) as usize] as char);
            s.push(T[((n >> 12) & 63) as usize] as char);
            s.push(if chunk.len() > 1 {
                T[((n >> 6) & 63) as usize] as char
            } else {
                '='
            });
            s.push(if chunk.len() > 2 {
                T[(n & 63) as usize] as char
            } else {
                '='
            });
        }
        s.push_str("\n-----END CERTIFICATE-----\n");
        s
    }

    #[test]
    fn verify_self_signed_as_trust_anchor() {
        let (der, subject) = self_signed_der();
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("ek-ca.pem"), der_to_pem(&der)).unwrap();
        let got = verify_ek_chain(&der, dir.path()).unwrap();
        assert_eq!(got, subject);
    }

    #[test]
    fn normalize_pem_roundtrip() {
        let (der, _) = self_signed_der();
        let pem = der_to_pem(&der);
        let got = normalize_to_der(pem.as_bytes()).unwrap();
        assert_eq!(got, der);
    }

    #[test]
    fn missing_ca_dir_errors() {
        let (der, _) = self_signed_der();
        let dir = tempfile::tempdir().unwrap();
        let err = verify_ek_chain(&der, dir.path()).unwrap_err();
        assert!(err.contains("no PEM"));
    }
}
