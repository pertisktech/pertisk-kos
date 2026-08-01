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

/// CPU architecture for EFI binary naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EfiArch {
    X64,
    Aa64,
}

impl EfiArch {
    pub fn detect() -> Self {
        match std::env::consts::ARCH {
            "aarch64" => Self::Aa64,
            _ => Self::X64,
        }
    }

    pub fn boot_efi_name(self) -> &'static str {
        match self {
            Self::X64 => "BOOTX64.EFI",
            Self::Aa64 => "BOOTAA64.EFI",
        }
    }

    pub fn systemd_boot_name(self) -> &'static str {
        match self {
            Self::X64 => "systemd-bootx64.efi",
            Self::Aa64 => "systemd-bootaa64.efi",
        }
    }
}

/// Assets used to seed a fresh ESP on first install.
#[derive(Debug, Clone)]
pub struct BootAssets {
    pub kernel: PathBuf,
    pub initramfs: PathBuf,
    /// Optional pre-built UKI (`pertisk.efi`); when set, loader entries use `efi` path.
    pub uki: Option<PathBuf>,
    /// systemd-boot EFI binary (e.g. systemd-bootx64.efi).
    pub bootloader_efi: PathBuf,
    pub arch: EfiArch,
    pub cmdline: String,
}

impl BootAssets {
    /// Resolve installer assets from `/usr/lib/pertisk/boot/` (initramfs overlay).
    pub fn from_installer_dir(dir: impl AsRef<Path>) -> Result<Self, BootloaderError> {
        let dir = dir.as_ref();
        let arch = EfiArch::detect();
        let kernel = dir.join("kernel");
        let initramfs = dir.join("initramfs");
        let uki = ["uki.efi", "pertisk.efi"]
            .into_iter()
            .map(|n| dir.join(n))
            .find(|p| p.exists());
        let bootloader_efi = ["BOOTX64.EFI", "BOOTAA64.EFI", arch.systemd_boot_name()]
            .into_iter()
            .map(|n| dir.join(n))
            .find(|p| p.exists())
            .ok_or_else(|| {
                BootloaderError::Msg(format!("no EFI bootloader binary under {}", dir.display()))
            })?;

        for (name, path) in [("kernel", &kernel), ("initramfs", &initramfs)] {
            if !path.exists() {
                return Err(BootloaderError::Msg(format!(
                    "missing {name} at {}",
                    path.display()
                )));
            }
        }

        Ok(Self {
            kernel,
            initramfs,
            uki,
            bootloader_efi,
            arch,
            cmdline: "console=ttyS0 console=tty0 rdinit=/init".into(),
        })
    }

    pub fn with_cmdline(mut self, cmdline: impl Into<String>) -> Self {
        self.cmdline = cmdline.into();
        self
    }

    pub fn with_uki(mut self, uki: impl Into<PathBuf>) -> Self {
        self.uki = Some(uki.into());
        self
    }
}

/// Default installer asset directory embedded in the OS image.
pub const INSTALLER_BOOT_DIR: &str = "/usr/lib/pertisk/boot";

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
            if p.join("loader").is_dir() || p.join("EFI").is_dir() || p.join("EFI/BOOT").is_dir() {
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
        self.root.join("pertisk").join(slot.as_str().to_uppercase())
    }

    /// Path for a slot UKI under `EFI/Linux/`.
    pub fn slot_uki_path(&self, slot: BootSlot) -> PathBuf {
        self.root
            .join("EFI/Linux")
            .join(format!("pertisk-{}.efi", slot.as_str().to_lowercase()))
    }
}

/// Install systemd-boot removable-path EFI binary onto the ESP.
pub fn install_systemd_boot(
    esp: &EspPaths,
    bootloader_efi: &Path,
    arch: EfiArch,
) -> Result<(), BootloaderError> {
    if !bootloader_efi.exists() {
        return Err(BootloaderError::Msg(format!(
            "bootloader EFI missing: {}",
            bootloader_efi.display()
        )));
    }
    let dest_dir = esp.root.join("EFI/BOOT");
    fs::create_dir_all(&dest_dir)?;
    let dest = dest_dir.join(arch.boot_efi_name());
    fs::copy(bootloader_efi, &dest)?;
    // Also install under EFI/systemd for clarity.
    let systemd_dir = esp.root.join("EFI/systemd");
    fs::create_dir_all(&systemd_dir)?;
    fs::copy(bootloader_efi, systemd_dir.join(arch.systemd_boot_name()))?;
    info!(dest = %dest.display(), "installed systemd-boot EFI");
    Ok(())
}

