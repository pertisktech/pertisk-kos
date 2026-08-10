#!/usr/bin/env bash
# Static CIS-ish hardening gate (CI-friendly; no QEMU / kube-bench required).
#
# Checks that kubelet config defaults, sysctls, metrics auth, and mount flags
# remain present in source. For a live node scan, see docs/HARDENING.md
# ("Running kube-bench").
#
#   ./scripts/check-hardening.sh
#   make check-hardening
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

fail=0
pass=0

check() {
  local label="$1"
  shift
  if "$@"; then
    echo "PASS  ${label}"
    pass=$((pass + 1))
  else
    echo "FAIL  ${label}"
    fail=$((fail + 1))
  fi
}

file_has() {
  local file="$1"
  local pattern="$2"
  [[ -f "${file}" ]] && grep -qE "${pattern}" "${file}"
}

echo "==> Pertisk hardening static checks"
echo

# --- Kubelet CIS 4.2.x (generated config template) ---
KCFG="crates/pertisk-kubelet/src/config.rs"
check "4.2.1 anonymous.enabled false" file_has "${KCFG}" 'enabled: false'
check "4.2.2 authorization Webhook" file_has "${KCFG}" 'mode: Webhook'
check "4.2.4 readOnlyPort 0" file_has "${KCFG}" 'readOnlyPort: 0'
check "4.2.5 streamingConnectionIdleTimeout" file_has "${KCFG}" 'streamingConnectionIdleTimeout: 5m'
check "4.2.6 protectKernelDefaults" file_has "${KCFG}" 'protectKernelDefaults: true'
check "4.2.7 makeIPTablesUtilChains" file_has "${KCFG}" 'makeIPTablesUtilChains: true'
check "4.2.10 rotateCertificates" file_has "${KCFG}" 'rotateCertificates: true'
check "4.2.11 serverTLSBootstrap" file_has "${KCFG}" 'serverTLSBootstrap: true'
check "4.2.12 tlsCipherSuites" file_has "${KCFG}" 'tlsCipherSuites:'
check "4.2.12 ECDHE AES-GCM suite" file_has "${KCFG}" 'TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256'

# --- File mode restrictions ---
check "kubeconfig mode 0600" file_has "${KCFG}" 'set_mode\(0o600\)'
check "STATE secrets 0700" file_has "crates/pertisk-disk/src/state.rs" 'set_mode\(0o700\)'

# --- Sysctls for protectKernelDefaults ---
SYSCTL="crates/pertiskd/src/sysctl.rs"
check "sysctl vm.overcommit_memory" file_has "${SYSCTL}" 'vm/overcommit_memory'
check "sysctl kernel.panic" file_has "${SYSCTL}" 'kernel/panic'
check "sysctl ip_forward" file_has "${SYSCTL}" 'net/ipv4/ip_forward'
check "sysctl max_user_namespaces" file_has "${SYSCTL}" 'user/max_user_namespaces'
check "sysctl applied before runtime" file_has "crates/pertiskd/src/main.rs" 'apply_hardening_sysctls'

# --- Metrics auth surface ---
check "metrics bearer support" file_has "crates/pertisk-api/src/metrics.rs" 'bearer_authorized'
check "metrics mTLS builder" file_has "crates/pertisk-api/src/metrics.rs" 'build_metrics_tls'
check "metrics mTLS client verifier" file_has "crates/pertisk-api/src/metrics.rs" 'WebPkiClientVerifier'
check "metrics token CLI/env" file_has "crates/pertiskd/src/main.rs" 'PERTISK_METRICS_TOKEN'
check "metrics token from STATE" file_has "crates/pertiskd/src/main.rs" 'secrets/metrics.token'
check "metrics TLS wired from pertiskd" file_has "crates/pertiskd/src/main.rs" 'serve_metrics\(state, addr, bearer_token, tls\)'
check "mgmt metrics TLS env" file_has "crates/pertisk-mgmt/src/config.rs" 'MGMT_METRICS_TLS_CA'

# --- TPM PCR attestation (sysfs lab path) ---
check "Attest RPC in proto" file_has "proto/pertisk/machine/v1alpha1/machine.proto" 'rpc Attest'
check "sysfs PCR reader" file_has "crates/pertisk-api/src/attest.rs" 'pcr-sha256'
check "pertiskctl attest" file_has "crates/pertiskctl/src/main.rs" 'Commands::Attest'
check "QEMU PERTISK_TPM" file_has "image/run-qemu-uefi.sh" 'PERTISK_TPM'

# --- TPM2 Quote (pure-Rust lab path) ---
check "Quote RPC in proto" file_has "proto/pertisk/machine/v1alpha1/machine.proto" 'rpc Quote'
check "pertisk-tpm Quote client" file_has "crates/pertisk-tpm/src/quote.rs" 'produce_quote'
check "pertiskctl quote" file_has "crates/pertiskctl/src/main.rs" 'Commands::Quote'
check "persistent AK handle" file_has "crates/pertisk-tpm/src/wire.rs" 'AK_PERSISTENT_HANDLE'
check "EK cert NV read" file_has "crates/pertisk-tpm/src/ek.rs" 'read_ek_certificate'
check "EK CA chain verify" file_has "crates/pertisk-tpm/src/ek.rs" 'verify_ek_chain'
check "mgmt Quote enroll" file_has "crates/pertisk-mgmt/src/node_attestation.rs" 'pub async fn enroll'
check "mgmt Quote verify" file_has "crates/pertisk-mgmt/src/node_attestation.rs" 'pub async fn verify'

