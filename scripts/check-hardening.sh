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
check "sysctl applied before runtime" file_has "crates/pertiskd/src/main.rs" 'apply_hardening_sysctls'

# --- Metrics auth surface ---
check "metrics bearer support" file_has "crates/pertisk-api/src/metrics.rs" 'bearer_authorized'
check "metrics token CLI/env" file_has "crates/pertiskd/src/main.rs" 'PERTISK_METRICS_TOKEN'
check "metrics token from STATE" file_has "crates/pertiskd/src/main.rs" 'secrets/metrics.token'

# --- Mount hardening ---
LINUX="crates/pertiskd/src/linux.rs"
check "proc nosuid/noexec/nodev" file_has "${LINUX}" 'MS_NOSUID \| MsFlags::MS_NOEXEC \| MsFlags::MS_NODEV'
check "tmpfs nosuid/nodev" file_has "${LINUX}" 'MS_NOSUID \| MsFlags::MS_NODEV'

# --- Docs present ---
check "HARDENING.md exists" test -f docs/HARDENING.md
check "COMPATIBILITY.md exists" test -f docs/COMPATIBILITY.md

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
