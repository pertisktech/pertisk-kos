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
    -h | --help) usage ;;
    *) echo "unknown arg: $1" >&2; usage ;;
  esac
done

[[ -n "${VMID}" && -n "${DISK}" ]] || usage
[[ -f "${DISK}" ]] || {
  echo "disk not found: ${DISK}" >&2
  exit 1
}

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
NETWORK_UUID="$(NETWORKS_JSON="$NETWORKS" python3 -c '
import json,os,sys
want=sys.argv[1]
data=json.loads(os.environ["NETWORKS_JSON"])
ents=data.get("entities") or (data if isinstance(data,list) else [data])
for e in ents:
    if e.get("name")==want:
        print(e.get("uuid") or "")
        raise SystemExit
raise SystemExit("network %r not found" % want)
' "$NETWORK")"
[[ -n "$NETWORK_UUID" ]] || { echo "network '${NETWORK}' not found" >&2; exit 1; }

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

IMAGE_NAME="${IMAGE_NAME:-pertisk-cloud-${VMID}-$(basename "$DISK" .qcow2)}"
echo "==> create/import image ${IMAGE_NAME} (${DISK_BYTES} bytes file)"

IMAGE_UUID="$(find_image_uuid "$IMAGE_NAME")"
VMDISK_UUID=""
if [[ -n "${IMAGE_UUID:-}" ]]; then
  IMG="$(api_get "images/${IMAGE_UUID}")"
  IMG_STATE="$(echo "$IMG" | jq -r '.image_state // .status // empty' | tr 'a-z' 'A-Z')"
  VMDISK_UUID="$(echo "$IMG" | jq -r '.vmdisk_uuid // .vm_disk_id // empty')"
  if [[ "$IMG_STATE" == "ACTIVE" || "$IMG_STATE" == "COMPLETE" ]] && [[ -n "$VMDISK_UUID" ]]; then
    echo "==> reusing ACTIVE image ${IMAGE_UUID}"
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
# Do not set nic "model" — PE rejects "VIRTIO" (InvalidArgument). Default AHV NIC is virtio.
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
    description: "pertisk cloud guest",
    boot: {
      uefi_boot: true,
      secure_boot: false,
      disk_address: { device_bus: "scsi", device_index: 0 }
    },
    vm_serial_ports: [ { index: 0 } ],
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
  echo "warn: VM create rejected; retrying without boot.disk_address" >&2
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
      boot: { uefi_boot: true, secure_boot: false },
      vm_serial_ports: [ { index: 0 } ],
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
}
echo "==> created VM uuid=${VM_UUID}"

if [[ "$START" == "1" ]]; then
  echo "==> power on"
  POW="$(api_json POST "vms/${VM_UUID}/set_power_state" '{"transition":"ON"}')"
  if echo "${POW:-}" | jq -e '.task_uuid' >/dev/null 2>&1; then
    if ! wait_task "$(echo "$POW" | jq -r '.task_uuid')" "vm" >/dev/null; then
      echo "power on failed for ${NAME} (${VM_UUID})" >&2
      echo "HINT: NoHostResources → lower --memory / worker size, or free AHV capacity." >&2
      echo "      Check Prism: VM may exist but be powered off." >&2
      exit 1
    fi
  elif echo "${POW:-}" | jq -e '.message // .error_detail' >/dev/null 2>&1; then
    echo "power on failed: $POW" >&2
    exit 1
  fi
fi

# MAC is assigned when the NIC is live — need power on first.
MAC=""
for _ in $(seq 1 60); do
  DET="$(api_get "vms/${VM_UUID}" || true)"
  MAC="$(echo "${DET:-}" | jq -r '
    (.vm_nics // [])
    | map(.mac_address // .mac_addr // empty)
    | map(select(. != null and . != ""))
    | .[0] // empty
  ')"
  PWR="$(echo "${DET:-}" | jq -r '.power_state // .state // empty' | tr 'a-z' 'A-Z')"
  [[ -n "$MAC" ]] && break
  if [[ "$PWR" == *"OFF"* ]]; then
    echo "VM is powered off — cannot read MAC" >&2
    exit 1
  fi
  sleep 1
done
if [[ -z "$MAC" ]]; then
  echo "VM ${NAME} has no MAC after power on (uuid=${VM_UUID})" >&2
  exit 1
fi
echo "OK ${NAME} uuid=${VM_UUID} image=${IMAGE_UUID} mac=${MAC}"
echo "    note: AHV VGA often stays on 'EFI stub: Loaded initrd…' — open Prism → Serial Console"
