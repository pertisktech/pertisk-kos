//! GPT partition roles and filesystem mount points.

/// EFI system partition label.
pub const PARTLABEL_EFI: &str = "EFI";
/// Boot slot A (kernel + initramfs).
pub const PARTLABEL_BOOT_A: &str = "BOOT_A";
/// Boot slot B (kernel + initramfs).
pub const PARTLABEL_BOOT_B: &str = "BOOT_B";
/// Small metadata partition (boot counters, active slot).
pub const PARTLABEL_META: &str = "META";
/// Persistent machine state (config, certs, identity).
pub const PARTLABEL_STATE: &str = "STATE";
/// Scratch / container storage (wiped across upgrades when desired).
pub const PARTLABEL_EPHEMERAL: &str = "EPHEMERAL";

/// Logical role of a GPT partition in the Pertisk disk layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionRole {
    Efi,
    BootA,
    BootB,
    Meta,
    State,
    Ephemeral,
}

impl PartitionRole {
    pub fn partlabel(self) -> &'static str {
        match self {
            Self::Efi => PARTLABEL_EFI,
            Self::BootA => PARTLABEL_BOOT_A,
            Self::BootB => PARTLABEL_BOOT_B,
            Self::Meta => PARTLABEL_META,
            Self::State => PARTLABEL_STATE,
            Self::Ephemeral => PARTLABEL_EPHEMERAL,
        }
    }

    pub fn from_partlabel(label: &str) -> Option<Self> {
        match label {
            PARTLABEL_EFI => Some(Self::Efi),
            PARTLABEL_BOOT_A => Some(Self::BootA),
            PARTLABEL_BOOT_B => Some(Self::BootB),
            PARTLABEL_META => Some(Self::Meta),
            PARTLABEL_STATE => Some(Self::State),
            PARTLABEL_EPHEMERAL => Some(Self::Ephemeral),
            _ => None,
        }
    }
}

/// Canonical mount paths used by `pertiskd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountPaths {
    /// Persistent STATE mount (config, identity).
    pub state: &'static str,
    /// EFI System Partition (systemd-boot + A/B kernels).
    pub efi: &'static str,
    /// Writable ephemeral data (logs, containerd, kubelet).
    pub var: &'static str,
    /// Optional EPHEMERAL mount backing `/var`.
    pub ephemeral: &'static str,
}

impl Default for MountPaths {
    fn default() -> Self {
        Self::standard()
    }
}

impl MountPaths {
    pub const fn standard() -> Self {
        Self {
            state: "/system/state",
            efi: "/boot/efi",
            var: "/var",
            ephemeral: "/system/ephemeral",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partlabels_roundtrip() {
        for role in [
            PartitionRole::Efi,
            PartitionRole::BootA,
            PartitionRole::BootB,
            PartitionRole::Meta,
            PartitionRole::State,
            PartitionRole::Ephemeral,
        ] {
            assert_eq!(PartitionRole::from_partlabel(role.partlabel()), Some(role));
        }
    }
}