/// First-boot ESP populate: systemd-boot + slot A (UKI or kernel/initramfs) + loader entries.
pub fn bootstrap_esp(esp: &EspPaths, assets: &BootAssets) -> Result<(), BootloaderError> {
    install_systemd_boot(esp, &assets.bootloader_efi, assets.arch)?;

    let staging = esp.root.join(".bootstrap-slot");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;
    fs::copy(&assets.kernel, staging.join("kernel"))?;
    fs::copy(&assets.initramfs, staging.join("initramfs"))?;
    if let Some(ref uki) = assets.uki {
        fs::copy(uki, staging.join("uki"))?;
    }

    activate_slot(esp, &staging, BootSlot::A, &assets.cmdline)?;
    fs::remove_dir_all(&staging)?;
    info!(
        esp = %esp.root.display(),
        uki = assets.uki.is_some(),
        "ESP bootstrap complete (slot A)"
    );
    Ok(())
}

/// Copy slot artifacts onto the ESP and point systemd-boot at `next`.
///
/// If `slot_dir/uki` exists, installs a UKI under `EFI/Linux/` and writes an
/// `efi`-style loader entry. Otherwise uses classic `linux` + `initrd` paths.
pub fn activate_slot(
    esp: &EspPaths,
    slot_dir: &Path,
    next: BootSlot,
    cmdline: &str,
) -> Result<(), BootloaderError> {
    let image_dir = esp.slot_image_dir(next);
    fs::create_dir_all(&image_dir)?;
    fs::create_dir_all(esp.root.join("loader/entries"))?;

    let uki_src = slot_dir.join("uki");
    if uki_src.exists() {
        let dest = esp.slot_uki_path(next);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&uki_src, &dest)?;
        // Keep classic copies for rollback tooling / non-UKI inspect.
        for name in ["kernel", "initramfs"] {
            let src = slot_dir.join(name);
            if src.exists() {
                fs::copy(&src, image_dir.join(name))?;
            }
        }
        write_uki_entry(esp, next)?;
    } else {
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
    }

    write_loader_default(esp, next)?;
    info!(
        slot = %next,
        esp = %esp.root.display(),
        uki = uki_src.exists(),
        "systemd-boot default updated"
    );
    Ok(())
}

fn write_entry(esp: &EspPaths, slot: BootSlot, cmdline: &str) -> Result<(), BootloaderError> {
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
    Ok(())
}

fn write_uki_entry(esp: &EspPaths, slot: BootSlot) -> Result<(), BootloaderError> {
    let slot_u = slot.as_str().to_uppercase();
    let slot_l = slot.as_str().to_lowercase();
    // Cmdline is embedded inside the UKI; entry only points at the PE image.
    let body = format!(
        "title Pertisk KOS UKI (slot {slot_u})\n\
         efi /EFI/Linux/pertisk-{slot_l}.efi\n"
    );
    let path = esp.entry_path(slot);
    let mut f = fs::File::create(path)?;
    f.write_all(body.as_bytes())?;
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

    #[test]
    fn bootstrap_installs_removable_path() {
        let dir = tempdir().unwrap();
        let esp_root = dir.path().join("efi");
        fs::create_dir_all(&esp_root).unwrap();
        let assets_dir = dir.path().join("assets");
        fs::create_dir_all(&assets_dir).unwrap();
        fs::write(assets_dir.join("kernel"), b"k").unwrap();
        fs::write(assets_dir.join("initramfs"), b"i").unwrap();
        fs::write(assets_dir.join("BOOTX64.EFI"), b"efi").unwrap();

        let assets = BootAssets::from_installer_dir(&assets_dir).unwrap();
        let esp = EspPaths {
            root: esp_root.clone(),
        };
        bootstrap_esp(&esp, &assets).unwrap();

        assert!(esp_root
            .join("EFI/BOOT")
            .join(EfiArch::detect().boot_efi_name())
            .exists());
        assert!(esp_root.join("pertisk/A/kernel").exists());
        let loader = fs::read_to_string(esp_root.join("loader/loader.conf")).unwrap();
        assert!(loader.contains("default pertisk-a.conf"));
    }

    #[test]
    fn activate_uki_writes_efi_entry() {
        let dir = tempdir().unwrap();
        let esp_root = dir.path().join("efi");
        fs::create_dir_all(esp_root.join("EFI/BOOT")).unwrap();
        let slot = dir.path().join("slotA");
        fs::create_dir_all(&slot).unwrap();
        fs::write(slot.join("kernel"), b"k").unwrap();
        fs::write(slot.join("initramfs"), b"i").unwrap();
        fs::write(slot.join("uki"), b"uki-pe").unwrap();

        let esp = EspPaths {
            root: esp_root.clone(),
        };
        activate_slot(&esp, &slot, BootSlot::A, "ignored").unwrap();

        let entry = fs::read_to_string(esp.entry_path(BootSlot::A)).unwrap();
        assert!(entry.contains("efi /EFI/Linux/pertisk-a.efi"));
        assert!(!entry.contains("linux /pertisk"));
        assert_eq!(fs::read(esp.slot_uki_path(BootSlot::A)).unwrap(), b"uki-pe");
    }
}
