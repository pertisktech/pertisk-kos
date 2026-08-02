//! Disk layout planning: fixed-size partitions + EPHEMERAL consuming the rest.

use crate::layout::PartitionRole;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// Filesystem to create on a partition after GPT install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsType {
    Vfat,
    Ext4,
    /// Unformatted (bootloader / future use).
    None,
}

/// One partition in the planned layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionSpec {
    pub role: PartitionRole,
    /// Size in bytes. `None` means "all remaining space" (EPHEMERAL).
    pub size: Option<u64>,
    pub fstype: FsType,
}

/// Full disk plan for a target device size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskPlan {
    pub disk_size: u64,
    pub partitions: Vec<PartitionSpec>,
}

/// Default fixed sizes.
///
/// ESP must hold systemd-boot + A/B `kernel`+`initramfs` (runtime-embedded
/// initramfs is ~160MiB+). 100MiB was enough for boot-only images; 512MiB
/// fits one full slot today with room for a second slot / UKI growth.
pub fn default_fixed_partitions() -> Vec<PartitionSpec> {
    vec![
        PartitionSpec {
            role: PartitionRole::Efi,
            size: Some(512 * MIB),
            fstype: FsType::Vfat,
        },
        PartitionSpec {
            role: PartitionRole::BootA,
            size: Some(768 * MIB),
            fstype: FsType::Ext4,
        },
        PartitionSpec {
            role: PartitionRole::BootB,
            size: Some(768 * MIB),
            fstype: FsType::Ext4,
        },
        PartitionSpec {
            role: PartitionRole::Meta,
            size: Some(32 * MIB),
            fstype: FsType::Ext4,
        },
        PartitionSpec {
            role: PartitionRole::State,
            size: Some(GIB),
            fstype: FsType::Ext4,
        },
        PartitionSpec {
            role: PartitionRole::Ephemeral,
            size: None,
            fstype: FsType::Ext4,
        },
    ]
}

/// Minimum disk size required by [`default_fixed_partitions`] (plus GPT overhead).
pub fn minimum_disk_size() -> u64 {
    let fixed: u64 = default_fixed_partitions()
        .iter()
        .filter_map(|p| p.size)
        .sum();
    // 256 MiB minimum ephemeral + 2 MiB GPT/alignment slack
    fixed + 256 * MIB + 2 * MIB
}

/// Build a concrete plan for `disk_size`, assigning remaining bytes to EPHEMERAL.
pub fn plan_disk(disk_size: u64) -> Result<DiskPlan, PlanError> {
    let min = minimum_disk_size();
    if disk_size < min {
        return Err(PlanError::TooSmall {
            disk_size,
            minimum: min,
        });
    }

    let mut partitions = default_fixed_partitions();
    let fixed: u64 = partitions.iter().filter_map(|p| p.size).sum();
    let remaining = disk_size.saturating_sub(fixed + 2 * MIB);

    for part in &mut partitions {
        if part.size.is_none() {
            part.size = Some(remaining);
        }
    }

    Ok(DiskPlan {
        disk_size,
        partitions,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("disk too small: have {disk_size} bytes, need at least {minimum}")]
    TooSmall { disk_size: u64, minimum: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_8g_disk() {
        let plan = plan_disk(8 * GIB).unwrap();
        assert_eq!(plan.partitions.len(), 6);
        let ephemeral = plan
            .partitions
            .iter()
            .find(|p| p.role == PartitionRole::Ephemeral)
            .unwrap();
        assert!(ephemeral.size.unwrap() > 256 * MIB);
        let total: u64 = plan.partitions.iter().map(|p| p.size.unwrap()).sum();
        assert!(total < 8 * GIB);
    }

    #[test]
    fn rejects_tiny_disk() {
        assert!(plan_disk(100 * MIB).is_err());
    }
}
