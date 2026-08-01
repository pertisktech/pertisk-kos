//! Disk layout, STATE volume, install, and mount helpers for Pertisk KOS.

mod esp;
mod install;
mod layout;
mod plan;
mod state;

pub use esp::{prepare_esp, prepare_esp_at, try_prepare_esp, EspError, EspVolume};
pub use install::{
    disk_size, install_disk, layout_present, partition_node, plan_install, InstallError,
    InstallOptions,
};
pub use layout::{
    MountPaths, PartitionRole, PARTLABEL_BOOT_A, PARTLABEL_BOOT_B, PARTLABEL_EFI,
    PARTLABEL_EPHEMERAL, PARTLABEL_META, PARTLABEL_STATE,
};
pub use plan::{
    default_fixed_partitions, minimum_disk_size, plan_disk, DiskPlan, FsType, PartitionSpec,
    PlanError,
};
pub use state::{
    prepare_state, StateError, StateSource, StateVolume, DEFAULT_CONFIG_NAME,
};
