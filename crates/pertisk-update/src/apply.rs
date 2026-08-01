//! Apply a verified bundle into the inactive A/B slot and update boot meta.

use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::info;

use crate::bootloader::{try_activate_slot, BootloaderError};
use crate::bundle::{verify_bundle, BundleError, VerifiedBundle};
use crate::meta::{BootMeta, BootSlot, MetaError, MAX_BOOT_ATTEMPTS};

#[derive(Debug, Error)]
pub enum ApplyError {
    #[error(transparent)]
    Bundle(#[from] BundleError),
    #[error(transparent)]
    Meta(#[from] MetaError),
    #[error(transparent)]
    Bootloader(#[from] BootloaderError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Msg(String),
}

#[derive(Debug, Clone)]
pub struct ApplyResult {
    pub target_slot: BootSlot,
    pub version: String,
    pub slot_dir: PathBuf,
    /// True when systemd-boot / ESP was updated.
    pub bootloader_updated: bool,
}

/// Paths used when staging OS images for a slot.
#[derive(Debug, Clone)]
pub struct SlotLayout {
    pub state_root: PathBuf,
    pub slots_root: PathBuf,
    pub trust_public_key: PathBuf,
    /// Optional ESP mount override (default: auto-discover).
    pub esp_root: Option<PathBuf>,
    /// Kernel cmdline written into systemd-boot entries.
    pub cmdline: String,
}

impl SlotLayout {
    pub fn new(state_root: impl Into<PathBuf>, trust_public_key: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            slots_root: state_root.join("slots"),
            trust_public_key: trust_public_key.into(),
            state_root,
            esp_root: None,
            cmdline: "console=ttyS0 console=tty0 rdinit=/init".into(),
        }
    }

    pub fn with_esp(mut self, esp: impl Into<PathBuf>) -> Self {
        self.esp_root = Some(esp.into());
        self
    }

    pub fn with_cmdline(mut self, cmdline: impl Into<String>) -> Self {
        self.cmdline = cmdline.into();
        self
    }

    pub fn slot_dir(&self, slot: BootSlot) -> PathBuf {
        self.slots_root.join(slot.as_str())
    }
}

/// Verify bundle signature/hashes, copy into inactive slot, set `next` + pending version.
pub fn apply_bundle(layout: &SlotLayout, bundle_dir: &Path) -> Result<ApplyResult, ApplyError> {
    let verified = verify_bundle(bundle_dir, &layout.trust_public_key)?;
    let mut meta = BootMeta::load(&layout.state_root)?;
    let target = meta.active.other();
    stage_slot(layout, target, &verified)?;

    let bootloader_updated = try_activate_slot(
        &layout.slot_dir(target),
        target,
        &layout.cmdline,
        layout.esp_root.as_deref(),
    )?;

    meta.previous_good = meta.active;
    meta.next = target;
    meta.boot_ok = false;
    meta.boot_attempts = 0;
    meta.pending_version = Some(verified.manifest.version.clone());
    meta.save(&layout.state_root)?;

    info!(
        slot = %target,
        version = %verified.manifest.version,
        bootloader_updated,
        "upgrade staged; reboot to activate"
    );

    Ok(ApplyResult {
        target_slot: target,
        version: verified.manifest.version,
        slot_dir: layout.slot_dir(target),
        bootloader_updated,
    })
}

fn stage_slot(
    layout: &SlotLayout,
    slot: BootSlot,
    bundle: &VerifiedBundle,
) -> Result<(), ApplyError> {
    let dest = layout.slot_dir(slot);
    if dest.exists() {
        fs::remove_dir_all(&dest)?;
    }
    fs::create_dir_all(&dest)?;

    for name in bundle.manifest.artifacts.keys() {
        let src = bundle.root.join(name);
        let dst = dest.join(name);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&src, &dst)?;
    }
    fs::copy(
        bundle.root.join("manifest.json"),
        dest.join("manifest.json"),
    )?;
    fs::copy(bundle.root.join("manifest.sig"), dest.join("manifest.sig"))?;
    Ok(())
}

/// Called early in boot: bump attempts; rollback next slot if attempts exhausted.
pub fn record_boot_attempt(state_root: &Path) -> Result<BootMeta, ApplyError> {
    record_boot_attempt_with_layout(state_root, None)
}

/// Like [`record_boot_attempt`], optionally flipping systemd-boot back on rollback.
pub fn record_boot_attempt_with_layout(
    state_root: &Path,
    layout: Option<&SlotLayout>,
) -> Result<BootMeta, ApplyError> {
    let mut meta = BootMeta::load(state_root)?;

    // Activate pending slot if next differs from active.
    if meta.next != meta.active {
        meta.active = meta.next;
        if let Some(v) = meta.pending_version.take() {
            meta.active_version = Some(v);
        }
    }

    if meta.boot_ok {
        return Ok(meta);
    }

    meta.boot_attempts = meta.boot_attempts.saturating_add(1);
    if meta.boot_attempts >= MAX_BOOT_ATTEMPTS {
        info!(
            attempts = meta.boot_attempts,
            rollback_to = %meta.previous_good,
            "boot attempts exceeded; rolling back"
        );
        let rollback_to = meta.previous_good;
        meta.next = rollback_to;
        meta.active = rollback_to;
        meta.boot_attempts = 0;
        meta.boot_ok = true;
        meta.pending_version = None;

        if let Some(layout) = layout {
            let slot_dir = layout.slot_dir(rollback_to);
            if slot_dir.join("kernel").exists() {
                let _ = try_activate_slot(
                    &slot_dir,
                    rollback_to,
                    &layout.cmdline,
                    layout.esp_root.as_deref(),
                )?;
            }
        }
    }
    meta.save(state_root)?;
    Ok(meta)
}

/// Mark current boot as healthy (disables auto-rollback).
pub fn mark_boot_good(state_root: &Path) -> Result<BootMeta, ApplyError> {
    let mut meta = BootMeta::load(state_root)?;
    meta.boot_ok = true;
    meta.boot_attempts = 0;
    meta.previous_good = meta.active;
    meta.pending_version = None;
    meta.save(state_root)?;
    info!(slot = %meta.active, "boot marked good");
    Ok(meta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::build_manifest;
    use crate::sign::{generate_keypair, load_signing_key, sign_manifest};
    use tempfile::tempdir;

    fn make_signed_bundle(dir: &Path) -> (PathBuf, PathBuf) {
        let bundle = dir.join("bundle");
        fs::create_dir_all(&bundle).unwrap();
        fs::write(bundle.join("kernel"), b"k").unwrap();
        fs::write(bundle.join("initramfs"), b"i").unwrap();
        let sk = dir.join("os.sk");
        let pk = dir.join("os.pk");
        generate_keypair(&sk, &pk).unwrap();
        let manifest = build_manifest(&bundle, "0.2.0", &["kernel", "initramfs"]).unwrap();
        let json = manifest.to_canonical_json().unwrap();
        fs::write(bundle.join("manifest.json"), &json).unwrap();
        let sig = sign_manifest(&load_signing_key(&sk).unwrap(), &json);
        fs::write(bundle.join("manifest.sig"), sig).unwrap();
        (bundle, pk)
    }

    #[test]
    fn apply_and_rollback_flow() {
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        fs::create_dir_all(&state).unwrap();
        let (bundle, pk) = make_signed_bundle(dir.path());
        let layout = SlotLayout::new(&state, &pk);

        let result = apply_bundle(&layout, &bundle).unwrap();
        assert_eq!(result.target_slot, BootSlot::B);
        assert!(layout.slot_dir(BootSlot::B).join("kernel").exists());

        let meta = BootMeta::load(&state).unwrap();
        assert_eq!(meta.next, BootSlot::B);
        assert!(!meta.boot_ok);

        // Simulate failed boots until rollback.
        for _ in 0..MAX_BOOT_ATTEMPTS {
            let _ = record_boot_attempt(&state).unwrap();
        }
        let meta = BootMeta::load(&state).unwrap();
        assert_eq!(meta.next, BootSlot::A);
        assert!(meta.boot_ok);
    }

    #[test]
    fn mark_good_clears_attempts() {
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        fs::create_dir_all(&state).unwrap();
        let (bundle, pk) = make_signed_bundle(dir.path());
        apply_bundle(&SlotLayout::new(&state, &pk), &bundle).unwrap();
        record_boot_attempt(&state).unwrap();
        let meta = mark_boot_good(&state).unwrap();
        assert!(meta.boot_ok);
        assert_eq!(meta.boot_attempts, 0);
        assert_eq!(meta.active, BootSlot::B);
    }

    #[test]
    fn apply_updates_bootloader_when_esp_present() {
        let dir = tempdir().unwrap();
        let state = dir.path().join("state");
        let esp = dir.path().join("efi");
        fs::create_dir_all(esp.join("EFI/BOOT")).unwrap();
        fs::create_dir_all(&state).unwrap();
        let (bundle, pk) = make_signed_bundle(dir.path());
        let layout = SlotLayout::new(&state, &pk).with_esp(&esp);

        let result = apply_bundle(&layout, &bundle).unwrap();
        assert!(result.bootloader_updated);
        let loader = fs::read_to_string(esp.join("loader/loader.conf")).unwrap();
        assert!(loader.contains("default pertisk-b.conf"));
    }
}
