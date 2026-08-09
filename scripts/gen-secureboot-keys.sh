#!/usr/bin/env bash
# Generate test Secure Boot keys (PK / KEK / db) for OVMF lab enrollment.
# NOT for production — use HSM / org PKI there.
#
#   ./scripts/gen-secureboot-keys.sh
#   # → out/secureboot/{PK,KEK,db}.{key,crt,esl,auth}
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out/secureboot"
CN_BASE="${PERTISK_SB_CN:-Pertisk KOS Test}"

mkdir -p "${OUT}"
cd "${OUT}"

if ! command -v openssl >/dev/null 2>&1; then
  echo "openssl required" >&2
  exit 1
fi

gen_cert() {
  local name="$1"
  local cn="$2"
  if [[ -f "${name}.key" && -f "${name}.crt" ]]; then
    echo "keep existing ${name}.key / ${name}.crt"
    return
  fi
  openssl req -new -x509 -newkey rsa:2048 -subj "/CN=${cn}/" \
    -keyout "${name}.key" -out "${name}.crt" -days 3650 -nodes -sha256 2>/dev/null
  chmod 600 "${name}.key"
  echo "wrote ${name}.key ${name}.crt"
}

echo "==> generating Secure Boot test keys in ${OUT}"
gen_cert PK "${CN_BASE} PK"
gen_cert KEK "${CN_BASE} KEK"
gen_cert db "${CN_BASE} db"

# Optional DER for tools that want it.
for name in PK KEK db; do
  openssl x509 -in "${name}.crt" -outform DER -out "${name}.cer" 2>/dev/null || true
done

cat >README.md <<'EOF'
# Pertisk Secure Boot test keys

Lab-only keys. Do not ship private keys in production images.

## Sign a UKI

```bash
PERTISK_SB_KEY=out/secureboot/db.key \
PERTISK_SB_CERT=out/secureboot/db.crt \
  ./image/build-uki.sh
```

## Enroll in OVMF (QEMU)

Automated:

```bash
./scripts/enroll-ovmf-vars.sh
# or: make enroll-ovmf
PERTISK_OVMF_VARS=out/secureboot/OVMF_VARS.secboot.fd ./image/run-qemu-uefi.sh
```

Manual:

1. Boot OVMF with Secure Boot disabled / setup mode (empty PK).
2. Use the firmware UI (or `virt-fw-vars`) to enroll:
   - PK ← `PK.cer`
   - KEK ← `KEK.cer`
   - db ← `db.cer`
3. Rebuild UKI signed with `db.key`, place on ESP as `EFI/Linux/pertisk-a.efi`.
4. Enable Secure Boot and reboot.

See [docs/SECURE_BOOT.md](../../docs/SECURE_BOOT.md).
EOF

echo "==> done"
ls -lh "${OUT}"
