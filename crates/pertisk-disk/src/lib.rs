//! Disk layout, STATE volume, install, and mount helpers for Pertisk KOS.

mod ephemeral;
mod esp;
mod install;
mod layout;
mod partlabel;
mod plan;
mod state;

pub use ephemeral::{
    prepare_ephemeral, prepare_ephemeral_at, try_prepare_ephemeral, EphemeralError, EphemeralVolume,
};
pub use esp::{prepare_esp, prepare_esp_at, try_prepare_esp, EspError, EspVolume};
pub use install::{
    disk_size, install_disk, layout_present, partition_node, plan_install, InstallError,
    InstallOptions,
};
pub use layout::{
    MountPaths, PartitionRole, PARTLABEL_BOOT_A, PARTLABEL_BOOT_B, PARTLABEL_EFI,
    PARTLABEL_EPHEMERAL, PARTLABEL_META, PARTLABEL_STATE,
};
pub use partlabel::{find_by_partlabel, settle_block_devices, wait_for_partlabel};
pub use plan::{
    default_fixed_partitions, minimum_disk_size, plan_disk, DiskPlan, FsType, PartitionSpec,
    PlanError,
};
pub use state::{prepare_state, StateError, StateSource, StateVolume, DEFAULT_CONFIG_NAME};
