#!/usr/bin/env bash
# Start kubelet on NotReady Pertisk nodes (0.3.9 CRI race after VM power-on).
#
# Usage:
#   ./scripts/recover-not-ready-nodes.sh ~/.kube/ptkos/lab-ha-nutanix.yaml
#   PERTISKCTL=./target/debug/pertiskctl ./scripts/recover-not-ready-nodes.sh admin.conf
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KC="${1:-}"
if [[ -z "${KC}" || ! -f "${KC}" ]]; then
  echo "usage: $0 <kubeconfig>" >&2
  exit 1
fi

if [[ -n "${PERTISKCTL:-}" && -x "${PERTISKCTL}" ]]; then
  CTL="${PERTISKCTL}"
elif [[ -x "${ROOT}/target/debug/pertiskctl" ]]; then
  CTL="${ROOT}/target/debug/pertiskctl"
elif [[ -x "${ROOT}/out/bin/pertiskctl" ]]; then
  CTL="${ROOT}/out/bin/pertiskctl"
elif command -v pertiskctl >/dev/null 2>&1; then
  CTL="$(command -v pertiskctl)"
else
  echo "pertiskctl not found (set PERTISKCTL=…)" >&2
  exit 1
fi

TMPDIR="$(mktemp -d)"
trap 'rm -rf "${TMPDIR}"' EXIT
kubectl --kubeconfig "${KC}" get nodes -o json >"${TMPDIR}/nodes.json"

eval "$(python3 - "${TMPDIR}/nodes.json" <<'PY'
import json, shlex, sys
doc = json.load(open(sys.argv[1]))
not_ready = []
cp_ip = ""
for item in doc.get("items") or []:
    name = item.get("metadata", {}).get("name") or ""
    labels = item.get("metadata", {}).get("labels") or {}
    is_cp = "node-role.kubernetes.io/control-plane" in labels
    conds = item.get("status", {}).get("conditions") or []
    ready = next((c.get("status") for c in conds if c.get("type") == "Ready"), "") == "True"
    ip = ""
    for addr in item.get("status", {}).get("addresses") or []:
        a = addr.get("address") or ""
        if addr.get("type") == "InternalIP" and ":" not in a:
            ip = a
            break
    ver = item.get("status", {}).get("nodeInfo", {}).get("kubeletVersion") or ""
    if is_cp and ready and ip and not cp_ip:
        cp_ip = ip
    if not ready:
        not_ready.append((name, ip, ver))
print("CP_IP=" + shlex.quote(cp_ip))
print("NOT_READY_COUNT=" + str(len(not_ready)))
for i, (name, ip, ver) in enumerate(not_ready):
    print(f"NR_{i}_NAME=" + shlex.quote(name))
    print(f"NR_{i}_IP=" + shlex.quote(ip))
    print(f"NR_{i}_VER=" + shlex.quote(ver))
PY
)"

if [[ "${NOT_READY_COUNT}" -eq 0 ]]; then
  echo "no NotReady nodes"
  exit 0
fi
if [[ -z "${CP_IP}" ]]; then
  echo "no Ready control plane to fetch join config" >&2
  exit 1
fi

WORKER_YAML="${TMPDIR}/worker.yaml"
"${CTL}" -e "${CP_IP}:50000" get-join-config --worker-out "${WORKER_YAML}" >/dev/null

recovered=0
i=0
while [[ "${i}" -lt "${NOT_READY_COUNT}" ]]; do
  eval "name=\${NR_${i}_NAME} ip=\${NR_${i}_IP} ver=\${NR_${i}_VER}"
  if [[ -z "${ip}" ]]; then
    echo "skip ${name}: no InternalIP"
    i=$((i + 1))
    continue
  fi
  health="$("${CTL}" -e "${ip}:50000" health 2>/dev/null || true)"
  if [[ "${health}" != *"kubelet=absent"* ]]; then
    echo "skip ${name} (${ip}): ${health:-unreachable}"
    i=$((i + 1))
    continue
  fi
  yaml="${TMPDIR}/${name}.yaml"
  python3 - "${WORKER_YAML}" "${yaml}" "${name}" "${ver}" <<'PY'
import sys
src, dest, hostname, ver = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
lines = open(src).read().splitlines(True)
out = []
seen_host = False
for line in lines:
    stripped = line.lstrip()
    indent = line[: len(line) - len(stripped)]
    if not seen_host and stripped.startswith("hostname:"):
        out.append(f"{indent}hostname: {hostname}\n")
        seen_host = True
        continue
    if ver and stripped.startswith("kubernetesVersion:"):
        out.append(f"{indent}kubernetesVersion: {ver}\n")
        continue
    out.append(line)
open(dest, "w").writelines(out)
PY
  echo "apply ${name} @ ${ip}:50000 (kubelet=absent)"
  "${CTL}" -e "${ip}:50000" apply -f "${yaml}"
  recovered=$((recovered + 1))
  i=$((i + 1))
done

echo "applied ${recovered} node(s); wait ~15s then: kubectl --kubeconfig ${KC} get nodes"
