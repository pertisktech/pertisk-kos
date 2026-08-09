//! Local Quote verification (ECDSA-P256 + PCR digest + nonce).

use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use p256::{EncodedPoint, FieldBytes};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::wire::{
    Reader, TPM_ALG_ECC, TPM_ALG_ECDSA, TPM_ALG_SHA256, TPM_GENERATED_VALUE, TPM_ST_ATTEST_QUOTE,
};

/// PCR digest from Attest/sysfs used for cross-check.
#[derive(Debug, Clone)]
pub struct PcrDigest {
    pub index: u32,
    pub digest_hex: String,
}

/// Verify a Quote produced by this crate.
///
/// - `quoted`: TPMS_ATTEST bytes (contents of TPM2B_ATTEST)
/// - `signature`: TPMT_SIGNATURE marshaled (ECDSA)
/// - `ak_public`: TPMT_PUBLIC bytes
/// - `nonce`: expected qualifyingData / extraData
/// - `pcrs`: optional sysfs digests to recompute pcrDigest
pub fn verify_quote(
    quoted: &[u8],
    signature: &[u8],
    ak_public: &[u8],
    nonce: &[u8],
    pcrs: &[PcrDigest],
) -> Result<()> {
    let attest = parse_tpms_attest(quoted)?;
    if attest.magic != TPM_GENERATED_VALUE {
        return Err(Error::Verify(format!(
            "bad attest magic 0x{:08x}",
            attest.magic
        )));
    }
    if attest.type_ != TPM_ST_ATTEST_QUOTE {
        return Err(Error::Verify(format!(
            "attest type 0x{:04x} is not Quote",
            attest.type_
        )));
    }
    if attest.extra_data != nonce {
        return Err(Error::Verify("nonce / extraData mismatch".into()));
    }

    if !pcrs.is_empty() {
        let expected = compute_pcr_digest(&attest.pcr_select, pcrs)?;
        if expected != attest.pcr_digest {
            return Err(Error::Verify(format!(
                "pcrDigest mismatch: quote={} recomputed={}",
                hex::encode(&attest.pcr_digest),
                hex::encode(&expected)
            )));
        }
    }

    let (x, y) = extract_ecc_xy(ak_public)?;
    let point = EncodedPoint::from_affine_coordinates(
        field_bytes(&x)?,
        field_bytes(&y)?,
        false,
    );
    let vk = VerifyingKey::from_encoded_point(&point)
        .map_err(|e| Error::Verify(format!("AK public key: {e}")))?;

    let (r, s) = parse_ecdsa_signature(signature)?;
    let sig = Signature::from_scalars(field_bytes(&r)?.clone(), field_bytes(&s)?.clone())
        .map_err(|e| Error::Verify(format!("signature scalars: {e}")))?;

    // p256 Verifier hashes the message with SHA-256 (matches ECDSA-SHA256 scheme).
    vk.verify(quoted, &sig)
        .map_err(|e| Error::Verify(format!("ECDSA verify failed: {e}")))?;
    Ok(())
}

struct AttestQuote {
    magic: u32,
    type_: u16,
    extra_data: Vec<u8>,
    pcr_select: Vec<u32>,
    pcr_digest: Vec<u8>,
}

fn parse_tpms_attest(data: &[u8]) -> Result<AttestQuote> {
    let mut r = Reader::new(data);
    let magic = r.u32()?;
    let type_ = r.u16()?;
    r.skip_tpm2b()?; // qualifiedSigner
    let extra_data = r.tpm2b()?.to_vec();
    // clockInfo: clock u64, resetCount u32, restartCount u32, safe u8
    let _ = r.u64()?;
    let _ = r.u32()?;
    let _ = r.u32()?;
    let _ = r.u8()?;
    let _ = r.u64()?; // firmwareVersion
    // TPMS_QUOTE_INFO
    let pcr_select = parse_pcr_selection_indices(&mut r)?;
    let pcr_digest = r.tpm2b()?.to_vec();
    Ok(AttestQuote {
        magic,
        type_,
        extra_data,
        pcr_select,
        pcr_digest,
    })
}

fn parse_pcr_selection_indices(r: &mut Reader<'_>) -> Result<Vec<u32>> {
    let count = r.u32()?;
    let mut indices = Vec::new();
    for _ in 0..count {
        let hash = r.u16()?;
        let size = r.u8()? as usize;
        let bitmap = r.take(size)?;
        if hash != TPM_ALG_SHA256 {
            continue;
        }
        for (byte_i, byte) in bitmap.iter().enumerate() {
            for bit in 0..8u32 {
                if byte & (1 << bit) != 0 {
                    indices.push(byte_i as u32 * 8 + bit);
                }
            }
        }
    }
    indices.sort_unstable();
    Ok(indices)
}

