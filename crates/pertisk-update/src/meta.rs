//! Boot slot metadata persisted on STATE (stands in for META partition in M5).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const META_FILENAME: &str = "boot-meta.json";
pub const MAX_BOOT_ATTEMPTS: u32 = 3;

#[derive(Debug, Error)]
pub enum MetaError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BootSlot {
    A,
    B,
}

impl BootSlot {
    pub fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }
}

impl std::fmt::Display for BootSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Persistent boot/update metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BootMeta {
    pub active: BootSlot,
    /// Slot to boot on next reboot (may equal active).
    pub next: BootSlot,
    /// Previous known-good slot for rollback.
    pub previous_good: BootSlot,
    pub boot_attempts: u32,
    pub boot_ok: bool,
    #[serde(default)]
    pub pending_version: Option<String>,
    #[serde(default)]
    pub active_version: Option<String>,
}

impl Default for BootMeta {
    fn default() -> Self {
        Self {
            active: BootSlot::A,
            next: BootSlot::A,
            previous_good: BootSlot::A,
            boot_attempts: 0,
            boot_ok: true,
            pending_version: None,
            active_version: Some(env!("CARGO_PKG_VERSION").into()),
        }
    }
}

impl BootMeta {
    pub fn path(state_root: &Path) -> PathBuf {
        state_root.join(META_FILENAME)
    }

    pub fn load(state_root: &Path) -> Result<Self, MetaError> {
        let path = Self::path(state_root);
        if !path.exists() {
            let meta = Self::default();
            meta.save(state_root)?;
            return Ok(meta);
        }
        let raw = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn save(&self, state_root: &Path) -> Result<(), MetaError> {
        fs::create_dir_all(state_root)?;
        let path = Self::path(state_root);
        let raw = serde_json::to_string_pretty(self)?;
        fs::write(path, raw)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn roundtrip_meta() {
        let dir = tempdir().unwrap();
        let mut meta = BootMeta::default();
        meta.next = BootSlot::B;
        meta.pending_version = Some("0.2.0".into());
        meta.save(dir.path()).unwrap();
        let loaded = BootMeta::load(dir.path()).unwrap();
        assert_eq!(loaded.next, BootSlot::B);
        assert_eq!(loaded.pending_version.as_deref(), Some("0.2.0"));
    }
}
