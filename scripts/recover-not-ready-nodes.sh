#!/usr/bin/env bash
# Recover Pertisk nodes after VM power-off/on when kubelet never came back,
# or when HA etcd has no leader so the API VIP is down.
#
# Usage:
#   ./scripts/recover-not-ready-nodes.sh ~/.kube/ptkos/lab-ha-h255.yaml
#   CLUSTER=lab-ha-h255 LAB_SUBNET=10.1.1.0/24 ./scripts/recover-not-ready-nodes.sh admin.conf
#   SKIP_ETCD_RECOVER=1 ./scripts/recover-not-ready-nodes.sh admin.conf
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

server="$(kubectl --kubeconfig "${KC}" config view --minify -o jsonpath='{.clusters[0].cluster.server}' 2>/dev/null || true)"
echo "kubeconfig server: ${server:-unknown}"

vip_host="$(python3 - "${server}" <<'PY'
import sys, urllib.parse
u = urllib.parse.urlparse(sys.argv[1] if sys.argv[1] else "")
print(u.hostname or "")
PY
)"

if [[ -z "${CLUSTER:-}" ]]; then
  CLUSTER="$(basename "${KC}")"
  CLUSTER="${CLUSTER%.yaml}"
  CLUSTER="${CLUSTER%.yml}"
fi
echo "cluster prefix: ${CLUSTER}-*"

if [[ -z "${LAB_SUBNET:-}" && "${vip_host}" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)\.[0-9]+$ ]]; then
  LAB_SUBNET="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.${BASH_REMATCH[3]}.0/24"
  echo "auto LAB_SUBNET=${LAB_SUBNET}"
fi
LAB_SUBNET="${LAB_SUBNET:-10.1.1.0/24}"

python3 - "${LAB_SUBNET}" "${TMPDIR}/scan_6443" "${TMPDIR}/scan_50000" <<'PY'
import socket, sys
from concurrent.futures import ThreadPoolExecutor, as_completed

cidr, path_6443, path_50000 = sys.argv[1], sys.argv[2], sys.argv[3]
base = cidr.split("/")[0].rsplit(".", 1)[0]
ports = (6443, 50000)

def probe(ip, port):
    s = socket.socket()
    s.settimeout(0.6)
    try:
        s.connect((ip, port))
        return ip, port, True
    except OSError:
        return ip, port, False
    finally:
        s.close()

found = {6443: [], 50000: []}
with ThreadPoolExecutor(max_workers=64) as ex:
    futs = [ex.submit(probe, f"{base}.{i}", p) for i in range(1, 255) for p in ports]
    for fut in as_completed(futs):
        ip, port, ok = fut.result()
        if ok:
            found[port].append(ip)

def key(ip):
    return tuple(int(x) for x in ip.split("."))

open(path_6443, "w").write("\n".join(sorted(set(found[6443]), key=key)) + "\n")
open(path_50000, "w").write("\n".join(sorted(set(found[50000]), key=key)) + "\n")
PY

has_port() {
  local ip="$1" file="$2" line
  while IFS= read -r line || [[ -n "${line}" ]]; do
    [[ "${line}" == "${ip}" ]] && return 0
  done <"${file}"
  return 1
}

prefix_ok() {
  local name="$1"
  [[ -n "${name}" && "${name}" == "${CLUSTER}-"* ]]
}

is_cp() {
  case "$1" in
    *-cp-*) return 0 ;;
    *) return 1 ;;
  esac
}

is_cp1() {
  case "$1" in
    *-cp-1) return 0 ;;
    *) return 1 ;;
  esac
}

KUBECTL=(kubectl --kubeconfig "${KC}" --request-timeout=5s)

inventory="${TMPDIR}/nodes.tsv"
: >"${inventory}"
while IFS= read -r ip || [[ -n "${ip}" ]]; do
  [[ -z "${ip}" ]] && continue
  health="$("${CTL}" -e "${ip}:50000" health 2>/dev/null || true)"
  ver="$("${CTL}" -e "${ip}:50000" version 2>/dev/null || true)"
  name="$(printf '%s\n' "${ver}" | sed -n 's/.*hostname=\([^ ]*\).*/\1/p' | head -1)"
  if ! prefix_ok "${name}"; then
    continue
  fi
  apiserver=down
  if is_cp "${name}" && has_port "${ip}" "${TMPDIR}/scan_6443"; then
    if "${KUBECTL[@]}" --server="https://${ip}:6443" get --raw=/readyz >/dev/null 2>&1; then
      apiserver=ready
    else
      apiserver=listen
    fi
  fi
  printf '%s\t%s\t%s\t%s\n' "${ip}" "${name}" "${apiserver}" "${health:-unreachable}" >>"${inventory}"
done <"${TMPDIR}/scan_50000"

if [[ ! -s "${inventory}" ]]; then
  echo "no :50000 hosts with hostname ${CLUSTER}-*" >&2
  exit 1
fi

echo "cluster nodes:"
while IFS=$'\t' read -r ip name apiserver health; do
  echo "  ${ip}  ${name}  apiserver=${apiserver}  ${health}"
done <"${inventory}"

API_OK=0
if "${KUBECTL[@]}" get --raw=/readyz >/dev/null 2>&1; then
  API_OK=1
else
  echo "VIP/API in kubeconfig is unreachable; probing this cluster's /readyz…"
  while IFS=$'\t' read -r ip name apiserver health; do
    [[ "${apiserver}" == "ready" ]] || continue
    echo "using apiserver https://${ip}:6443 (${name})"
    KUBECTL=(kubectl --kubeconfig "${KC}" --server="https://${ip}:6443" --request-timeout=8s)
    API_OK=1
    break
  done <"${inventory}"
