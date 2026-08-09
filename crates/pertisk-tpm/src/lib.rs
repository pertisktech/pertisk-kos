//! Minimal pure-Rust TPM 2.0 Quote client for Pertisk lab (no libtss2).
//!
//! Speaks the TPM wire protocol over `/dev/tpmrm0` (fallback `/dev/tpm0`),
//! creates an ephemeral ECC NIST-P256 restricted signing AK, and issues
//! `TPM2_Quote` for PCRs 0–7 and 11.

mod commands;
mod device;
mod error;
mod quote;
mod verify;
mod wire;

pub use error::{Error, Result};
pub use quote::{produce_quote, QuoteBundle, QUOTE_PCR_INDICES};
pub use quote::pcr_digests_from_hex;
pub use verify::{verify_quote, PcrDigest};