fn compute_pcr_digest(indices: &[u32], pcrs: &[PcrDigest]) -> Result<Vec<u8>> {
    let mut concat = Vec::new();
    for &idx in indices {
        let Some(p) = pcrs.iter().find(|p| p.index == idx) else {
            return Err(Error::Verify(format!("missing PCR {idx} for digest check")));
        };
        let bytes = hex_decode(&p.digest_hex)?;
        if bytes.len() != 32 {
            return Err(Error::Verify(format!(
                "PCR {idx} digest length {} (want 32)",
                bytes.len()
            )));
        }
        concat.extend_from_slice(&bytes);
    }
    Ok(Sha256::digest(&concat).to_vec())
}

fn extract_ecc_xy(tpmt_public: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut r = Reader::new(tpmt_public);
    let type_ = r.u16()?;
    if type_ != TPM_ALG_ECC {
        return Err(Error::Verify(format!("AK type 0x{type_:04x} not ECC")));
    }
    let _name_alg = r.u16()?;
    let _attrs = r.u32()?;
    r.skip_tpm2b()?; // authPolicy
    // TPMS_ECC_PARMS
    let sym = r.u16()?;
    if sym == crate::wire::TPM_ALG_AES {
        let _ = r.u16()?; // keyBits
        let _ = r.u16()?; // mode
    } else if sym != crate::wire::TPM_ALG_NULL {
        return Err(Error::Verify(format!("unexpected symmetric 0x{sym:04x}")));
    }
    let scheme = r.u16()?;
    if scheme == TPM_ALG_ECDSA {
        let _ = r.u16()?; // hash
    } else if scheme != crate::wire::TPM_ALG_NULL {
        let _ = r.u16()?; // try skip one
    }
    let _curve = r.u16()?;
    let kdf = r.u16()?;
    if kdf != crate::wire::TPM_ALG_NULL {
        let _ = r.u16()?;
    }
    // unique TPM2B_ECC_POINT — but unique is TPMS_ECC_POINT with two TPM2B
    let x = r.tpm2b()?.to_vec();
    let y = r.tpm2b()?.to_vec();
    if x.is_empty() || y.is_empty() {
        return Err(Error::Verify("AK unique point empty".into()));
    }
    Ok((pad32(x), pad32(y)))
}

fn parse_ecdsa_signature(sig: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut r = Reader::new(sig);
    let alg = r.u16()?;
    if alg != TPM_ALG_ECDSA {
        return Err(Error::Verify(format!("sig alg 0x{alg:04x}")));
    }
    let _hash = r.u16()?;
    let rr = pad32(r.tpm2b()?.to_vec());
    let ss = pad32(r.tpm2b()?.to_vec());
    Ok((rr, ss))
}

fn pad32(mut v: Vec<u8>) -> Vec<u8> {
    if v.len() > 32 {
        // trim leading zeros
        while v.len() > 32 && v[0] == 0 {
            v.remove(0);
        }
    }
    if v.len() < 32 {
        let mut out = vec![0u8; 32 - v.len()];
        out.extend(v);
        out
    } else {
        v
    }
}

fn field_bytes(b: &[u8]) -> Result<&FieldBytes> {
    if b.len() != 32 {
        return Err(Error::Verify(format!("expected 32 bytes, got {}", b.len())));
    }
    Ok(FieldBytes::from_slice(b))
}

fn hex_decode(s: &str) -> Result<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return Err(Error::Verify("odd hex length".into()));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|e| Error::Verify(format!("hex: {e}")))
        })
        .collect()
}

// Minimal hex encode without extra dep
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn rejects_bad_magic() {
        let mut w = crate::wire::Writer::new();
        w.u32(0xdead_beef);
        w.u16(TPM_ST_ATTEST_QUOTE);
        w.tpm2b(&[]);
        w.tpm2b(b"nonce");
        w.u64(0);
        w.u32(0);
        w.u32(0);
        w.bytes(&[1]); // safe
        w.u64(0);
        w.u32(0); // empty pcr select
        w.tpm2b(&[0u8; 32]);
        let err = verify_quote(&w.into_vec(), &[], &[], b"nonce", &[]).unwrap_err();
        assert!(err.to_string().contains("magic"));
    }

    #[test]
    fn pcr_digest_order() {
        let zero = "00".repeat(32);
        let one = "11".repeat(32);
        let pcrs = vec![
            PcrDigest {
                index: 1,
                digest_hex: one.clone(),
            },
            PcrDigest {
                index: 0,
                digest_hex: zero.clone(),
            },
        ];
        let d = compute_pcr_digest(&[0, 1], &pcrs).unwrap();
        let mut concat = hex_decode(&zero).unwrap();
        concat.extend(hex_decode(&one).unwrap());
        assert_eq!(d, Sha256::digest(&concat).to_vec());
    }
}
