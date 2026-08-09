//! Minimal pure-Rust TPM 2.0 Quote client for Pertisk lab (no libtss2).
//!
//! Speaks the TPM wire protocol over `/dev/tpmrm0` (fallback `/dev/tpm0`),
//! creates a persistent ECC NIST-P256 restricted signing AK, issues
//! `TPM2_Quote` for PCRs 0–7 and 11, and attaches the manufacturer EK
//! certificate from TCG NV indexes when present.

mod commands;
mod device;
mod ek;
mod error;
mod quote;
mod verify;
mod wire;

pub use ek::{
    read_ek_certificate, resolve_ca_dir, verify_ek_chain, EkCertificate, EkChainStatus,
    EK_CERT_NV_INDEXES,
};
pub use error::{Error, Result};
pub use quote::pcr_digests_from_hex;
pub use quote::{produce_quote, QuoteBundle, QUOTE_PCR_INDICES};
pub use verify::{verify_quote, PcrDigest};
pub use wire::AK_PERSISTENT_HANDLE;
