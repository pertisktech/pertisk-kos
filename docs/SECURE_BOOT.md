# Secure Boot / UKI

Pertisk boots with **systemd-boot** today (kernel + initramfs on the ESP). A **Unified Kernel Image (UKI)** packs kernel + initramfs + cmdline into one PE/COFF `.efi`, which is the unit Secure Boot signs and measures.

## Status

| Step | Status |
|------|--------|
| Signed A/B OS bundles (Ed25519) | done |
| systemd-boot A/B ESP entries | done |
| Build UKI (`ukify`) | done — `./image/build-uki.sh` |
| ESP install UKI (`EFI/Linux/pertisk-{a,b}.efi`) | done — when `uki` present in slot/assets |
| Sign UKI with db key | done — optional `PERTISK_SB_*` |
| Enroll PK/KEK/db in firmware | done — `./scripts/enroll-ovmf-vars.sh` (or manual OVMF UI) |
| TPM PCR attestation | todo |

## Build a UKI

```bash
./image/fetch-kernel.sh
./image/build-initramfs.sh                 # production profile
./image/build-uki.sh                       # → out/uki/pertisk-amd64.efi
PERTISK_ARCH=arm64 ./image/build-uki.sh    # → out/uki/pertisk-arm64.efi

# or: make uki ARCH=amd64
```

Embed into the installer initramfs (first-boot ESP bootstrap prefers UKI):

```bash
./image/build-uki.sh
PERTISK_EMBED_BOOT=1 PERTISK_EMBED_UKI=1 ./image/build-initramfs.sh
```

Loader entry shape (written by `pertisk-update`):

```
title Pertisk KOS UKI (slot A)
efi /EFI/Linux/pertisk-a.efi
```

Classic `linux`/`initrd` entries remain the default when no `uki` file is staged.

## Sign for Secure Boot (lab)

```bash
./scripts/gen-secureboot-keys.sh           # → out/secureboot/{PK,KEK,db}.*
PERTISK_SB_KEY=out/secureboot/db.key \
PERTISK_SB_CERT=out/secureboot/db.crt \
  ./image/build-uki.sh
```

Private keys are **test-only**. Production must use org PKI / HSM.

## Enroll keys in OVMF (QEMU)

### Automated (recommended)

Requires [`virt-fw-vars`](https://pypi.org/project/virt-firmware/) (`pip install virt-firmware` or `python3-virt-firmware`).

```bash
./scripts/gen-secureboot-keys.sh
./scripts/enroll-ovmf-vars.sh                 # → out/secureboot/OVMF_VARS.secboot.fd
./scripts/enroll-ovmf-vars.sh --arch arm64    # → out/secureboot/AAVMF_VARS.secboot.fd

# Sign UKI with the same db key, then boot with enrolled vars:
PERTISK_SB_KEY=out/secureboot/db.key \
PERTISK_SB_CERT=out/secureboot/db.crt \
  make uki ARCH=amd64

PERTISK_OVMF_VARS=out/secureboot/OVMF_VARS.secboot.fd \
  ./image/run-qemu-uefi.sh
```

Override the blank template with `PERTISK_OVMF_VARS_TEMPLATE=/path/to/VARS.fd` if auto-detect misses your edk2 package.

### Manual (firmware UI)

1. Start QEMU with OVMF **and** a writable vars store (see `image/run-qemu-uefi.sh`).
2. Enter firmware setup (often Esc / F2 during POST) → Secure Boot / Device Manager.
3. Enter **Setup Mode** (delete Platform Key) if needed.
4. Enroll:
   - **PK** ← `out/secureboot/PK.cer`
   - **KEK** ← `out/secureboot/KEK.cer`
   - **db** ← `out/secureboot/db.cer`
5. Place the **db-signed** UKI on the ESP at `EFI/Linux/pertisk-a.efi` (or rebuild with `PERTISK_EMBED_UKI=1`).
6. Enable Secure Boot and reboot.

Without enrolled `db`, a signed UKI still boots when Secure Boot is off; with Secure Boot on, unsigned or foreign-signed images are rejected.

## Upgrade path note

A/B upgrades still stage `kernel` + `initramfs` (and optional `uki`) into the inactive slot directory. When `uki` is present, `activate_slot` installs the PE under `EFI/Linux/` and flips `loader.conf`. Bundle format can add a `uki` artifact in a later revision; until then, operators can place `uki` beside kernel/initramfs in the slot staging dir.

## Related

- Hardening checklist: [HARDENING.md](./HARDENING.md)
- Bootloader code: `crates/pertisk-update/src/bootloader.rs`
