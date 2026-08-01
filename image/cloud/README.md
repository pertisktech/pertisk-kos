# Cloud / golden disk images

Build a **pre-installed** GPT disk (raw + qcow2) with EFI systemd-boot, slot A
kernel/initramfs, and seeded STATE — suitable for QEMU UEFI, AWS AMI import,
GCP custom images, and Azure VHD conversion.

## Build

```bash
./image/fetch-kernel.sh
./image/fetch-bootloader.sh
PERTISK_EMBED_BOOT=1 ./image/build-initramfs.sh   # provides initramfs + boot assets

./image/build-cloud-image.sh
# → out/pertisk-cloud-amd64.raw
# → out/pertisk-cloud-amd64.qcow2
```

Optional:

```bash
PERTISK_DISK_GB=20 PERTISK_SEED_CONFIG=examples/worker-cloud.yaml ./image/build-cloud-image.sh
PERTISK_ARCH=arm64 ./image/build-cloud-image.sh
```

Seed config must **not** set `machine.install` (image is already laid out).

## Local UEFI boot

```bash
PERTISK_DISK=out/pertisk-cloud-amd64.raw ./image/run-qemu-uefi.sh
# arm64 disk auto-selects qemu-system-aarch64 + AAVMF:
PERTISK_DISK=out/pertisk-cloud-arm64.raw ./image/run-qemu-uefi.sh
```

## Cloud upload (operator outline)

### AWS

1. Upload `pertisk-cloud-amd64.raw` (or qcow2) to S3.
2. `ec2 import-image` / Image Builder with **UEFI** boot mode.
3. Register AMI; launch with user-data only if you later add a cloud config path.
   Today: bake join settings into STATE via `PERTISK_SEED_CONFIG`, or replace
   STATE `config.yaml` before import.

### GCP

1. Compress raw: `tar --format=oldgnu -Sczf disk.tar.gz pertisk-cloud-amd64.raw`
2. `gcloud compute images create pertisk-kos --source-file=disk.tar.gz --guest-os-features=UEFI_COMPATIBLE`
3. Create instance from the image.

### Azure

1. Convert: `qemu-img convert -f raw -O vpc -o force_size pertisk-cloud-amd64.raw pertisk.vhd`
2. Upload VHD to a page blob storage account.
3. Create image with UEFI generation 2 VM support; deploy VM.

## Layout

Same as metal install: `EFI`, `BOOT_A`, `BOOT_B`, `META`, `STATE`, `EPHEMERAL`.
ESP holds `EFI/BOOT/BOOT*.EFI`, `loader/`, and `pertisk/A/{kernel,initramfs}`.
