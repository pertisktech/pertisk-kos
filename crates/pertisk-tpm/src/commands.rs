//! TPM2 command builders / response parsers for Quote.

use crate::device::Device;
use crate::error::{Error, Result};
use crate::wire::{
    marshal_ecc_ak_public, marshal_ecc_storage_public, marshal_pcr_selection,
    marshal_sensitive_create_empty, parse_response, response_params, Reader, Writer,
    AK_PERSISTENT_HANDLE, TPM_ALG_ECDSA, TPM_ALG_NULL, TPM_CC_CREATE, TPM_CC_CREATE_PRIMARY,
    TPM_CC_EVICT_CONTROL, TPM_CC_FLUSH_CONTEXT, TPM_CC_LOAD, TPM_CC_NV_READ, TPM_CC_NV_READ_PUBLIC,
    TPM_CC_QUOTE, TPM_CC_READ_PUBLIC, TPM_RH_OWNER, TPM_ST_NO_SESSIONS, TPM_ST_SESSIONS,
};

pub struct LoadedKey {
    pub handle: u32,
    /// TPMT_PUBLIC bytes (without TPM2B size prefix).
    pub public: Vec<u8>,
}

fn cmd_create_primary() -> Vec<u8> {
    let mut w = Writer::new();
    w.u16(TPM_ST_SESSIONS);
    w.u32(0);
    w.u32(TPM_CC_CREATE_PRIMARY);
    w.u32(TPM_RH_OWNER);

    let mut auth = Writer::new();
    auth.pw_empty_auth();
    let auth_bytes = auth.into_vec();
    w.u32(auth_bytes.len() as u32);
    w.bytes(&auth_bytes);

    w.bytes(&marshal_sensitive_create_empty());
    w.bytes(&marshal_ecc_storage_public());
    w.tpm2b(&[]); // outsideInfo
    w.u32(0); // creationPCR count

    w.finish_header()
}

fn parse_create_primary(resp: &[u8]) -> Result<(u32, Vec<u8>)> {
    let (tag, _, mut r) = parse_response(resp)?;
    let handle = r.u32()?;
    let mut p = response_params(&mut r, tag)?;
    let public = tpm2b_owned(&mut p)?;
    Ok((handle, public))
}

fn cmd_create(parent: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u16(TPM_ST_SESSIONS);
    w.u32(0);
    w.u32(TPM_CC_CREATE);
    w.u32(parent);

    let mut auth = Writer::new();
    auth.pw_empty_auth();
    let auth_bytes = auth.into_vec();
    w.u32(auth_bytes.len() as u32);
    w.bytes(&auth_bytes);

    w.bytes(&marshal_sensitive_create_empty());
    w.bytes(&marshal_ecc_ak_public());
    w.tpm2b(&[]); // outsideInfo
    w.u32(0); // creationPCR

    w.finish_header()
}

fn parse_create(resp: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    let (tag, _, mut r) = parse_response(resp)?;
    let mut p = response_params(&mut r, tag)?;
    let private = tpm2b_owned(&mut p)?;
    let public = tpm2b_owned(&mut p)?;
    Ok((private, public))
}

fn cmd_load(parent: u32, private: &[u8], public: &[u8]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u16(TPM_ST_SESSIONS);
    w.u32(0);
    w.u32(TPM_CC_LOAD);
    w.u32(parent);

    let mut auth = Writer::new();
    auth.pw_empty_auth();
    let auth_bytes = auth.into_vec();
    w.u32(auth_bytes.len() as u32);
    w.bytes(&auth_bytes);

    w.tpm2b(private);
    w.tpm2b(public);
    w.finish_header()
}

fn parse_load(resp: &[u8]) -> Result<u32> {
    let (tag, _, mut r) = parse_response(resp)?;
    let handle = r.u32()?;
    let _ = response_params(&mut r, tag)?;
    Ok(handle)
}

fn cmd_read_public(handle: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u16(TPM_ST_NO_SESSIONS);
    w.u32(0);
    w.u32(TPM_CC_READ_PUBLIC);
    w.u32(handle);
    w.finish_header()
}

fn parse_read_public(resp: &[u8]) -> Result<Vec<u8>> {
    let (tag, _, mut r) = parse_response(resp)?;
    let mut p = response_params(&mut r, tag)?;
    tpm2b_owned(&mut p)
}

