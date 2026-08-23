#!/usr/bin/env bash
# Upload a Pertisk cloud qcow2 to Nutanix Prism Element (AHV) and create a UEFI VM.
#
# Auth (Prism Element Basic auth on :9440):
#   export NUTANIX_URL="https://10.1.1.50:9440"
#   export NUTANIX_USER="admin"
#   export NUTANIX_PASSWORD="…"
#   export NUTANIX_STORAGE="SelfServiceContainer"
#   export NUTANIX_NETWORK="vlan.100"
#   export NUTANIX_INSECURE=1
#
#   ./scripts/nutanix-upload-vm.sh --vmid 9100 --name lab-9100 \
#     --disk out/pertisk-cloud-amd64.qcow2
set -euo pipefail

VMID=""
NAME=""
DISK=""
MEMORY="${NUTANIX_MEMORY:-4096}"
CORES="${NUTANIX_CORES:-2}"
DISK_GB="${NUTANIX_DISK_GB:-}"
NETWORK="${NUTANIX_NETWORK:-}"
STORAGE="${NUTANIX_STORAGE:-}"
START=1
IMAGE_NAME="${NUTANIX_IMAGE_NAME:-}"
REPAIR_NAME=""

usage() {
  cat <<'EOF'
Usage:
  ./scripts/nutanix-upload-vm.sh --vmid ID --disk PATH [options]

Options:
  --vmid N          numeric id used in default name PREFIX-N (required)
  --disk PATH       qcow2 path (required)
  --name NAME       VM name (default: ${NAME_PREFIX:-pertisk}-$VMID)
  --memory MB       RAM (default 4096; env NUTANIX_MEMORY)
  --cores N         vCPUs (default 2; env NUTANIX_CORES)
  --disk-gb N       grow primary disk to N GiB after clone (env NUTANIX_DISK_GB)
  --network NAME    AHV network name (default $NUTANIX_NETWORK)
  --storage NAME    storage container (default $NUTANIX_STORAGE)
  --no-start        do not power on after create
  --repair-name NAME  existing VM: power off, attach IPAM netcfg, power on

Env:
  NUTANIX_FORCE_IMPORT=1   re-import even when the qcow2 fingerprint already exists
  NUTANIX_IMAGE_NAME=NAME  pin a Prism image (skips fingerprint; can boot a stale guest)
EOF
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --vmid) VMID="$2"; shift 2 ;;
    --name) NAME="$2"; shift 2 ;;
    --disk) DISK="$2"; shift 2 ;;
    --memory) MEMORY="$2"; shift 2 ;;
    --cores) CORES="$2"; shift 2 ;;
    --disk-gb) DISK_GB="$2"; shift 2 ;;
    --network) NETWORK="$2"; shift 2 ;;
    --storage) STORAGE="$2"; shift 2 ;;
    --no-start) START=0; shift ;;
    --repair-name) REPAIR_NAME="$2"; shift 2 ;;
    -h | --help) usage ;;
    *) echo "unknown arg: $1" >&2; usage ;;
  esac
done

if [[ -n "$REPAIR_NAME" ]]; then
  NAME="$REPAIR_NAME"
  VMID="${VMID:-0}"
  DISK="${DISK:-/dev/null}"
else
  [[ -n "${VMID}" && -n "${DISK}" ]] || usage
  [[ -f "${DISK}" ]] || {
    echo "disk not found: ${DISK}" >&2
    exit 1
  }
fi

: "${NUTANIX_URL:?set NUTANIX_URL}"
: "${NUTANIX_USER:?set NUTANIX_USER}"
: "${NUTANIX_PASSWORD:?set NUTANIX_PASSWORD}"
: "${STORAGE:?set NUTANIX_STORAGE or --storage}"
: "${NETWORK:?set NUTANIX_NETWORK or --network}"

command -v python3 >/dev/null || {
  echo "python3 required" >&2
  exit 1
}
command -v jq >/dev/null || {
  echo "jq required" >&2
  exit 1
}

NAME="${NAME:-${NAME_PREFIX:-pertisk}-${VMID}}"
BASE="${NUTANIX_URL%/}"
API="${BASE}/api/nutanix/v2.0"

CURL=(curl -sS)
[[ "${NUTANIX_INSECURE:-0}" == "1" ]] && CURL+=(-k)
CURL+=(-u "${NUTANIX_USER}:${NUTANIX_PASSWORD}" -H 'Accept: application/json')

api_get() {
  "${CURL[@]}" "${API}/$1"
}

api_json() {
  local method="$1" path="$2" data="${3:-}"
  if [[ -n "$data" ]]; then
    "${CURL[@]}" -X "$method" -H 'Content-Type: application/json' -d "$data" "${API}/${path}"
  else
    "${CURL[@]}" -X "$method" "${API}/${path}"
  fi
}

