//! Disk layout, STATE volume, and mount helpers for Pertisk KOS.
//!
//! Milestone M1: discover/mount STATE (or use a directory in dev mode),
//! ensure layout, and resolve the machine-config path.

mod layout;
mod state;

pub use layout::{
    MountPaths, PartitionRole, PARTLABEL_BOOT_A, PARTLABEL_BOOT_B, PARTLABEL_EFI,
    PARTLABEL_EPHEMERAL, PARTLABEL_META, PARTLABEL_STATE,
};
pub use state::{
    prepare_state, StateError, StateSource, StateVolume, DEFAULT_CONFIG_NAME,
};