fn cmd_quote(ak: u32, nonce: &[u8], pcr_indices: &[u32]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u16(TPM_ST_SESSIONS);
    w.u32(0);
    w.u32(TPM_CC_QUOTE);
    w.u32(ak);

    let mut auth = Writer::new();
    auth.pw_empty_auth();
    let auth_bytes = auth.into_vec();
    w.u32(auth_bytes.len() as u32);
    w.bytes(&auth_bytes);

    w.tpm2b(nonce);
    w.u16(TPM_ALG_NULL);
    w.bytes(&marshal_pcr_selection(pcr_indices));
    w.finish_header()
}

fn parse_quote(resp: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    let (tag, _, mut r) = parse_response(resp)?;
    // Parameters live in a sub-slice; capture signature from that sub-slice.
    let param_size = if tag == TPM_ST_SESSIONS {
        r.u32()? as usize
    } else {
        r.remaining()
    };
    let params = r.take(param_size)?;
    let mut p = Reader::new(params);
    let quoted = tpm2b_owned(&mut p)?;
    let sig_off = p.absolute_pos();
    let alg = p.u16()?;
    if alg != TPM_ALG_ECDSA {
        return Err(Error::Parse(format!("unsupported sig alg 0x{alg:04x}")));
    }
    let _hash = p.u16()?;
    p.skip_tpm2b()?;
    p.skip_tpm2b()?;
    let signature = params[sig_off..p.absolute_pos()].to_vec();
    Ok((quoted, signature))
}

fn cmd_flush(handle: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u16(TPM_ST_NO_SESSIONS);
    w.u32(0);
    w.u32(TPM_CC_FLUSH_CONTEXT);
    w.u32(handle);
    w.finish_header()
}

fn tpm2b_owned(r: &mut Reader<'_>) -> Result<Vec<u8>> {
    Ok(r.tpm2b()?.to_vec())
}

pub fn flush(dev: &mut Device, handle: u32) {
    let _ = dev.transact(&cmd_flush(handle));
}

pub fn create_primary(dev: &mut Device) -> Result<LoadedKey> {
    let resp = dev.transact(&cmd_create_primary())?;
    let (handle, public) = parse_create_primary(&resp)?;
    Ok(LoadedKey { handle, public })
}

pub fn create_and_load_ak(dev: &mut Device, parent: u32) -> Result<LoadedKey> {
    let resp = dev.transact(&cmd_create(parent))?;
    let (private, public) = parse_create(&resp)?;
    let resp = dev.transact(&cmd_load(parent, &private, &public))?;
    let handle = parse_load(&resp)?;
    let pub_resp = dev.transact(&cmd_read_public(handle))?;
    let public = parse_read_public(&pub_resp).unwrap_or(public);
    Ok(LoadedKey { handle, public })
}

pub fn quote(
    dev: &mut Device,
    ak: u32,
    nonce: &[u8],
    pcr_indices: &[u32],
) -> Result<(Vec<u8>, Vec<u8>)> {
    let resp = dev.transact(&cmd_quote(ak, nonce, pcr_indices))?;
    parse_quote(&resp)
}

fn cmd_evict(object: u32, persistent: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u16(TPM_ST_SESSIONS);
    w.u32(0);
    w.u32(TPM_CC_EVICT_CONTROL);
    w.u32(TPM_RH_OWNER);
    w.u32(object);

    let mut auth = Writer::new();
    auth.pw_empty_auth();
    let auth_bytes = auth.into_vec();
    w.u32(auth_bytes.len() as u32);
    w.bytes(&auth_bytes);

    w.u32(persistent);
    w.finish_header()
}

/// ReadPublic for an existing handle (transient or persistent).
pub fn read_public(dev: &mut Device, handle: u32) -> Result<Vec<u8>> {
    let resp = dev.transact(&cmd_read_public(handle))?;
    parse_read_public(&resp)
}

/// Persist a loaded object at `persistent` (EvictControl).
pub fn evict_control(dev: &mut Device, object: u32, persistent: u32) -> Result<()> {
    let resp = dev.transact(&cmd_evict(object, persistent))?;
    let _ = parse_response(&resp)?;
    Ok(())
}