# PE POST create returns {"task_uuid":"..."} — poll until Succeeded and extract entity uuid.
wait_task() {
  local task_uuid="$1" kind_hint="${2:-}"
  local task status entity err i
  echo "==> wait task ${task_uuid}${kind_hint:+ ($kind_hint)}" >&2
  for i in $(seq 1 180); do
    task="$(api_get "tasks/${task_uuid}" 2>/dev/null || true)"
    if [[ -z "$task" || "$task" == "null" ]]; then
      sleep 2
      continue
    fi
    status="$(echo "$task" | jq -r '
      (.progress_status // .status // .percentage_complete // "")
      | tostring | ascii_upcase
    ')"
    err="$(echo "$task" | jq -r '
      .meta_response.error_detail
      // .error_detail
      // .message
      // empty
    ' 2>/dev/null || true)"
    case "$status" in
      SUCCEEDED|SUCCESS|COMPLETE|COMPLETED|100)
        entity="$(echo "$task" | jq -r --arg hint "$kind_hint" '
          (
            (.entity_reference_list // [])
            + (.entity_list // [])
            + (if .entity_id then [{entity_id:.entity_id, uuid:.entity_id, entity_type:(.entity_type // "")}] else [] end)
          )
          | map(
              (.uuid // .entity_id // .id // empty) as $u
              | select($u != null and $u != "")
              | {uuid:$u, kind:((.kind // .entity_type // .entity_name // "")|tostring|ascii_downcase)}
            )
          | if $hint != "" then
              (map(select(.kind | contains($hint))) + .)
            else . end
          | .[0].uuid // empty
        ')"
        if [[ -n "$entity" ]]; then
          echo "$entity"
          return 0
        fi
        # Task succeeded but no entity uuid in payload — caller may look up by name.
        echo ""
        return 0
        ;;
      FAILED|ABORTED|ERROR|CANCELLED|CANCELED)
        echo "task ${task_uuid} failed: ${err:-$status}" >&2
        echo "$task" | head -c 800 >&2
        echo >&2
        return 1
        ;;
    esac
    sleep 2
  done
  echo "task ${task_uuid} timed out" >&2
  return 1
}

# Resolve uuid from create response: direct uuid, or poll task_uuid.
resolve_create_uuid() {
  local resp="$1" kind_hint="${2:-}" lookup_name="${3:-}"
  local uuid task_uuid
  uuid="$(echo "$resp" | jq -r '.metadata.uuid // .uuid // .entity_id // empty')"
  if [[ -n "$uuid" ]]; then
    echo "$uuid"
    return 0
  fi
  task_uuid="$(echo "$resp" | jq -r '.task_uuid // .task_uuid_list[0] // empty')"
  if [[ -z "$task_uuid" ]]; then
    echo "create response missing uuid/task_uuid: $resp" >&2
    return 1
  fi
  uuid="$(wait_task "$task_uuid" "$kind_hint")" || return 1
  if [[ -n "$uuid" ]]; then
    echo "$uuid"
    return 0
  fi
  # Fallback: look up by name after task success.
  if [[ -n "$lookup_name" ]]; then
    sleep 2
    case "$kind_hint" in
      image)
        uuid="$(api_get images | jq -r --arg n "$lookup_name" '
          (.entities // .)
          | if type=="array" then . else [.] end
          | map(select(.name==$n))
          | .[0].uuid // empty
        ')"
        ;;
      vm)
        uuid="$(api_get vms | jq -r --arg n "$lookup_name" '
          (.entities // .)
          | if type=="array" then . else [.] end
          | map(select(.name==$n))
          | .[0].uuid // empty
        ')"
        ;;
    esac
    if [[ -n "$uuid" ]]; then
      echo "$uuid"
      return 0
    fi
  fi
  echo "task succeeded but could not resolve ${kind_hint:-entity} uuid" >&2
  return 1
}

find_image_uuid() {
  local want="$1"
  api_get images 2>/dev/null | jq -r --arg n "$want" '
    (.entities // .)
    | if type=="array" then . else [.] end
    | map(select(.name==$n))
    | .[0].uuid // empty
  ' || true
}

delete_image() {
  local uuid="$1"
  [[ -n "$uuid" ]] || return 0
  echo "==> deleting image ${uuid}" >&2
  local del
  del="$(api_json DELETE "images/${uuid}" || true)"
  if echo "${del:-}" | jq -e '.task_uuid' >/dev/null 2>&1; then
    wait_task "$(echo "$del" | jq -r '.task_uuid')" "image" >/dev/null || true
  else
    sleep 2
  fi
}

# Content id for the qcow2 on mgmt. Prism image names used to be
# pertisk-cloud-${VMID}-$(basename) with no hash, so create-cluster reused an
# old ACTIVE DISK_IMAGE and kubelet kept OS-IMAGE "pertisk-kos 0.1.0".
qcow2_fingerprint() {
  local f="$1" sum
  if command -v sha256sum >/dev/null 2>&1; then
    sum="$(sha256sum "$f" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    sum="$(shasum -a 256 "$f" | awk '{print $1}')"
  else
    sum="$(python3 -c 'import hashlib,sys
h=hashlib.sha256()
with open(sys.argv[1],"rb") as fh:
    for chunk in iter(lambda: fh.read(1024*1024), b""):
        h.update(chunk)
print(h.hexdigest())' "$f")"
  fi
  printf '%s\n' "${sum:0:12}"
}

# Drop leftover per-VMID images from older scripts (unhashed names).
delete_legacy_vmid_images() {
  local vmid="$1"
  local prefix="pertisk-cloud-${vmid}-"
  local uuid name
  while IFS=$'\t' read -r uuid name; do
    [[ -n "$uuid" ]] || continue
    echo "==> removing stale Prism image ${name} (${uuid})" >&2
    delete_image "$uuid"
  done < <(api_get images 2>/dev/null | jq -r --arg p "$prefix" '
    (.entities // .)
    | if type=="array" then . else [.] end
    | .[]
    | select((.name // "") | startswith($p))
    | "\(.uuid)\t\(.name // "")"
  ')
}

# Address Prism can reach to pull the qcow2 over HTTP.
detect_http_addr() {
  if [[ -n "${NUTANIX_HTTP_ADDR:-}" ]]; then
    echo "${NUTANIX_HTTP_ADDR}"
    return 0
  fi
  local pe_host pe_octets ip
  pe_host="$(echo "${NUTANIX_URL}" | sed -E 's|https?://([^/:]+).*|\1|')"
  if [[ "$pe_host" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)\.[0-9]+$ ]]; then
    pe_octets="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.${BASH_REMATCH[3]}."
    while read -r ip; do
      [[ "$ip" == ${pe_octets}* ]] && { echo "$ip"; return 0; }
    done < <(hostname -I 2>/dev/null | tr ' ' '\n'; ip -4 -o addr show scope global 2>/dev/null | awk '{print $4}' | cut -d/ -f1)
  fi
  # Fallback: first global IPv4
  ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
  if [[ -n "$ip" ]]; then
    echo "$ip"
    return 0
  fi
  echo "cannot detect NUTANIX_HTTP_ADDR (set it to an IP Prism can reach on this host)" >&2
  return 1
}

# Serve DISK over HTTP so Prism Element can import via image_import_spec.url
# (PE often has no local binary upload endpoint — PUT …/upload returns 404).
# Default port 18765 (override with NUTANIX_HTTP_PORT). Ephemeral ports are usually
# blocked by firewalld and show up as "No route to host" from Prism.
HTTP_PID=""
HTTP_DIR=""
FW_OPENED=""
stop_http() {
  if [[ -n "${HTTP_PID:-}" ]]; then
    kill "$HTTP_PID" 2>/dev/null || true
    wait "$HTTP_PID" 2>/dev/null || true
    HTTP_PID=""
  fi
  if [[ -n "${HTTP_DIR:-}" && -d "${HTTP_DIR}" ]]; then
    rm -rf "$HTTP_DIR"
    HTTP_DIR=""
  fi
  case "${FW_OPENED:-}" in
    firewalld:*)
      firewall-cmd --quiet --remove-port="${FW_OPENED#firewalld:}/tcp" 2>/dev/null || true
      ;;
    iptables:*)
      iptables -D INPUT -p tcp --dport "${FW_OPENED#iptables:}" -j ACCEPT 2>/dev/null || true
      ;;
  esac
  FW_OPENED=""
}
trap 'stop_http' EXIT

open_fw_port() {
  local port="$1"
  if command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
    if firewall-cmd --quiet --query-port="${port}/tcp" 2>/dev/null; then
      echo "==> firewalld already allows ${port}/tcp" >&2
      return 0
    fi
    if firewall-cmd --quiet --add-port="${port}/tcp" 2>/dev/null; then
      FW_OPENED="firewalld:${port}"
      echo "==> opened firewalld ${port}/tcp for Prism image pull (temporary)" >&2
      return 0
    fi
    echo "warn: could not open firewalld ${port}/tcp (need root?). Prism may get 'No route to host'." >&2
  elif command -v iptables >/dev/null 2>&1; then
    if iptables -C INPUT -p tcp --dport "$port" -j ACCEPT 2>/dev/null; then
      return 0
    fi
    if iptables -I INPUT -p tcp --dport "$port" -j ACCEPT 2>/dev/null; then
      FW_OPENED="iptables:${port}"
      echo "==> opened iptables INPUT tcp/${port} for Prism image pull (temporary)" >&2
      return 0
    fi
    echo "warn: could not open iptables tcp/${port} (need root?). Prism may get 'No route to host'." >&2
  fi
}

import_image_via_http() {
  local addr port url create_img
  addr="$(detect_http_addr)"
  # Stable default — ephemeral ports are almost always firewalled.
  port="${NUTANIX_HTTP_PORT:-18765}"
  HTTP_DIR="$(mktemp -d /var/tmp/pertisk-nutanix-http.XXXXXX 2>/dev/null || mktemp -d)"
  ln -sf "$(cd "$(dirname "$DISK")" && pwd)/$(basename "$DISK")" "${HTTP_DIR}/disk.qcow2"

  open_fw_port "$port"

  # Fail fast if the port is already taken.
  if ss -ltn "( sport = :${port} )" 2>/dev/null | grep -q ":${port}"; then
    echo "port ${port} already in use — set NUTANIX_HTTP_PORT to a free port" >&2
    return 1
  fi

  python3 -m http.server "$port" --bind 0.0.0.0 --directory "$HTTP_DIR" >/dev/null 2>&1 &
  HTTP_PID=$!
  sleep 0.4
  if ! kill -0 "$HTTP_PID" 2>/dev/null; then
    echo "failed to start HTTP server on :${port}" >&2
    return 1
  fi

  # Local sanity check (does not prove Prism can reach us).
  if ! curl -fsSI --max-time 3 "http://127.0.0.1:${port}/disk.qcow2" >/dev/null; then
    echo "local HTTP server not responding on :${port}" >&2
    return 1
  fi

  url="http://${addr}:${port}/disk.qcow2"
  echo "==> Prism image import from ${url}" >&2
  echo "    (CVMs must reach ${addr}:${port}; firewalld REJECT shows as 'No route to host')" >&2

  create_img="$(api_json POST images "$(jq -n \
    --arg name "$IMAGE_NAME" \
    --arg cuuid "$CONTAINER_UUID" \
    --arg url "$url" \
    '{
      name: $name,
      image_type: "DISK_IMAGE",
      image_import_spec: {
        storage_container_uuid: $cuuid,
        url: $url
      }
    }')")"
  if ! IMAGE_UUID="$(resolve_create_uuid "$create_img" image "$IMAGE_NAME")"; then
    echo "image import create failed: $create_img" >&2
    echo "HINT: on this host run:  sudo firewall-cmd --add-port=${port}/tcp" >&2
    echo "      (or permanent: --permanent && firewall-cmd --reload)" >&2
    echo "      Also whitelist ${addr} in Prism Settings → HTTP Proxy if a proxy is set." >&2
    echo "      Override bind IP with NUTANIX_HTTP_ADDR if needed." >&2
    return 1
  fi
  echo "==> image uuid=${IMAGE_UUID} (import in progress)" >&2

  local img state vmdisk i
  for i in $(seq 1 360); do
    img="$(api_get "images/${IMAGE_UUID}")"
    state="$(echo "$img" | jq -r '.image_state // .status // empty' | tr 'a-z' 'A-Z')"
    vmdisk="$(echo "$img" | jq -r '.vmdisk_uuid // .vm_disk_id // empty')"
    if [[ "$state" == "ACTIVE" || "$state" == "COMPLETE" ]] && [[ -n "$vmdisk" ]]; then
      VMDISK_UUID="$vmdisk"
      stop_http
      return 0
    fi
    if [[ "$state" == "ERROR" ]]; then
      echo "image import ERROR: $(echo "$img" | head -c 600)" >&2
      echo "HINT: sudo firewall-cmd --add-port=${port}/tcp   # Prism pulls from ${addr}:${port}" >&2
      stop_http
      return 1
    fi
    sleep 2
  done
  echo "image import timed out (state=${state:-?})" >&2
  stop_http
  return 1
}

echo "==> resolve storage container '${STORAGE}' and network '${NETWORK}'"
CONTAINERS="$(api_get storage_containers)"
CONTAINER_UUID="$(CONTAINERS_JSON="$CONTAINERS" python3 -c '
import json,os,sys
want=sys.argv[1]
data=json.loads(os.environ["CONTAINERS_JSON"])
ents=data.get("entities") or (data if isinstance(data,list) else [data])
for e in ents:
    if e.get("name")==want:
        print(e.get("storage_container_uuid") or e.get("uuid") or "")
        raise SystemExit
raise SystemExit("storage container %r not found" % want)
' "$STORAGE")"
[[ -n "$CONTAINER_UUID" ]] || { echo "storage container '${STORAGE}' not found" >&2; exit 1; }

NETWORKS="$(api_get networks)"
NETWORK_INFO="$(NETWORKS_JSON="$NETWORKS" python3 -c '
import json,os,sys
want=sys.argv[1]

def ents(data):
    e=data.get("entities") if isinstance(data,dict) else None
    if e is None:
        return data if isinstance(data,list) else [data]
    return e

def pick_gw(obj):
    if not isinstance(obj, dict):
        return ""
    ip=obj.get("ip_config") or obj.get("ipConfig") or {}
    if not isinstance(ip, dict):
        ip={}
    for k in ("default_gateway","default_gateway_ip","gateway","defaultGateway"):
        v=ip.get(k) or obj.get(k)
        if isinstance(v,str) and "." in v:
            return v
    return ""

def pick_prefix(obj):
    if not isinstance(obj, dict):
        return ""
    ip=obj.get("ip_config") or obj.get("ipConfig") or {}
    if isinstance(ip, dict):
        p=ip.get("prefix_length") or ip.get("prefixLength")
        if isinstance(p,int) and 0 < p <= 32:
            return str(p)
    return ""

data=json.loads(os.environ["NETWORKS_JSON"])
for e in ents(data):
    if e.get("name")==want:
        print((e.get("uuid") or "")+"\t"+(pick_gw(e) or "")+"\t"+(pick_prefix(e) or ""))
        raise SystemExit
raise SystemExit("network %r not found" % want)
' "$NETWORK")"
NETWORK_UUID="${NETWORK_INFO%%$'\t'*}"
NETWORK_GATEWAY="$(printf '%s\n' "$NETWORK_INFO" | awk -F'\t' '{print $2}')"
NETWORK_PREFIX="$(printf '%s\n' "$NETWORK_INFO" | awk -F'\t' '{print $3}')"
[[ -n "$NETWORK_UUID" ]] || { echo "network '${NETWORK}' not found" >&2; exit 1; }
# List payload is often thin — GET the network for ip_config.default_gateway.
if [[ -z "${NETWORK_GATEWAY}" || -z "${NETWORK_PREFIX}" ]]; then
  NET_DET="$(api_get "networks/${NETWORK_UUID}" 2>/dev/null || true)"
  if [[ -n "${NET_DET:-}" ]]; then
    extra="$(echo "$NET_DET" | python3 -c '
import json,sys
e=json.load(sys.stdin)
ip=e.get("ip_config") or e.get("ipConfig") or {}
if not isinstance(ip, dict):
    ip={}
gw=""
for k in ("default_gateway","default_gateway_ip","gateway","defaultGateway"):
    v=ip.get(k) or e.get(k)
    if isinstance(v,str) and "." in v:
        gw=v
        break
pref=""
p=ip.get("prefix_length") or ip.get("prefixLength")
if isinstance(p,int) and 0 < p <= 32:
    pref=str(p)
print(gw+"\t"+pref)
' 2>/dev/null || true)"
    [[ -z "${NETWORK_GATEWAY}" ]] && NETWORK_GATEWAY="$(printf '%s\n' "$extra" | awk -F'\t' '{print $1}')"
    [[ -z "${NETWORK_PREFIX}" ]] && NETWORK_PREFIX="$(printf '%s\n' "$extra" | awk -F'\t' '{print $2}')"
  fi
fi
if [[ -n "${NETWORK_GATEWAY}" ]]; then
  echo "==> AHV network '${NETWORK}' uuid=${NETWORK_UUID} gateway=${NETWORK_GATEWAY} prefix=${NETWORK_PREFIX:-?}"
  echo "warn: '${NETWORK}' is a *managed* IPAM subnet. AHV reserves Prism NIC IPs and typically" >&2
  echo "      does NOT DHCP the guest or flood DISCOVER onto vs0 (mgmt :67 stays silent; dashboard has no ipv4)." >&2
  echo "      Use an unmanaged network on the same virtual switch (this cluster: vlan.0 on vs0 VLAN 0)" >&2
  echo "      or Prism → Network → ${NETWORK} → Edit → enable DHCP for the IPAM pool." >&2
else
  echo "==> AHV network '${NETWORK}' uuid=${NETWORK_UUID} (unmanaged / no IPAM — guest DHCP from LAN)"
fi

if [[ -z "$REPAIR_NAME" ]]; then
# Delete existing VM with same name (recreate).
EXISTING="$(api_get vms)"
EXIST_UUID="$(EXISTING_JSON="$EXISTING" python3 -c '
import json,os,sys
want=sys.argv[1]
data=json.loads(os.environ["EXISTING_JSON"])
ents=data.get("entities") or (data if isinstance(data,list) else [data])
for e in ents:
    if e.get("name")==want:
        print(e.get("uuid") or "")
        raise SystemExit
' "$NAME" || true)"
if [[ -n "${EXIST_UUID:-}" ]]; then
  echo "==> deleting existing VM ${NAME} (${EXIST_UUID})"
  DEL="$(api_json POST "vms/${EXIST_UUID}/set_power_state" '{"transition":"OFF"}' || true)"
  sleep 3
  DEL="$(api_json DELETE "vms/${EXIST_UUID}?delete_snapshots=true" || true)"
  if echo "${DEL:-}" | jq -e '.task_uuid' >/dev/null 2>&1; then
    wait_task "$(echo "$DEL" | jq -r '.task_uuid')" "vm" >/dev/null || true
  else
    sleep 2
  fi
fi

DISK_BYTES="$(python3 -c 'import os,sys; print(os.path.getsize(sys.argv[1]))' "$DISK")"
if [[ -n "$DISK_GB" ]]; then
  WANT_BYTES=$((DISK_GB * 1024 * 1024 * 1024))
else
  if command -v qemu-img >/dev/null 2>&1; then
    WANT_BYTES="$(qemu-img info -f qcow2 --output=json "$DISK" | jq -r '.["virtual-size"]')"
  else
    WANT_BYTES="$DISK_BYTES"
  fi
fi

if [[ -n "$IMAGE_NAME" ]]; then
  echo "warn: NUTANIX_IMAGE_NAME=${IMAGE_NAME} — Prism will reuse this image even if ${DISK} changed" >&2
else
  echo "==> fingerprinting ${DISK}" >&2
  DISK_HASH="$(qcow2_fingerprint "$DISK")"
  IMAGE_NAME="pertisk-$(basename "$DISK" .qcow2)-${DISK_HASH}"
fi
echo "==> create/import image ${IMAGE_NAME} (${DISK_BYTES} bytes file, src=${DISK})"
delete_legacy_vmid_images "$VMID"
if [[ "${NUTANIX_FORCE_IMPORT:-0}" == "1" ]]; then
  FORCE_UUID="$(find_image_uuid "$IMAGE_NAME" || true)"
  if [[ -n "${FORCE_UUID:-}" ]]; then
    echo "==> NUTANIX_FORCE_IMPORT=1 — re-import ${IMAGE_NAME}" >&2
    delete_image "$FORCE_UUID"
  fi
fi

IMAGE_UUID="$(find_image_uuid "$IMAGE_NAME")"
VMDISK_UUID=""
if [[ -n "${IMAGE_UUID:-}" ]]; then
  IMG="$(api_get "images/${IMAGE_UUID}")"
  IMG_STATE="$(echo "$IMG" | jq -r '.image_state // .status // empty' | tr 'a-z' 'A-Z')"
  VMDISK_UUID="$(echo "$IMG" | jq -r '.vmdisk_uuid // .vm_disk_id // empty')"
  if [[ "$IMG_STATE" == "ACTIVE" || "$IMG_STATE" == "COMPLETE" ]] && [[ -n "$VMDISK_UUID" ]]; then
    echo "==> reusing ACTIVE image ${IMAGE_UUID} (qcow2 fingerprint matches ${DISK})"
  else
    echo "==> existing image ${IMAGE_UUID} state=${IMG_STATE:-?} — delete and re-import"
    delete_image "$IMAGE_UUID"
    IMAGE_UUID=""
    VMDISK_UUID=""
  fi
fi

if [[ -z "${IMAGE_UUID:-}" || -z "${VMDISK_UUID:-}" ]]; then
  import_image_via_http || {
    echo "HINT: if Prism has an HTTP proxy, whitelist this host IP (Settings → HTTP Proxy)." >&2
    echo "      Or set NUTANIX_HTTP_ADDR to an IP on the AHV/mgmt L2 network." >&2
    exit 1
  }
fi

[[ -n "${VMDISK_UUID:-}" ]] || {
  echo "image ${IMAGE_UUID} has no vmdisk_uuid" >&2
  exit 1
}

echo "==> create UEFI VM ${NAME} (mem=${MEMORY} cores=${CORES} disk>=${WANT_BYTES})"
# Deterministic MAC (like Proxmox) — PE GET /vms/{uuid} often omits NICs unless
# include_vm_nic_config=true, and MAC may be empty until after power-on without this.
mac_for_vmid() {
  local id="$1"
  local salt_src="${NUTANIX_MAC_SALT:-${NUTANIX_URL:-nutanix}}"
  local h
  h="$(printf '%s|%s' "$salt_src" "$id" | sha256sum | awk '{print $1}')"
  # Locally administered unicast: x2/x6/xA/xE in first octet.
  printf '52:54:%s:%s:%s:%s\n' "${h:0:2}" "${h:2:2}" "${h:4:2}" "${h:6:2}"
}
NET0_MAC="$(mac_for_vmid "${VMID}")"
echo "==> nic mac=${NET0_MAC}"

# AHV: SCSI (virtio-scsi) often hangs guest disk I/O after EFI stub; PCI virtio-blk
# (/dev/vda) is the reliable bus for Linux cloud images. Override: NUTANIX_DISK_BUS=scsi
DISK_BUS="${NUTANIX_DISK_BUS:-pci}"
echo "==> disk bus=${DISK_BUS}"

# Do not set nic "model" — PE rejects "VIRTIO" (InvalidArgument). Default AHV NIC is virtio.
# vm_serial_ports on POST is silently ignored by PE — attach after create (see ensure_serial).
VM_BODY="$(jq -n \
  --arg name "$NAME" \
  --argjson mem "$MEMORY" \
  --argjson cores "$CORES" \
  --arg net "$NETWORK_UUID" \
  --arg disk "$VMDISK_UUID" \
  --arg mac "$NET0_MAC" \
  --arg bus "$DISK_BUS" \
  --argjson size "$WANT_BYTES" \
  '{
    name: $name,
    memory_mb: $mem,
    num_vcpus: $cores,
    num_cores_per_vcpu: 1,
    description: "pertisk cloud guest",
    boot: {
      uefi_boot: true,
      secure_boot: false,
      disk_address: { device_bus: $bus, device_index: 0 }
    },
    vm_nics: [ {
      network_uuid: $net,
      is_connected: true,
      mac_address: $mac
    } ],
    vm_disks: [ {
      is_cdrom: false,
      disk_address: { device_bus: $bus, device_index: 0 },
      vm_disk_clone: {
        disk_address: { vmdisk_uuid: $disk },
        minimum_size: $size
      }
    } ]
  }')"
CREATE_VM="$(api_json POST vms "$VM_BODY")"
VM_UUID="$(resolve_create_uuid "$CREATE_VM" vm "$NAME")" || {
  echo "warn: VM create with fixed MAC / bus=${DISK_BUS} rejected; retrying scsi + no mac" >&2
  echo "      response: $CREATE_VM" >&2
  VM_BODY="$(jq -n \
    --arg name "$NAME" \
    --argjson mem "$MEMORY" \
    --argjson cores "$CORES" \
    --arg net "$NETWORK_UUID" \
    --arg disk "$VMDISK_UUID" \
    --argjson size "$WANT_BYTES" \
    '{
      name: $name,
      memory_mb: $mem,
      num_vcpus: $cores,
      num_cores_per_vcpu: 1,
      boot: {
        uefi_boot: true,
        secure_boot: false,
        disk_address: { device_bus: "scsi", device_index: 0 }
      },
      vm_nics: [ { network_uuid: $net, is_connected: true } ],
      vm_disks: [ {
        is_cdrom: false,
        disk_address: { device_bus: "scsi", device_index: 0 },
        vm_disk_clone: {
          disk_address: { vmdisk_uuid: $disk },
          minimum_size: $size
        }
      } ]
    }')"
  CREATE_VM="$(api_json POST vms "$VM_BODY")"
  VM_UUID="$(resolve_create_uuid "$CREATE_VM" vm "$NAME")" || {
    echo "VM create failed: $CREATE_VM" >&2
    exit 1
  }
  # Keep the VMID-derived MAC even if create omitted it — pin_nic applies it next.
  NET0_MAC="$(mac_for_vmid "${VMID}")"
  DISK_BUS="scsi"
}
echo "==> created VM uuid=${VM_UUID}"

vm_has_serial() {
  local uuid="$1" det
  det="$(api_get "vms/${uuid}" 2>/dev/null || true)"
  echo "${det:-}" | jq -e '
    ((.vm_serial_ports // .serial_ports // []) | length) > 0
  ' >/dev/null 2>&1
}

# PE ignores vm_serial_ports on POST. Attach while powered off (required for acli/REST).
ensure_serial_port() {
  local uuid="$1" name="$2"
  if vm_has_serial "$uuid"; then
    echo "==> serial port already present" >&2
    return 0
  fi
  echo "==> attach serial port (kServer) for Prism Serial Console" >&2

  # 1) PE v2 PUT — try type spellings PE accepts
  local t body resp
  for t in kServer SERVER server; do
    body="$(jq -n --arg t "$t" '{vm_serial_ports:[{index:0, type:$t}]}')"
    resp="$(api_json PUT "vms/${uuid}" "$body" 2>/dev/null || true)"
    if echo "${resp:-}" | jq -e '.task_uuid' >/dev/null 2>&1; then
      wait_task "$(echo "$resp" | jq -r '.task_uuid')" "serial" >/dev/null || true
    fi
    sleep 1
    if vm_has_serial "$uuid"; then
      echo "==> serial attached via v2 PUT type=${t}" >&2
      return 0
    fi
  done

  # 2) PE/PC v3 intent PUT (serial_port_list)
  local api3="${BASE}/api/nutanix/v3"
  local get3 put3
  get3="$("${CURL[@]}" "${api3}/vms/${uuid}" 2>/dev/null || true)"
  if echo "${get3:-}" | jq -e '.spec.resources' >/dev/null 2>&1; then
    put3="$(echo "$get3" | jq '
      del(.status)
      | .spec.resources.serial_port_list = [{index:0, is_connected:true}]
      | .spec.resources.power_state = "OFF"
    ')"
    resp="$("${CURL[@]}" -X PUT -H 'Content-Type: application/json' -d "$put3" \
      "${api3}/vms/${uuid}" 2>/dev/null || true)"
    # v3 returns 202 + status.execution_context.task_uuid sometimes nested
    local tu
    tu="$(echo "${resp:-}" | jq -r '
      .status.execution_context.task_uuid
      // .task_uuid // empty
    ' 2>/dev/null || true)"
    if [[ -n "$tu" ]]; then
      # v3 tasks live under v3/tasks
      for _ in $(seq 1 60); do
        local st
        st="$("${CURL[@]}" "${api3}/tasks/${tu}" 2>/dev/null | jq -r '.status // empty' || true)"
        case "${st}" in
          SUCCEEDED|Succeeded|COMPLETE|Complete) break ;;
          FAILED|Failed|ABORTED|Aborted) break ;;
        esac
        sleep 2
      done
    else
      sleep 3
    fi
    if vm_has_serial "$uuid"; then
      echo "==> serial attached via v3 PUT serial_port_list" >&2
      return 0
    fi
  fi

  # 3) Optional CVM acli (most reliable)
  local ssh_target="${NUTANIX_CVM_SSH:-${NUTANIX_SSH:-}}"
  if [[ -n "$ssh_target" ]]; then
    echo "==> serial via acli on ${ssh_target}" >&2
    if ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=8 -o BatchMode=yes \
      "$ssh_target" "acli vm.serial_port_create '${name}' type=kServer index=0"; then
      sleep 1
      if vm_has_serial "$uuid"; then
        echo "==> serial attached via acli" >&2
        return 0
      fi
    fi
  fi

  echo "warn: could not attach serial port — Prism Serial Console will be empty" >&2
  echo "      set NUTANIX_CVM_SSH=nutanix@<cvm-ip> (BatchMode key) and recreate, or run:" >&2
  echo "      acli vm.serial_port_create '${name}' type=kServer index=0" >&2
  return 1
}

ensure_serial_port "$VM_UUID" "$NAME" || true
else
  echo "==> repair netcfg on existing VM ${NAME} (no recreate)"
  EXISTING="$(api_get vms)"
  VM_UUID="$(EXISTING_JSON="$EXISTING" python3 -c '
import json,os,sys
want=sys.argv[1]
data=json.loads(os.environ["EXISTING_JSON"])
ents=data.get("entities") or (data if isinstance(data,list) else [data])
for e in ents:
    if e.get("name")==want:
        print(e.get("uuid") or "")
        raise SystemExit
' "$NAME" || true)"
  [[ -n "$VM_UUID" ]] || { echo "VM ${NAME} not found" >&2; exit 1; }
  echo "==> ${NAME} uuid=${VM_UUID}"
  DET="$(api_get "vms/${VM_UUID}?include_vm_nic_config=true" 2>/dev/null || api_get "vms/${VM_UUID}")"
  DISK_BUS="$(echo "$DET" | jq -r '
    (.vm_disks // .vm_disk_info // [])[0].disk_address.device_bus
    // "pci"
  ')"
  [[ -n "$DISK_BUS" && "$DISK_BUS" != "null" ]] || DISK_BUS="pci"
  NET0_MAC="$(echo "$DET" | jq -r '
    (.vm_nics // .nic_list // [])[0].mac_address
    // (.vm_nics // .nic_list // [])[0].mac_addr
    // empty
  ')"
  echo "==> disk bus=${DISK_BUS} mac=${NET0_MAC:-unknown}"
fi

# Inject Prism IPAM address as a tiny extra disk the guest applies at boot.
# AHV IPAM reservations are not DHCP leases — without this the dashboard stays (no ipv4).
fetch_ipam_ips() {
  local uuid="$1" det
  det="$(api_get "vms/${uuid}?include_vm_nic_config=true" 2>/dev/null || true)"
  echo "${det:-}" | jq -r '
    [(.vm_nics // .nic_list // [])[]
      | (.ip_addresses // [])[]?, .ip_address?, .requested_ip_address?, .endpoint_address?
    ] | map(select(. != null and . != "" and (contains(".") ))) | unique | .[]
  ' 2>/dev/null || true
}

# Pin NIC MAC (DHCP identity) and, on managed IPAM, requested_ip_address so Prism
# does not hand out a new IPv4 every power-off/on. Proxmox does this via net0 MAC.
pin_nic() {
  local uuid="$1" mac="${2:-}" ip="${3:-}"
  local det net nic body resp tu api3 get3 put3
  [[ -n "$uuid" ]] || return 0
  det="$(api_get "vms/${uuid}?include_vm_nic_config=true" 2>/dev/null || api_get "vms/${uuid}" || true)"
  net="$(echo "${det:-}" | jq -r '
    (.vm_nics // .nic_list // [])[0].network_uuid
    // (.vm_nics // .nic_list // [])[0].network_uuid
    // empty
  ')"
  [[ -n "$net" && "$net" != "null" ]] || net="${NETWORK_UUID:-}"
  [[ -n "$mac" ]] || mac="$(echo "${det:-}" | jq -r '
    (.vm_nics // .nic_list // [])[0].mac_address
    // (.vm_nics // .nic_list // [])[0].mac_addr
    // empty
  ')"
  [[ -n "$ip" ]] || ip="$(fetch_ipam_ips "$uuid" | head -1 || true)"
  if [[ -z "$net" || -z "$mac" ]]; then
    echo "==> pin_nic: skip (net=${net:-none} mac=${mac:-none})" >&2
    return 0
  fi
  echo "==> pin NIC mac=${mac}${ip:+ ip=${ip}} (sticky across power-off)" >&2
  nic="$(jq -n --arg net "$net" --arg mac "$mac" --arg ip "$ip" '{
      network_uuid: $net,
      is_connected: true,
      mac_address: $mac
    } + (if $ip != "" then {requested_ip_address: $ip} else {} end)')"
  if echo "${det:-}" | jq -e 'type=="object"' >/dev/null 2>&1; then
    body="$(echo "$det" | jq --argjson nic "$nic" '
      del(.vm_disk_info, .stats, .usage_stats, .host_uuid, .host_name)
      | .vm_nics = [$nic]
    ')"
  else
    body="$(jq -n --argjson nic "$nic" '{vm_nics: [$nic]}')"
  fi
  resp="$(api_json PUT "vms/${uuid}" "$body" 2>/dev/null || true)"
  tu="$(echo "${resp:-}" | jq -r '.task_uuid // empty')"
  if [[ -n "$tu" ]]; then
    wait_task "$tu" "nic" >/dev/null || true
    return 0
  fi
  api3="${BASE}/api/nutanix/v3"
  get3="$("${CURL[@]}" "${api3}/vms/${uuid}" 2>/dev/null || true)"
  if echo "${get3:-}" | jq -e '.spec.resources' >/dev/null 2>&1; then
    put3="$(echo "$get3" | jq --arg mac "$mac" --arg ip "$ip" --arg net "$net" '
      del(.status)
      | .spec.resources.power_state = ((.spec.resources.power_state) // "OFF")
      | .spec.resources.nic_list = (if ((.spec.resources.nic_list // []) | length) > 0
          then .spec.resources.nic_list
          else [{ nic_type: "NORMAL_NIC", subnet_reference: { kind: "subnet", uuid: $net } }]
          end)
      | .spec.resources.nic_list[0].mac_address = $mac
      | .spec.resources.nic_list[0].subnet_reference = (.spec.resources.nic_list[0].subnet_reference // { kind: "subnet", uuid: $net })
      | if $ip != "" then
          .spec.resources.nic_list[0].ip_endpoint_list = [{ ip: $ip }]
        else . end
    ')"
    resp="$("${CURL[@]}" -X PUT -H 'Content-Type: application/json' -d "$put3" \
      "${api3}/vms/${uuid}" 2>/dev/null || true)"
    tu="$(echo "${resp:-}" | jq -r '.status.execution_context.task_uuid // .task_uuid // empty')"
    [[ -n "$tu" ]] && wait_v3_task "$tu" || true
  fi
}

netcfg_gateway_for() {
  local ip="$1" via=""
  if [[ -n "${NUTANIX_GATEWAY:-}" ]]; then
    echo "${NUTANIX_GATEWAY}"
    return 0
  fi
  if [[ -n "${LAB_GATEWAY:-}" ]]; then
    echo "${LAB_GATEWAY}"
    return 0
  fi
  if [[ -n "${NETWORK_GATEWAY:-}" ]]; then
    echo "${NETWORK_GATEWAY}"
    return 0
  fi
  # Same L2 as mgmt: the host default route is the guest gateway (e.g. OpenWrt).
  via="$(ip -4 route show default 2>/dev/null | awk '{
    for (i = 1; i < NF; i++) if ($i == "via") { print $(i+1); exit }
  }')"
  if [[ "$via" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ && "$via" == "${ip%.*}."* ]]; then
    echo "$via"
    return 0
  fi
  echo "${ip%.*}.1"
}

netcfg_prefix() {
  if [[ -n "${NETWORK_PREFIX:-}" ]]; then
    echo "${NETWORK_PREFIX}"
    return 0
  fi
  local p="${LAB_SUBNET:-}"
  if [[ "$p" == */* ]]; then
    echo "${p##*/}"
  else
    echo 24
  fi
}

import_raw_image() {
  local path="$1" img_name="$2"
  local addr port url create_img uuid vmdisk state img i
  addr="$(detect_http_addr)"
  port="${NUTANIX_HTTP_PORT:-18765}"
  HTTP_DIR="$(mktemp -d /var/tmp/pertisk-nutanix-http.XXXXXX 2>/dev/null || mktemp -d)"
  ln -sf "$(cd "$(dirname "$path")" && pwd)/$(basename "$path")" "${HTTP_DIR}/disk.raw"
  open_fw_port "$port"
  if ss -ltn "( sport = :${port} )" 2>/dev/null | grep -q ":${port}"; then
    echo "port ${port} already in use — set NUTANIX_HTTP_PORT" >&2
    return 1
  fi
  python3 -m http.server "$port" --bind 0.0.0.0 --directory "$HTTP_DIR" >/dev/null 2>&1 &
  HTTP_PID=$!
  sleep 0.4
  kill -0 "$HTTP_PID" 2>/dev/null || {
    echo "failed to start HTTP server on :${port} for netcfg" >&2
    return 1
  }
  url="http://${addr}:${port}/disk.raw"
  echo "==> Prism netcfg import from ${url}" >&2
  create_img="$(api_json POST images "$(jq -n \
    --arg name "$img_name" \
    --arg cuuid "$CONTAINER_UUID" \
    --arg url "$url" \
    '{
      name: $name,
      image_type: "DISK_IMAGE",
      image_import_spec: {
        storage_container_uuid: $cuuid,
        url: $url
      }
    }')")"
  uuid="$(resolve_create_uuid "$create_img" image "$img_name")" || {
    echo "netcfg image import failed: $create_img" >&2
    stop_http
    return 1
  }
  for i in $(seq 1 180); do
    img="$(api_get "images/${uuid}")"
    state="$(echo "$img" | jq -r '.image_state // .status // empty' | tr 'a-z' 'A-Z')"
    vmdisk="$(echo "$img" | jq -r '.vmdisk_uuid // .vm_disk_id // empty')"
    if [[ "$state" == "ACTIVE" || "$state" == "COMPLETE" ]] && [[ -n "$vmdisk" ]]; then
      NETCFG_IMAGE_UUID="$uuid"
      echo "$vmdisk"
      stop_http
      return 0
    fi
    if [[ "$state" == "ERROR" ]]; then
      echo "netcfg image import ERROR" >&2
      stop_http
      return 1
    fi
    sleep 2
  done
  echo "netcfg image import timed out" >&2
  stop_http
  return 1
}

NETCFG_IMAGE_UUID=""

set_power() {
  local uuid="$1" trans="$2" resp
  echo "==> power ${trans}" >&2
  resp="$(api_json POST "vms/${uuid}/set_power_state" "$(jq -n --arg t "$trans" '{transition:$t}')")"
  if echo "${resp:-}" | jq -e '.task_uuid' >/dev/null 2>&1; then
    wait_task "$(echo "$resp" | jq -r '.task_uuid')" "vm" >/dev/null || return 1
  elif echo "${resp:-}" | jq -e '.message // .error_detail' >/dev/null 2>&1; then
    echo "power ${trans} failed: $resp" >&2
    return 1
  fi
  return 0
}

wait_v3_task() {
  local tu="$1" api3="${BASE}/api/nutanix/v3" st
  for _ in $(seq 1 90); do
    st="$("${CURL[@]}" "${api3}/tasks/${tu}" 2>/dev/null | jq -r '.status // empty' || true)"
    case "${st}" in
      SUCCEEDED|Succeeded|COMPLETE|Complete) return 0 ;;
      FAILED|Failed|ABORTED|Aborted)
        echo "v3 task ${tu} ${st}" >&2
        return 1
        ;;
    esac
    sleep 2
  done
  echo "v3 task ${tu} timed out" >&2
  return 1
}

vm_disk_count() {
  local uuid="$1" det n n3
  det="$(api_get "vms/${uuid}?include_vm_disk_config=true" 2>/dev/null || api_get "vms/${uuid}" 2>/dev/null || true)"
  n="$(echo "${det:-}" | jq '(.vm_disks // .vm_disk_info // []) | length' 2>/dev/null || echo 0)"
  n3="$("${CURL[@]}" "${BASE}/api/nutanix/v3/vms/${uuid}" 2>/dev/null \
    | jq '(.spec.resources.disk_list // []) | length' 2>/dev/null || echo 0)"
  if [[ "${n3:-0}" -gt "${n:-0}" ]]; then
    echo "$n3"
  else
    echo "${n:-0}"
  fi
}

# Extra virtio disks can steal UEFI boot from the OS image (guest never reaches pertiskd).
pin_boot_os_disk() {
  local uuid="$1" bus="${2:-$DISK_BUS}" body resp tu api3 get3 put3 adapter
  echo "==> pin UEFI boot to OS disk (${bus}:0) via v2+v3" >&2
  body="$(jq -n --arg bus "$bus" '{
    boot: {
      uefi_boot: true,
      secure_boot: false,
      boot_device_type: "disk",
      disk_address: { device_bus: $bus, device_index: 0 }
    }
  }')"
  resp="$(api_json PUT "vms/${uuid}" "$body" 2>/dev/null || true)"
  tu="$(echo "${resp:-}" | jq -r '.task_uuid // empty')"
  if [[ -n "$tu" ]]; then
    wait_task "$tu" "boot" >/dev/null || true
  fi
  api3="${BASE}/api/nutanix/v3"
  get3="$("${CURL[@]}" "${api3}/vms/${uuid}" 2>/dev/null || true)"
  echo "${get3:-}" | jq -e '.spec.resources' >/dev/null 2>&1 || return 0
  adapter="$(echo "$get3" | jq -r \
    '.spec.resources.disk_list[0].device_properties.disk_address.adapter_type // "PCI"')"
  put3="$(echo "$get3" | jq --arg adapter "$adapter" '
    del(.status)
    | .spec.resources.power_state = "OFF"
    | .spec.resources.boot_config = (
        (.spec.resources.boot_config // {boot_type:"UEFI"}) + {
          boot_type: "UEFI",
          boot_device: { disk_address: { adapter_type: $adapter, device_index: 0 } }
        }
      )
  ')"
  resp="$("${CURL[@]}" -X PUT -H 'Content-Type: application/json' -d "$put3" \
    "${api3}/vms/${uuid}" 2>/dev/null || true)"
  tu="$(echo "${resp:-}" | jq -r '.status.execution_context.task_uuid // .task_uuid // empty')"
  if [[ -n "$tu" ]]; then
    wait_v3_task "$tu" || true
  fi
}

wait_ipam_ip() {
  local uuid="$1" secs="${2:-20}" i ip
  for i in $(seq 1 "$secs"); do
    ip="$(fetch_ipam_ips "$uuid" | head -1 || true)"
    if [[ "$ip" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
      echo "$ip"
      return 0
    fi
    sleep 1
  done
  return 1
}

learn_ipam_ip() {
  local uuid="$1" ip
  ip="$(wait_ipam_ip "$uuid" 12 || true)"
  if [[ "$ip" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    pin_nic "$uuid" "${NET0_MAC:-}" "$ip"
    echo "$ip"
    return 0
  fi
  echo "==> IPAM empty while powered off — brief power-on to learn address" >&2
  set_power "$uuid" ON || true
  ip="$(wait_ipam_ip "$uuid" 45 || true)"
  if [[ "$ip" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    # Lock the address *before* power-off or IPAM returns it to the pool.
    pin_nic "$uuid" "${NET0_MAC:-}" "$ip"
  fi
  set_power "$uuid" OFF || true
  sleep 2
  if [[ "$ip" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "$ip"
    return 0
  fi
  return 1
}

detach_extra_disk() {
  local uuid="$1" bus="$2" idx="$3" body resp tu
  body="$(jq -n --arg uuid "$uuid" --arg bus "$bus" --argjson idx "$idx" '{
    uuid: $uuid,
    vm_disks: [{ disk_address: { device_bus: $bus, device_index: $idx } }]
  }')"
  resp="$(api_json POST "vms/${uuid}/disks/detach" "$body" 2>/dev/null || true)"
  tu="$(echo "${resp:-}" | jq -r '.task_uuid // empty')"
  [[ -n "$tu" ]] || return 0
  echo "==> disks/detach ${bus}:${idx} task=${tu}" >&2
  wait_task "$tu" "disk" >/dev/null || true
}

# PE v2 attach endpoint is /disks/attach (POST /disks is not the attach API).
attach_via_v2_attach() {
  local uuid="$1" vmdisk="$2" bus="$3" idx="$4" cdrom="$5"
  local body resp tu
  body="$(jq -n \
    --arg uuid "$uuid" --arg disk "$vmdisk" --arg bus "$bus" \
    --argjson idx "$idx" --argjson cdrom "$cdrom" '{
    uuid: $uuid,
    vm_disks: [{
      is_cdrom: $cdrom,
      is_thin_provisioned: true,
      disk_address: { device_bus: $bus, device_index: $idx, vmdisk_uuid: $disk },
      vm_disk_clone: {
        disk_address: { device_bus: $bus, device_index: $idx, vmdisk_uuid: $disk }
      }
    }]
  }')"
  resp="$(api_json POST "vms/${uuid}/disks/attach" "$body" 2>/dev/null || true)"
  tu="$(echo "${resp:-}" | jq -r '.task_uuid // empty')"
  if [[ -z "$tu" ]]; then
    echo "==> disks/attach ${bus}:${idx} cdrom=${cdrom} rejected: $(echo "${resp:-}" | tr -d '\n' | head -c 280)" >&2
    return 1
  fi
  echo "==> disks/attach ${bus}:${idx} cdrom=${cdrom} task=${tu}" >&2
  wait_task "$tu" "disk" >/dev/null
}

attach_via_v3() {
  local uuid="$1" image_uuid="$2" adapter="$3"
  local api3="${BASE}/api/nutanix/v3" get3 put3 resp tu
  [[ -n "$image_uuid" ]] || return 1
  get3="$("${CURL[@]}" "${api3}/vms/${uuid}" 2>/dev/null || true)"
  echo "${get3:-}" | jq -e '.spec.resources.disk_list' >/dev/null 2>&1 || return 1
  adapter="$(echo "${get3}" | jq -r \
    '.spec.resources.disk_list[0].device_properties.disk_address.adapter_type // empty')"
  [[ -n "$adapter" ]] || adapter="${3:-SCSI}"
  echo "==> v3 append netcfg disk (${adapter}:1) from image ${image_uuid}" >&2
  put3="$(echo "$get3" | jq --arg img "$image_uuid" --arg adapter "$adapter" '
    del(.status)
    | .spec.resources.power_state = "OFF"
    | .spec.resources.disk_list += [{
        device_properties: {
          device_type: "DISK",
          disk_address: { adapter_type: $adapter, device_index: 1 }
        },
        data_source_reference: { kind: "image", uuid: $img }
      }]
    | .spec.resources.boot_config = (
        (.spec.resources.boot_config // {}) + {
          boot_device: { disk_address: { adapter_type: $adapter, device_index: 0 } }
        }
      )
  ')"
  resp="$("${CURL[@]}" -X PUT -H 'Content-Type: application/json' -d "$put3" \
    "${api3}/vms/${uuid}" 2>/dev/null || true)"
  tu="$(echo "${resp:-}" | jq -r '
    .status.execution_context.task_uuid // .task_uuid // empty
  ' 2>/dev/null || true)"
  if [[ -z "$tu" ]]; then
    echo "==> v3 disk PUT rejected: $(echo "${resp:-}" | tr -d '\n' | head -c 280)" >&2
    return 1
  fi
  wait_v3_task "$tu"
}

attach_via_acli() {
  local name="$1" vmdisk="$2" bus="$3"
  local ssh_target="${NUTANIX_CVM_SSH:-${NUTANIX_SSH:-}}"
  [[ -n "$ssh_target" ]] || return 1
  echo "==> acli vm.disk_create ${name} clone_from_vmdisk=${vmdisk} bus=${bus}" >&2
  ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=8 -o BatchMode=yes \
    "$ssh_target" "acli vm.disk_create '${name}' clone_from_vmdisk='${vmdisk}' bus='${bus}' index=1"
}

# MAC-filtered DHCPv4 on mgmt so the guest can bind the IPAM address when
# AHV is not actually serving DHCP (reservation ≠ lease).
ensure_ipam_dhcp_helper() {
  local mac="$1" ip="$2" gw="$3" prefix="$4"
  local helper
  helper="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/nutanix-ipam-dhcp.sh"
  if [[ ! -x "$helper" ]]; then
    helper="/usr/share/pertisk-mgmt/scripts/nutanix-ipam-dhcp.sh"
  fi
  if [[ ! -x "$helper" ]]; then
    echo "warn: nutanix-ipam-dhcp.sh not found" >&2
    return 0
  fi
  "$helper" "$mac" "$ip" "$gw" "$prefix" || true
}

attach_netcfg_media() {
  local uuid="$1" vmdisk="$2" bus bus_up
  bus="$DISK_BUS"
  bus_up="$(echo "$bus" | tr 'a-z' 'A-Z')"
  echo "==> attach IPAM netcfg via POST vms/.../disks/attach (${bus}:1)" >&2
  detach_extra_disk "$uuid" "$bus" 1
  detach_extra_disk "$uuid" "$bus_up" 1
  if attach_via_v2_attach "$uuid" "$vmdisk" "$bus" 1 false \
    || attach_via_v2_attach "$uuid" "$vmdisk" "$bus_up" 1 false \
    || { [[ "$bus_up" != "SCSI" ]] && attach_via_v2_attach "$uuid" "$vmdisk" "scsi" 1 false; } \
    || attach_via_v3 "$uuid" "${NETCFG_IMAGE_UUID:-}" "$bus_up" \
    || attach_via_acli "$NAME" "$vmdisk" "$bus"; then
    pin_boot_os_disk "$uuid" "$bus"
    echo "==> netcfg attached; disk count=$(vm_disk_count "$uuid")" >&2
    return 0
  fi
  echo "==> disk attach failed — try IDE CD-ROM via disks/attach" >&2
  if attach_via_v2_attach "$uuid" "$vmdisk" "ide" 0 true \
    || attach_via_v2_attach "$uuid" "$vmdisk" "IDE" 0 true; then
    pin_boot_os_disk "$uuid" "$bus"
    return 0
  fi
  return 1
}

attach_ipam_netcfg() {
  local uuid="$1" ip prefix gw raw img_name vmdisk mac
  ip="$(learn_ipam_ip "$uuid" || true)"
  if [[ ! "$ip" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "==> no Prism IPAM IP yet — guest will try DHCP" >&2
    return 0
  fi
  prefix="$(netcfg_prefix)"
  gw="$(netcfg_gateway_for "$ip")"
  echo "==> IPAM ${ip}/${prefix} gw=${gw} → guest netcfg" >&2
  mac="${NET0_MAC:-}"
  pin_nic "$uuid" "$mac" "$ip"
  ensure_ipam_dhcp_helper "$mac" "$ip" "$gw" "$prefix" || true
  raw="$(mktemp /var/tmp/pertisk-netcfg.XXXXXX.raw)"
  python3 - "$raw" "${ip}/${prefix}" "$gw" <<'PY'
import sys
path, cidr, gw = sys.argv[1], sys.argv[2], sys.argv[3]
blob = f"PERTISK-NET\nIPV4={cidr}\nGATEWAY={gw}\nNAMESERVER={gw}\nINTERFACE=eth0\n".encode()
size = 16 * 1024 * 1024
open(path, "wb").write(blob + b"\x00" * (size - len(blob)))
PY
  img_name="${NAME}-netcfg"
  old="$(find_image_uuid "$img_name" || true)"
  [[ -n "$old" ]] && delete_image "$old"
  if ! vmdisk="$(import_raw_image "$raw" "$img_name")"; then
    echo "warn: netcfg image import failed — guest DHCP helper only" >&2
    rm -f "$raw"
    return 0
  fi
  rm -f "$raw"
  if ! attach_netcfg_media "$uuid" "$vmdisk"; then
    echo "warn: netcfg attach failed — guest will use IPAM DHCP helper on mgmt :67" >&2
  fi
}

if [[ -n "$REPAIR_NAME" ]]; then
  set_power "$VM_UUID" OFF || true
  sleep 2
fi
if [[ -z "${NET0_MAC:-}" ]]; then
  NET0_MAC="$(mac_for_vmid "${VMID}")"
fi
pin_nic "$VM_UUID" "${NET0_MAC}" ""
attach_ipam_netcfg "$VM_UUID"

if [[ -n "$REPAIR_NAME" ]]; then
  echo "==> repair: power off, attach done, power on" >&2
fi

if [[ "$START" == "1" ]]; then
  if ! set_power "$VM_UUID" ON; then
    echo "power on failed for ${NAME} (${VM_UUID})" >&2
    echo "HINT: NoHostResources → lower --memory / worker size, or free AHV capacity." >&2
    echo "      Check Prism: VM may exist but be powered off." >&2
    exit 1
  fi
fi

# Resolve MAC: prefer the one we set; else query with include_vm_nic_config / nics API.
fetch_vm_mac() {
  local uuid="$1" det nics mac
  det="$(api_get "vms/${uuid}?include_vm_nic_config=true" || true)"
  mac="$(echo "${det:-}" | jq -r '
    (.vm_nics // .nic_list // [])
    | map(.mac_address // .mac_addr // empty)
    | map(select(. != null and . != ""))
    | .[0] // empty
  ')"
  if [[ -z "$mac" ]]; then
    nics="$(api_get "vms/${uuid}/nics" 2>/dev/null || api_get "vms/${uuid}/virtual_nics" 2>/dev/null || true)"
    mac="$(echo "${nics:-}" | jq -r '
      (.entities // . // [])
      | if type=="array" then . else [.] end
      | map(.mac_address // .mac_addr // empty)
      | map(select(. != null and . != ""))
      | .[0] // empty
    ' 2>/dev/null || true)"
  fi
  echo "$mac"
}

MAC="${NET0_MAC}"
if [[ -z "$MAC" ]]; then
  for _ in $(seq 1 60); do
    MAC="$(fetch_vm_mac "$VM_UUID")"
    [[ -n "$MAC" ]] && break
    sleep 1
  done
fi
# Confirm API agrees when we set MAC at create (best-effort).
if [[ -n "$NET0_MAC" ]]; then
  GOT="$(fetch_vm_mac "$VM_UUID" || true)"
  if [[ -n "$GOT" && "${GOT,,}" != "${NET0_MAC,,}" ]]; then
    echo "warn: requested mac=${NET0_MAC} but API reports ${GOT} — using API value" >&2
    MAC="$GOT"
  fi
fi
if [[ -z "$MAC" ]]; then
  echo "VM ${NAME} has no MAC after power on (uuid=${VM_UUID})" >&2
  echo "hint: GET vms/${VM_UUID}?include_vm_nic_config=true" >&2
  api_get "vms/${VM_UUID}?include_vm_nic_config=true" 2>/dev/null | head -c 800 >&2 || true
  echo >&2
  exit 1
fi
echo "OK ${NAME} uuid=${VM_UUID} image=${IMAGE_UUID:-repair} mac=${MAC}"
echo "    note: AHV VGA often stays on 'EFI stub: Loaded initrd…' — open Prism → Serial Console"
# IPAM can reserve an address at NIC create; that is not proof the guest booted.
DET="$(api_get "vms/${VM_UUID}?include_vm_nic_config=true" 2>/dev/null || true)"
if [[ -n "${DET:-}" ]]; then
  IPAM_IPS="$(echo "$DET" | jq -r '
    [(.vm_nics // .nic_list // [])[]
      | (.ip_addresses // [])[]?, .ip_address?, .requested_ip_address?, .endpoint_address?
    ] | map(select(. != null and . != "" and (contains(".") ))) | unique | join(" ")
  ' 2>/dev/null || true)"
  SERIAL="$(echo "$DET" | jq -c '.vm_serial_ports // .serial_ports // []' 2>/dev/null || true)"
  [[ -n "${IPAM_IPS:-}" ]] && echo "    prism NIC IP(s): ${IPAM_IPS} (IPAM reserved — ping/:50000 still required)"
  [[ -n "${SERIAL:-}" && "$SERIAL" != "[]" ]] && echo "    serial_ports: ${SERIAL}"
  if [[ -z "${SERIAL:-}" || "$SERIAL" == "[]" || "$SERIAL" == "null" ]]; then
    echo "    warn: no serial_ports on VM — Prism Serial Console will be empty; recreate with kServer" >&2
  fi
fi
