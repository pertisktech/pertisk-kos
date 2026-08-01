//! systemd-boot A/B slot switching for Pertisk KOS.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::{info, warn};

use crate::meta::BootSlot;

#[derive(Debug, Error)]
pub enum BootloaderError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ESP not found (looked under /boot/efi, /efi, /boot)")]
    EspNotFound,
    #[error("{0}")]
    Msg(String),
}

/// Resolved EFI System Partition mount used by systemd-boot.
#[derive(Debug, Clone)]
pub struct EspPaths {
    pub root: PathBuf,
}

impl EspPaths {
    /// Discover a mounted ESP, or use an explicit override.
    pub fn discover(explicit: Option<&Path>) -> Result<Self, BootloaderError> {
        if let Some(path) = explicit {
            return Ok(Self {
                root: path.to_path_buf(),
            });
        }
        for candidate in ["/boot/efi", "/efi", "/boot"] {
            let p = Path::new(candidate);
            if p.join("loader").is_dir()
                || p.join("EFI").is_dir()
                || p.join("EFI/BOOT").is_dir()
            {
                return Ok(Self {
                    root: p.to_path_buf(),
                });
            }
        }
        Err(BootloaderError::EspNotFound)
    }

    pub fn entry_path(&self, slot: BootSlot) -> PathBuf {
        self.root
            .join("loader/entries")
            .join(format!("pertisk-{}.conf", slot.as_str().to_lowercase()))
    }

    pub fn loader_conf(&self) -> PathBuf {
        self.root.join("loader/loader.conf")
    }

    pub fn slot_image_dir(&self, slot: BootSlot) -> PathBuf {
        self.root
            .join("pertisk")
            .join(slot.as_str().to_uppercase())
    }
}

/// Copy slot kernel/initramfs onto the ESP and point systemd-boot at `next`.
pub fn activate_slot(
    esp: &EspPaths,
    slot_dir: &Path,
    next: BootSlot,
    cmdline: &str,
) -> Result<(), BootloaderError> {
    let image_dir = esp.slot_image_dir(next);
    fs::create_dir_all(&image_dir)?;
    fs::create_dir_all(esp.root.join("loader/entries"))?;

    for name in ["kernel", "initramfs"] {
        let src = slot_dir.join(name);
        if !src.exists() {
            return Err(BootloaderError::Msg(format!(
                "missing {} in slot dir {}",
                name,
                slot_dir.display()
            )));
        }
        fs::copy(&src, image_dir.join(name))?;
    }

    write_entry(esp, next, cmdline)?;
    // Keep the other slot entry if present; only flip default.
    write_loader_default(esp, next)?;
    info!(
        slot = %next,
        esp = %esp.root.display(),
        "systemd-boot default updated"
    );
    Ok(())
}

fn write_entry(esp: &EspPaths, slot: BootSlot, cmdline: &str) -> Result<(), BootloaderError> {
    let slot_l = slot.as_str().to_lowercase();
    let slot_u = slot.as_str().to_uppercase();
    let body = format!(
        "title Pertisk KOS (slot {slot_u})\n\
         linux /pertisk/{slot_u}/kernel\n\
         initrd /pertisk/{slot_u}/initramfs\n\
         options {cmdline}\n"
    );
    let path = esp.entry_path(slot);
    let mut f = fs::File::create(path)?;
    f.write_all(body.as_bytes())?;
    let _ = slot_l;
    Ok(())
}

fn write_loader_default(esp: &EspPaths, next: BootSlot) -> Result<(), BootloaderError> {
    let conf = esp.loader_conf();
    if let Some(parent) = conf.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = format!(
        "default pertisk-{}.conf\ntimeout 3\nconsole-mode keep\n",
        next.as_str().to_lowercase()
    );
    fs::write(conf, body)?;
    Ok(())
}

/// Best-effort activate: logs and returns Ok when ESP is missing (dev/QEMU without EFI).
pub fn try_activate_slot(
    slot_dir: &Path,
    next: BootSlot,
    cmdline: &str,
    esp_override: Option<&Path>,
) -> Result<bool, BootloaderError> {
    match EspPaths::discover(esp_override) {
        Ok(esp) => {
            activate_slot(&esp, slot_dir, next, cmdline)?;
            Ok(true)
        }
        Err(BootloaderError::EspNotFound) => {
            warn!("ESP not mounted; skipped systemd-boot update (meta-only staging)");
            Ok(false)
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn writes_loader_entries() {
        let dir = tempdir().unwrap();
        let esp_root = dir.path().join("efi");
        fs::create_dir_all(esp_root.join("EFI/BOOT")).unwrap();
        let slot = dir.path().join("slotB");
        fs::create_dir_all(&slot).unwrap();
        fs::write(slot.join("kernel"), b"k").unwrap();
        fs::write(slot.join("initramfs"), b"i").unwrap();

        let esp = EspPaths {
            root: esp_root.clone(),
        };
        activate_slot(&esp, &slot, BootSlot::B, "console=ttyS0 rdinit=/init").unwrap();

        let entry = fs::read_to_string(esp.entry_path(BootSlot::B)).unwrap();
        assert!(entry.contains("linux /pertisk/B/kernel"));
        let loader = fs::read_to_string(esp.loader_conf()).unwrap();
        assert!(loader.contains("default pertisk-b.conf"));
        assert!(esp_root.join("pertisk/B/kernel").exists());
    }
}
