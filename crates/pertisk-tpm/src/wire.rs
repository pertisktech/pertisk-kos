//! TPM 2.0 wire helpers (big-endian marshal / unmarshal).

use crate::error::{Error, Result};

pub const TPM_ST_NO_SESSIONS: u16 = 0x8001;
pub const TPM_ST_SESSIONS: u16 = 0x8002;
pub const TPM_ST_ATTEST_QUOTE: u16 = 0x8018;

pub const TPM_RH_OWNER: u32 = 0x4000_0001;
pub const TPM_RS_PW: u32 = 0x4000_0009;

pub const TPM_CC_CREATE_PRIMARY: u32 = 0x0000_0131;
pub const TPM_CC_CREATE: u32 = 0x0000_0153;
pub const TPM_CC_LOAD: u32 = 0x0000_0157;
pub const TPM_CC_QUOTE: u32 = 0x0000_0158;
pub const TPM_CC_FLUSH_CONTEXT: u32 = 0x0000_0165;
pub const TPM_CC_READ_PUBLIC: u32 = 0x0000_0173;
pub const TPM_CC_EVICT_CONTROL: u32 = 0x0000_0120;
pub const TPM_CC_NV_READ: u32 = 0x0000_014E;
pub const TPM_CC_NV_READ_PUBLIC: u32 = 0x0000_0169;

/// TCG EK certificate NV indexes (prefer ECC, then RSA).
pub const NV_EK_CERT_ECC_P256: u32 = 0x01C0_000A;
pub const NV_EK_CERT_RSA: u32 = 0x01C0_0002;
pub const NV_EK_CERT_ECC_P384: u32 = 0x01C0_000C;

/// Persistent handle for the lab attestation signing key.
pub const AK_PERSISTENT_HANDLE: u32 = 0x8100_000A;

pub const TPM_ALG_SHA256: u16 = 0x000B;
pub const TPM_ALG_NULL: u16 = 0x0010;
pub const TPM_ALG_ECDSA: u16 = 0x0018;
pub const TPM_ALG_ECC: u16 = 0x0023;
pub const TPM_ALG_AES: u16 = 0x0006;
pub const TPM_ALG_CFB: u16 = 0x0043;
pub const TPM_ECC_NIST_P256: u16 = 0x0003;

pub const TPM_GENERATED_VALUE: u32 = 0xff54_4347;

/// fixedTPM|fixedParent|sensitiveDataOrigin|userWithAuth|noDA|decrypt|restricted
pub const ATTR_STORAGE: u32 = 0x0003_0472;
/// fixedTPM|fixedParent|sensitiveDataOrigin|userWithAuth|noDA|sign|restricted
pub const ATTR_AK: u32 = 0x0005_0472;

pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(512),
        }
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.buf
    }

    #[allow(dead_code)]
    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    #[allow(dead_code)]
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    pub fn tpm2b(&mut self, data: &[u8]) {
        self.u16(data.len() as u16);
        self.bytes(data);
    }

    /// Empty auth session for TPM_RS_PW (password session with empty secret).
    pub fn pw_empty_auth(&mut self) {
        self.u32(TPM_RS_PW);
        self.tpm2b(&[]); // nonce
        self.buf.push(0); // sessionAttributes
        self.tpm2b(&[]); // hmac / password
    }

    /// Patch command size at offset 2 (after tag).
    pub fn finish_header(mut self) -> Vec<u8> {
        let size = self.buf.len() as u32;
        self.buf[2..6].copy_from_slice(&size.to_be_bytes());
        self.buf
    }
}

pub struct Reader<'a> {
    data: &'a [u8],
    pub(crate) pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// Absolute offset into the original buffer.
    pub fn absolute_pos(&self) -> usize {
        self.pos
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.data.len() {
            return Err(Error::Parse("truncated TPM buffer".into()));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_be_bytes(b.try_into().unwrap()))
    }

    pub fn tpm2b(&mut self) -> Result<&'a [u8]> {
        let size = self.u16()? as usize;
        self.take(size)
    }

    pub fn skip_tpm2b(&mut self) -> Result<()> {
        let _ = self.tpm2b()?;
        Ok(())
    }
}

/// Parse a TPM response: returns (response_code, body after header+handles area start).
/// For sessioned responses, caller must skip authorizationSize + auth area.
pub fn parse_response(resp: &[u8]) -> Result<(u16, u32, Reader<'_>)> {
    if resp.len() < 10 {
        return Err(Error::Parse(format!("response too short ({})", resp.len())));
    }
    let mut r = Reader::new(resp);
    let tag = r.u16()?;
    let size = r.u32()? as usize;
    let code = r.u32()?;
    if size != resp.len() {
        return Err(Error::Parse(format!(
            "response size mismatch: header={size} actual={}",
            resp.len()
        )));
    }
    if code != 0 {
        return Err(Error::TpmRc(code));
    }
    Ok((tag, code, r))
}