# --- etcd snapshot / restore (lab) ---
check "EtcdSnapshot RPC in proto" file_has "proto/pertisk/machine/v1alpha1/machine.proto" 'rpc EtcdSnapshot'
check "EtcdRestore RPC in proto" file_has "proto/pertisk/machine/v1alpha1/machine.proto" 'rpc EtcdRestore'
check "pertiskctl etcd" file_has "crates/pertiskctl/src/main.rs" 'Commands::Etcd'

# --- CRI containers / sandbox labels (lab) ---
check "Containers RPC in proto" file_has "proto/pertisk/machine/v1alpha1/machine.proto" 'rpc Containers'
check "ContainerInfo pod_namespace" file_has "proto/pertisk/machine/v1alpha1/machine.proto" 'string pod_namespace'
check "ctr info label parse" file_has "crates/pertisk-api/src/containers.rs" 'parse_container_info_labels'
check "CRI log resolve" file_has "crates/pertisk-api/src/containers.rs" 'resolve_cri_log'
check "logs container: service" file_has "crates/pertisk-api/src/logs.rs" 'container:'
check "logs follow stream" file_has "proto/pertisk/machine/v1alpha1/machine.proto" 'stream LogsResponse'
check "logs follow flag" file_has "crates/pertisk-api/src/logs.rs" 'follow_logs'
check "pertiskctl containers" file_has "crates/pertiskctl/src/main.rs" 'Commands::Containers'
check "NetInspect RPC" file_has "proto/pertisk/machine/v1alpha1/machine.proto" 'rpc NetInspect'
check "DiskInspect RPC" file_has "proto/pertisk/machine/v1alpha1/machine.proto" 'rpc DiskInspect'
check "pertiskctl interfaces" file_has "crates/pertiskctl/src/main.rs" 'Commands::Interfaces'
check "pertiskctl disks" file_has "crates/pertiskctl/src/main.rs" 'Commands::Disks'

# --- Mount hardening ---
LINUX="crates/pertiskd/src/linux.rs"
check "proc nosuid/noexec/nodev" file_has "${LINUX}" 'MS_NOSUID \| MsFlags::MS_NOEXEC \| MsFlags::MS_NODEV'
check "tmpfs nosuid/nodev" file_has "${LINUX}" 'MS_NOSUID \| MsFlags::MS_NODEV'

# --- Docs present ---
check "HARDENING.md exists" test -f docs/HARDENING.md
check "COMPATIBILITY.md exists" test -f docs/COMPATIBILITY.md
check "SECURE_BOOT.md exists" test -f docs/SECURE_BOOT.md
check "enroll-ovmf-vars.sh exists" test -x scripts/enroll-ovmf-vars.sh
check "gen-secureboot-keys.sh exists" test -x scripts/gen-secureboot-keys.sh

# --- Production image: no BusyBox / udhcpc ---
DF="image/Dockerfile.initramfs"
check "IMAGE_PROFILE defaults to production" file_has "${DF}" 'ARG IMAGE_PROFILE=production'
check "production removes BusyBox staging" file_has "${DF}" 'rm -f ./usr/lib/pertisk/.busybox-debug'
check "production removes /bin/busybox" file_has "${DF}" 'rm -f ./usr/lib/pertisk/.busybox-debug ./bin/busybox'
check "util-linux mount shipped" file_has "${DF}" 'tools/bin/mount /tools/bin/umount'
check "iproute2 ip shipped" file_has "${DF}" 'tools/bin/ip ./sbin/ip'
check "no udhcpc in image" bash -c "! grep -qE 'usr/sbin/udhcpc|pertisk-udhcpc-hook' '${DF}'"
check "debug profile installs ash via /bin/busybox" file_has "${DF}" 'mv ./usr/lib/pertisk/.busybox-debug ./bin/busybox'
check "builtin DHCP only" file_has "crates/pertisk-net/src/link.rs" 'crate::dhcp::run_dhcp\(iface\)'
check "DHCP renew/rebind maintainer" file_has "crates/pertisk-net/src/dhcp.rs" 'ensure_maintainer'
check "DHCP lease persist under STATE" file_has "crates/pertisk-net/src/dhcp.rs" 'persist_lease'
check "DHCP INIT-REBOOT before DISCOVER" file_has "crates/pertisk-net/src/dhcp.rs" 'init_reboot'
check "shared ioctl DHCP lease apply" file_has "crates/pertisk-net/src/link.rs" 'apply_dhcp_v4_lease'
check "no udhcpc_hook module" bash -c "! test -f crates/pertisk-net/src/udhcpc_hook.rs"

# --- Unit tests for kubelet CIS fields ---
echo
echo "==> cargo test (kubelet config / metrics auth)"
cargo test -p pertisk-kubelet -p pertisk-api --lib --quiet

echo
echo "==> summary: ${pass} passed, ${fail} failed"
if [[ "${fail}" -ne 0 ]]; then
  exit 1
fi
echo "hardening gate OK"