fi

apply_absent_kubelets() {
  local recovered=0 ip name apiserver health yaml
  local worker_yaml="${TMPDIR}/worker.yaml"
  local cp_src=""
  while IFS=$'\t' read -r ip name apiserver health; do
    is_cp "${name}" || continue
    if "${CTL}" -e "${ip}:50000" get-join-config --worker-out "${worker_yaml}" >/dev/null 2>&1; then
      cp_src="${ip}"
      echo "join-config from ${name} (${ip})"
      break
    fi
  done <"${inventory}"
  while IFS=$'\t' read -r ip name apiserver health; do
    [[ "${health}" == *"kubelet=absent"* ]] || continue
    if [[ -z "${cp_src}" || ! -f "${worker_yaml}" ]]; then
      echo "  skip ${name}: no join-config from a cluster CP"
      continue
    fi
    yaml="${TMPDIR}/${name}.yaml"
    python3 - "${worker_yaml}" "${yaml}" "${name}" <<'PY'
import sys
src, dest, hostname = sys.argv[1], sys.argv[2], sys.argv[3]
lines = open(src).read().splitlines(True)
out, seen_host = [], False
for line in lines:
    stripped = line.lstrip()
    indent = line[: len(line) - len(stripped)]
    if not seen_host and stripped.startswith("hostname:"):
        out.append(f"{indent}hostname: {hostname}\n")
        seen_host = True
        continue
    out.append(line)
open(dest, "w").writelines(out)
PY
    echo "  apply ${name} @ ${ip}:50000 (kubelet=absent)"
    "${CTL}" -e "${ip}:50000" apply -f "${yaml}"
    recovered=$((recovered + 1))
  done <"${inventory}"
  echo "applied ${recovered} kubelet=absent node(s)"
}

wait_apiserver() {
  local ip="$1" n=0
  while [[ "${n}" -lt 24 ]]; do
    if "${KUBECTL[@]}" --server="https://${ip}:6443" get --raw=/readyz >/dev/null 2>&1; then
      echo "apiserver https://${ip}:6443 is ready"
      KUBECTL=(kubectl --kubeconfig "${KC}" --server="https://${ip}:6443" --request-timeout=8s)
      return 0
    fi
    n=$((n + 1))
    sleep 5
  done
  return 1
}

recover_etcd_cp1() {
  local ip name apiserver health cp1_ip="" extra=""
  local ready=0 listen=0 down=0
  while IFS=$'\t' read -r ip name apiserver health; do
    is_cp "${name}" || continue
    case "${apiserver}" in
      ready)
        ready=$((ready + 1))
        ;;
      listen)
        listen=$((listen + 1))
        echo "  ${name} (${ip}): :6443 listens but /readyz fails (etcd has no leader)"
        ;;
      *)
        down=$((down + 1))
        echo "  ${name} (${ip}): kubelet up but :6443 closed"
        ;;
    esac
    if is_cp1 "${name}"; then
      cp1_ip="${ip}"
    fi
    if ! is_cp1 "${name}"; then
      extra="${extra} ${name}=${ip}"
    fi
  done <"${inventory}"
  if [[ "${ready}" -gt 0 ]]; then
    echo "a control-plane /readyz is already ok; not running etcd recover"
    return 0
  fi
  if [[ -z "${cp1_ip}" ]]; then
    echo "no ${CLUSTER}-cp-1 on :50000; cannot etcd recover" >&2
    return 1
  fi
  if [[ "${SKIP_ETCD_RECOVER:-}" == "1" ]]; then
    echo "SKIP_ETCD_RECOVER=1: would run:"
    echo "  ${CTL} -e ${cp1_ip}:50000 etcd recover --force-new-cluster --force"
    return 0
  fi
  echo "no CP /readyz — recovering etcd on ${CLUSTER}-cp-1 (${cp1_ip}) (listen=${listen} down=${down})"
  echo "(promotes cp-1 to a single-member cluster; extra CPs must be reset + re-joined)"
  if ! "${CTL}" -e "${cp1_ip}:50000" etcd recover --force-new-cluster --force; then
    echo "etcd recover failed (guest may be too old for the RPC). Power on CP1 first after peers have IPv4, or ship a new OS bundle." >&2
    return 1
  fi
  echo "waiting for apiserver on ${cp1_ip}:6443…"
  if wait_apiserver "${cp1_ip}"; then
    echo "kubectl --kubeconfig ${KC} --server=https://${cp1_ip}:6443 get nodes"
    "${KUBECTL[@]}" get nodes -o wide || true
  else
    echo "apiserver did not become ready on ${cp1_ip}:6443" >&2
    return 1
  fi
  if [[ -n "${extra}" ]]; then
    echo "extra CPs still on the old etcd membership. After cp-1 is stable, reset + re-join each:"
    for pair in ${extra}; do
      echo "  ${CTL} -e ${pair##*=}:50000 reset --force"
    done
    echo "  then get-join-config --controlplane from ${cp1_ip} and join-controlplane --etcd-endpoints https://${cp1_ip}:2379"
  fi
}

if [[ "${API_OK}" -eq 0 ]]; then
  echo "Kubernetes API still down (VIP ${vip_host:-?} timed out)."
  apply_absent_kubelets
  recover_etcd_cp1
  exit 0
fi

"${KUBECTL[@]}" get nodes -o json >"${TMPDIR}/nodes.json"

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
  echo "no Ready control plane in the API; falling back to Machine API" >&2
  apply_absent_kubelets
  recover_etcd_cp1
  exit 0
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