/// After handles in a TPM_ST_SESSIONS response, read `parameterSize` and return
/// a reader limited to the parameter bytes (authorization follows and is ignored).
pub fn response_params<'a>(r: &mut Reader<'a>, tag: u16) -> Result<Reader<'a>> {
    if tag == TPM_ST_SESSIONS {
        let param_size = r.u32()? as usize;
        let params = r.take(param_size)?;
        Ok(Reader::new(params))
    } else {
        // No sessions: remaining bytes are parameters.
        let rest = r.take(r.remaining())?;
        Ok(Reader::new(rest))
    }
}

/// Build TPML_PCR_SELECTION for SHA-256 with the given indices (0..=23).
pub fn marshal_pcr_selection(indices: &[u32]) -> Vec<u8> {
    let mut bitmap = [0u8; 3];
    for &i in indices {
        if i < 24 {
            let byte = (i / 8) as usize;
            let bit = (i % 8) as u8;
            bitmap[byte] |= 1 << bit;
        }
    }
    let mut w = Writer::new();
    w.u32(1); // count
    w.u16(TPM_ALG_SHA256);
    w.buf.push(3); // sizeofSelect
    w.bytes(&bitmap);
    w.into_vec()
}

/// Empty TPM2B_SENSITIVE_CREATE (userAuth + data empty).
pub fn marshal_sensitive_create_empty() -> Vec<u8> {
    let mut inner = Writer::new();
    inner.tpm2b(&[]); // userAuth
    inner.tpm2b(&[]); // data
    let inner_bytes = inner.into_vec();
    let mut w = Writer::new();
    w.tpm2b(&inner_bytes);
    w.into_vec()
}

/// TPM2B_PUBLIC for ECC NIST-P256 storage primary (restricted decrypt).
pub fn marshal_ecc_storage_public() -> Vec<u8> {
    let mut pub_area = Writer::new();
    pub_area.u16(TPM_ALG_ECC);
    pub_area.u16(TPM_ALG_SHA256);
    pub_area.u32(ATTR_STORAGE);
    pub_area.tpm2b(&[]); // authPolicy
                         // TPMS_ECC_PARMS
    pub_area.u16(TPM_ALG_AES);
    pub_area.u16(128);
    pub_area.u16(TPM_ALG_CFB);
    pub_area.u16(TPM_ALG_NULL); // scheme
    pub_area.u16(TPM_ECC_NIST_P256);
    pub_area.u16(TPM_ALG_NULL); // kdf
                                // unique TPM2B_ECC_POINT empty
    pub_area.tpm2b(&[]); // x
    pub_area.tpm2b(&[]); // y

    let area = pub_area.into_vec();
    let mut w = Writer::new();
    w.tpm2b(&area);
    w.into_vec()
}

/// TPM2B_PUBLIC for ECC NIST-P256 restricted signing AK (ECDSA-SHA256).
pub fn marshal_ecc_ak_public() -> Vec<u8> {
    let mut pub_area = Writer::new();
    pub_area.u16(TPM_ALG_ECC);
    pub_area.u16(TPM_ALG_SHA256);
    pub_area.u32(ATTR_AK);
    pub_area.tpm2b(&[]); // authPolicy
                         // TPMS_ECC_PARMS
    pub_area.u16(TPM_ALG_NULL); // symmetric
    pub_area.u16(TPM_ALG_ECDSA);
    pub_area.u16(TPM_ALG_SHA256);
    pub_area.u16(TPM_ECC_NIST_P256);
    pub_area.u16(TPM_ALG_NULL); // kdf
    pub_area.tpm2b(&[]); // x
    pub_area.tpm2b(&[]); // y

    let area = pub_area.into_vec();
    let mut w = Writer::new();
    w.tpm2b(&area);
    w.into_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcr_selection_bits() {
        let sel = marshal_pcr_selection(&[0, 7, 11]);
        // count=1, hash=sha256, size=3, bits
        assert_eq!(&sel[0..4], &[0, 0, 0, 1]);
        assert_eq!(&sel[4..6], &TPM_ALG_SHA256.to_be_bytes());
        assert_eq!(sel[6], 3);
        assert_eq!(sel[7], 0b1000_0001); // PCR 0 + PCR 7
        assert_eq!(sel[8], 0b0000_1000); // PCR 11
        assert_eq!(sel[9], 0);
    }

    #[test]
    fn writer_header_size() {
        let mut w = Writer::new();
        w.u16(TPM_ST_NO_SESSIONS);
        w.u32(0); // placeholder
        w.u32(TPM_CC_FLUSH_CONTEXT);
        w.u32(0x8000_0001);
        let cmd = w.finish_header();
        assert_eq!(cmd.len(), 14);
        assert_eq!(&cmd[2..6], &14u32.to_be_bytes());
    }
}