/// Load or create the lab persistent AK at [`AK_PERSISTENT_HANDLE`].
///
/// Returns the persistent handle + TPMT_PUBLIC. Primary is flushed after enroll.
pub fn ensure_persistent_ak(dev: &mut Device) -> Result<LoadedKey> {
    if let Ok(public) = read_public(dev, AK_PERSISTENT_HANDLE) {
        return Ok(LoadedKey {
            handle: AK_PERSISTENT_HANDLE,
            public,
        });
    }

    let primary = create_primary(dev)?;
    let ak = match create_and_load_ak(dev, primary.handle) {
        Ok(k) => k,
        Err(e) => {
            flush(dev, primary.handle);
            return Err(e);
        }
    };
    // If a stale persistent object occupies the slot, evict it first.
    let _ = evict_control(dev, AK_PERSISTENT_HANDLE, AK_PERSISTENT_HANDLE);
    if let Err(e) = evict_control(dev, ak.handle, AK_PERSISTENT_HANDLE) {
        flush(dev, ak.handle);
        flush(dev, primary.handle);
        return Err(e);
    }
    // Transient AK handle is invalidated by EvictControl into persistent.
    flush(dev, primary.handle);
    let public = read_public(dev, AK_PERSISTENT_HANDLE).unwrap_or(ak.public);
    Ok(LoadedKey {
        handle: AK_PERSISTENT_HANDLE,
        public,
    })
}

fn cmd_nv_read_public(nv_index: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u16(TPM_ST_NO_SESSIONS);
    w.u32(0);
    w.u32(TPM_CC_NV_READ_PUBLIC);
    w.u32(nv_index);
    w.finish_header()
}

/// Returns NV data size from TPMS_NV_PUBLIC (0 if index missing / unreadable).
pub fn nv_data_size(dev: &mut Device, nv_index: u32) -> Result<u16> {
    let resp = dev.transact(&cmd_nv_read_public(nv_index))?;
    let (tag, _, mut r) = parse_response(&resp)?;
    let mut p = response_params(&mut r, tag)?;
    let nv_public = p.tpm2b()?;
    // TPMS_NV_PUBLIC: nvIndex(u32) + nameAlg(u16) + attributes(u32) + authPolicy(TPM2B) + dataSize(u16)
    let mut n = Reader::new(nv_public);
    let _ = n.u32()?;
    let _ = n.u16()?;
    let _ = n.u32()?;
    n.skip_tpm2b()?;
    Ok(n.u16()?)
}

fn cmd_nv_read(nv_index: u32, size: u16, offset: u16) -> Vec<u8> {
    let mut w = Writer::new();
    w.u16(TPM_ST_SESSIONS);
    w.u32(0);
    w.u32(TPM_CC_NV_READ);
    w.u32(nv_index); // authHandle
    w.u32(nv_index); // nvIndex

    let mut auth = Writer::new();
    auth.pw_empty_auth();
    let auth_bytes = auth.into_vec();
    w.u32(auth_bytes.len() as u32);
    w.bytes(&auth_bytes);

    w.u16(size);
    w.u16(offset);
    w.finish_header()
}

fn parse_nv_read(resp: &[u8]) -> Result<Vec<u8>> {
    let (tag, _, mut r) = parse_response(resp)?;
    let mut p = response_params(&mut r, tag)?;
    tpm2b_owned(&mut p)
}

/// Read the full NV index contents (chunked). Soft-fails via `Result` if empty/missing.
pub fn nv_read_all(dev: &mut Device, nv_index: u32) -> Result<Vec<u8>> {
    let total = nv_data_size(dev, nv_index)? as usize;
    if total == 0 {
        return Err(Error::Parse(format!(
            "NV 0x{nv_index:08x} exists but dataSize=0"
        )));
    }
    let mut out = Vec::with_capacity(total);
    let chunk: u16 = 512;
    while out.len() < total {
        let remaining = total - out.len();
        let want = (remaining as u16).min(chunk);
        let offset = out.len() as u16;
        let resp = dev.transact(&cmd_nv_read(nv_index, want, offset))?;
        let piece = parse_nv_read(&resp)?;
        if piece.is_empty() {
            return Err(Error::Parse(format!(
                "NV 0x{nv_index:08x} read returned empty at offset {offset}"
            )));
        }
        out.extend_from_slice(&piece);
    }
    Ok(out)
}
