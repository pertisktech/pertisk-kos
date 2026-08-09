//! Linux TPM character-device transport.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

const CANDIDATES: &[&str] = &["/dev/tpmrm0", "/dev/tpm0"];

pub struct Device {
    file: std::fs::File,
    path: PathBuf,
}

impl Device {
    /// Open the first available TPM device.
    pub fn open_default() -> Result<Self> {
        for p in CANDIDATES {
            if Path::new(p).exists() {
                return Self::open(Path::new(p));
            }
        }
        Err(Error::NoDevice)
    }

    pub fn open(path: &Path) -> Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Send a command and read the full TPM response.
    pub fn transact(&mut self, cmd: &[u8]) -> Result<Vec<u8>> {
        self.file.write_all(cmd)?;
        let mut hdr = [0u8; 10];
        self.file.read_exact(&mut hdr)?;
        let size = u32::from_be_bytes([hdr[2], hdr[3], hdr[4], hdr[5]]) as usize;
        if size < 10 {
            return Err(Error::Parse(format!("invalid TPM response size {size}")));
        }
        let mut resp = vec![0u8; size];
        resp[..10].copy_from_slice(&hdr);
        if size > 10 {
            self.file.read_exact(&mut resp[10..])?;
        }
        Ok(resp)
    }
}
