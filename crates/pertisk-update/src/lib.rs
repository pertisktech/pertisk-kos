//! Signed A/B OS updates for Pertisk KOS (M5+).

mod apply;
mod bootloader;
mod bundle;
mod meta;
mod sign;

pub use apply::{
    apply_bundle, mark_boot_good, record_boot_attempt, record_boot_attempt_with_layout, ApplyError,
    ApplyResult, SlotLayout,
};
pub use bootloader::{
    activate_slot, bootstrap_esp, install_systemd_boot, try_activate_slot, BootAssets,
    BootloaderError, EfiArch, EspPaths, INSTALLER_BOOT_DIR,
};
pub use bundle::{
    build_manifest, sha256_file, verify_bundle, BundleError, BundleManifest, VerifiedBundle,
};
pub use meta::{BootMeta, BootSlot, MetaError, META_FILENAME};
pub use sign::{
    generate_keypair, load_signing_key, load_verifying_key, sign_manifest, verify_manifest,
    KeyError,
};
