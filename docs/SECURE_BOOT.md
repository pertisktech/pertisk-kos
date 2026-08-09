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
| TPM PCR attestation | done (lab) — sysfs PCR read via `MachineService.Attest` / `pertiskctl attest` |
| TPM2 Quote | done (lab) — pure-Rust `/dev/tpmrm0` Quote + `pertiskctl quote --verify` |

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

## TPM PCR attestation (lab)

Pertisk exposes a **read-only** attestation snapshot over gRPC (no `libtss2` in the image):

- Source: Linux sysfs `/sys/class/tpm/tpm0/pcr-sha256/{N}`
- Indices: **0–7** (firmware / Secure Boot) and **11** (UKI stub when measured)

## TPM2 Quote (lab)

Pure-Rust Quote over `/dev/tpmrm0` (fallback `/dev/tpm0`) — no `tpm2-tools` / `libtss2`:

- Ephemeral ECC NIST-P256 restricted signing AK per request
- `MachineService.Quote` + `pertiskctl quote [--verify] [--nonce HEX]`
- Local verify checks ECDSA signature, nonce (`extraData`), and PCR composite digest vs sysfs

Still later: persistent AK / EK certs, mgmt UI remote trust store.

```bash
# On a node with PERTISK_TPM=1 (or a real TPM):
./out/bin/pertiskctl -e 127.0.0.1:50000 quote --verify
```

### QEMU soft-TPM

Requires [`swtpm`](https://github.com/stefanberger/swtpm).

```bash
# amd64 example
PERTISK_TPM=1 ./image/run-qemu-uefi.sh

# With enrolled Secure Boot vars:
PERTISK_TPM=1 \
PERTISK_OVMF_VARS=out/secureboot/OVMF_VARS.secboot.fd \
  ./image/run-qemu-uefi.sh
```

If `swtpm` is missing, the script prints a warning and boots without a TPM.

Manual equivalent (amd64):

```bash
mkdir -p out/swtpm-amd64
swtpm socket --tpmstate dir=out/swtpm-amd64 \
  --ctrl type=unixio,path=out/swtpm-amd64/swtpm-sock \
  --tpm2 --daemon
qemu-system-x86_64 … \
  -chardev socket,id=chrtpm,path=out/swtpm-amd64/swtpm-sock \
  -tpmdev emulator,id=tpm0,chardev=chrtpm \
  -device tpm-tis,tpmdev=tpm0
```

On arm64 use `-device tpm-tis-device,tpmdev=tpm0` instead of `tpm-tis`.

### Verify from the host

```bash
./out/bin/pertiskctl -e <guest-ip>:50000 attest
# available=true slot=A version=… — read N SHA-256 PCR(s) from …
# PCR    ALGO     DIGEST
# 0      sha256   …

./out/bin/pertiskctl -e <guest-ip>:50000 quote --verify
# available=true … — quoted 9 PCR(s) via /dev/tpmrm0 …
# verify=ok
```

Without a TPM device the RPCs still succeed with `available=false` and an explanatory message.

## Upgrade path note

A/B upgrades still stage `kernel` + `initramfs` (and optional `uki`) into the inactive slot directory. When `uki` is present, `activate_slot` installs the PE under `EFI/Linux/` and flips `loader.conf`. Bundle format can add a `uki` artifact in a later revision; until then, operators can place `uki` beside kernel/initramfs in the slot staging dir.

## Related

- Hardening checklist: [HARDENING.md](./HARDENING.md)
- Bootloader code: `crates/pertisk-update/src/bootloader.rs`
