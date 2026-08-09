#!/usr/bin/env bash
# Join an existing Pertisk node (bare metal / already-running guest) into a cluster.
# No VM create — target must already expose Machine API on :50000.
#
# Usage:
#   ./scripts/adopt-node.sh \
#     --role worker --name lab-wk-4 --node-ip 10.1.1.50 \
#     --cp-ip 10.1.1.10 --cluster-out ./out/cluster --cluster-name lab
#
# Control plane:
#   ./scripts/adopt-node.sh --role controlplane --name lab-cp-2 --node-ip … \
#     --cp-ip … --controlplane-index 2 --cluster-out … --cluster-name …

set -euo pipefail

ROLE="worker"
NAME=""
NODE_IP=""
CP_IP=""
CLUSTER_OUT=""
CLUSTER_NAME=""
CP_INDEX=""
CTL="${PERTISKCTL:-./out/bin/pertiskctl}"

die() { echo "error: $*" >&2; exit 1; }
log() { echo "[adopt] $*"; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --role) ROLE="$2"; shift 2 ;;
    --name) NAME="$2"; shift 2 ;;
    --node-ip) NODE_IP="$2"; shift 2 ;;
    --cp-ip) CP_IP="$2"; shift 2 ;;
    --cluster-out) CLUSTER_OUT="$2"; shift 2 ;;
    --cluster-name) CLUSTER_NAME="$2"; shift 2 ;;
    --controlplane-index) CP_INDEX="$2"; shift 2 ;;
    --pertiskctl) CTL="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,20p' "$0"
      exit 0
      ;;
    *) die "unknown arg: $1" ;;
  esac
done

[[ "$ROLE" == "worker" || "$ROLE" == "controlplane" ]] || die "role must be worker|controlplane"
[[ -n "$NAME" ]] || die "--name required"
[[ -n "$NODE_IP" ]] || die "--node-ip required"
[[ -n "$CP_IP" ]] || die "--cp-ip required"
[[ -n "$CLUSTER_OUT" ]] || die "--cluster-out required"
[[ -n "$CLUSTER_NAME" ]] || die "--cluster-name required"
[[ -x "$CTL" || -f "$CTL" ]] || die "pertiskctl not found: $CTL"
mkdir -p "$CLUSTER_OUT"

wait_api() {
  local ip="$1" tries=0
  until "$CTL" -e "${ip}:50000" version >/dev/null 2>&1; do
    tries=$((tries + 1))
    (( tries < 60 )) || die "Machine API not reachable at ${ip}:50000"
    sleep 2
  done
}

set_hostname_yaml() {
  local src="$1" dst="$2" h="$3"
  awk -v h="$h" '
    BEGIN { innet=0; done=0 }
    /^machine:/ { innet=0 }
    /^  network:/ { innet=1 }
    innet && /^    hostname:/ && !done { print "    hostname: " h; done=1; next }
    { print }
    END {
      if (!done) {
        # Fallback: no hostname line — leave file as-is
      }
    }
  ' "$src" >"$dst"
}

log "waiting for CP API ${CP_IP}:50000"
wait_api "$CP_IP"
log "waiting for node API ${NODE_IP}:50000"
wait_api "$NODE_IP"

if [[ "$ROLE" == "worker" ]]; then
  [[ -f "$CLUSTER_OUT/worker.yaml" ]] || die "missing $CLUSTER_OUT/worker.yaml — create/bootstrap cluster first"
  log "refresh worker join CA from CP ${CP_IP}"
  "$CTL" -e "${CP_IP}:50000" join-config -f "$CLUSTER_OUT/worker.yaml"
  wyaml="${CLUSTER_OUT}/worker-adopt-${NAME}.yaml"
  if [[ "$NAME" =~ wk-([0-9]+)$ ]]; then
    wyaml="${CLUSTER_OUT}/worker-${BASH_REMATCH[1]}.yaml"
  fi
  set_hostname_yaml "$CLUSTER_OUT/worker.yaml" "$wyaml" "$NAME"
  log "apply join config → ${NAME} @ ${NODE_IP}"
  "$CTL" -e "${NODE_IP}:50000" apply -f "$wyaml"
else
  idx="${CP_INDEX:-}"
  if [[ -z "$idx" && "$NAME" =~ cp-([0-9]+)$ ]]; then
    idx="${BASH_REMATCH[1]}"
  fi
  [[ -n "$idx" ]] || die "controlplane requires --controlplane-index or name …-cp-N"
  cpyaml="${CLUSTER_OUT}/controlplane-${idx}.yaml"
  [[ "$idx" == "1" ]] && cpyaml="${CLUSTER_OUT}/controlplane.yaml"
  log "get-join-config for ${NAME} (index ${idx})"
  "$CTL" -e "${CP_IP}:50000" get-join-config \
    --controlplane --controlplane-index "$idx" --cluster-name "$CLUSTER_NAME" \
    -o "$cpyaml"
  set_hostname_yaml "$cpyaml" "${cpyaml}.tmp" "$NAME"
  mv "${cpyaml}.tmp" "$cpyaml"
  log "apply + join-controlplane ${NAME} @ ${NODE_IP}"
  "$CTL" -e "${NODE_IP}:50000" apply -f "$cpyaml"
  sleep 5
  wait_api "$NODE_IP"
  etcd_ep="https://${CP_IP}:2379"
  join_try=0
  until "$CTL" -e "${NODE_IP}:50000" join-controlplane --etcd-endpoints "$etcd_ep"; do
    join_try=$((join_try + 1))
    (( join_try < 5 )) || die "join-controlplane failed after ${join_try} attempts"
    log "join-controlplane retry ${join_try}/5..."
    sleep 10
    wait_api "$NODE_IP"
  done
fi

log "node ${NAME} adopted ip=${NODE_IP}"
echo "NODE_IP=${NODE_IP}"
