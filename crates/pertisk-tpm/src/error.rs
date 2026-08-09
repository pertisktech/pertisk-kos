//! Errors for the lab TPM Quote client.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("no TPM device (/dev/tpmrm0 or /dev/tpm0)")]
    NoDevice,
    #[error("TPM I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("TPM response code 0x{0:08x}")]
    TpmRc(u32),
    #[error("parse: {0}")]
    Parse(String),
    #[error("verify: {0}")]
    Verify(String),
}
