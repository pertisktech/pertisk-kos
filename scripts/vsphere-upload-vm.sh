#!/usr/bin/env bash
# Upload a Pertisk cloud qcow2 to standalone ESXi and create a UEFI VM.
#
# Auth (username/password — ESXi HostAgent SOAP):
#   export VSPHERE_URL="https://10.1.1.20"
#   export VSPHERE_USER="root"
#   export VSPHERE_PASSWORD="…"
#   export VSPHERE_DATASTORE="datastore1"
#   export VSPHERE_NETWORK="VM Network"
#   export VSPHERE_INSECURE=1
#
#   ./scripts/vsphere-upload-vm.sh --vmid 9100 --name lab-9100 \
#     --disk out/pertisk-cloud-amd64.qcow2
set -euo pipefail

VMID=""
NAME=""
DISK=""
MEMORY="${VSPHERE_MEMORY:-4096}"
CORES="${VSPHERE_CORES:-2}"
DISK_GB="${VSPHERE_DISK_GB:-}"
NETWORK="${VSPHERE_NETWORK:-VM Network}"
DATASTORE="${VSPHERE_DATASTORE:-datastore1}"
START=1
DC_PATH="${VSPHERE_DC_PATH:-ha-datacenter}"

usage() {
  cat <<'EOF'
Usage:
  ./scripts/vsphere-upload-vm.sh --vmid ID --disk PATH [options]

Options:
  --vmid N          numeric id used in default name PREFIX-N (required)
  --disk PATH       qcow2 path (required)
  --name NAME       VM name (default: ${NAME_PREFIX:-pertisk}-$VMID)
  --memory MB       RAM (default 4096; env VSPHERE_MEMORY)
  --cores N         vCPUs (default 2; env VSPHERE_CORES)
  --disk-gb N       grow primary disk to N GiB after create (env VSPHERE_DISK_GB)
  --network NAME    portgroup (default $VSPHERE_NETWORK or VM Network)
  --datastore NAME  datastore (default $VSPHERE_DATASTORE or datastore1)
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
    --datastore) DATASTORE="$2"; shift 2 ;;
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

: "${VSPHERE_URL:?set VSPHERE_URL}"
: "${VSPHERE_USER:?set VSPHERE_USER}"
: "${VSPHERE_PASSWORD:?set VSPHERE_PASSWORD}"

NAME="${NAME:-${NAME_PREFIX:-pertisk}-${VMID}}"
BASE="${VSPHERE_URL%/}"
SDK="${BASE}/sdk"
COOKIE_JAR="$(mktemp)"
# Prefer /var/tmp over /tmp — Docker bind-mounts of host /tmp often do not
# persist writes (PrivateTmp, rootless, or tmpfs isolation on Alma/RHEL).
WORKDIR="$(mktemp -d "${VSPHERE_TMPDIR:-/var/tmp}/pertisk-vsphere.XXXXXX")"
trap 'rm -f "${COOKIE_JAR}"; rm -rf "${WORKDIR}"' EXIT

CURL=(curl -sS)
[[ "${VSPHERE_INSECURE:-0}" == "1" ]] && CURL+=(-k)
CURL+=(-b "${COOKIE_JAR}" -c "${COOKIE_JAR}")

command -v python3 >/dev/null || {
  echo "python3 required" >&2
  exit 1
}

# Convert qcow2 → streamOptimized VMDK (single file, sparse-friendly upload).
# Prefer host qemu-img; else alpine via docker.
convert_qcow_to_vmdk() {
  local src="$1" dst="$2"
  if command -v qemu-img >/dev/null 2>&1; then
    qemu-img convert -p -f qcow2 -O vmdk \
      -o subformat=streamOptimized,adapter_type=lsilogic \
      "${src}" "${dst}"
  elif command -v docker >/dev/null 2>&1; then
    local src_dir src_base dst_dir dst_base vol_opts=""
    src_dir="$(cd "$(dirname "$src")" && pwd)"
    src_base="$(basename "$src")"
    mkdir -p "$(dirname "$dst")"
    dst_dir="$(cd "$(dirname "$dst")" && pwd)"
    dst_base="$(basename "$dst")"
    if command -v getenforce >/dev/null 2>&1 && [[ "$(getenforce 2>/dev/null)" != "Disabled" ]]; then
      vol_opts=":Z"
    fi
    echo "==> qemu-img via docker (alpine) → ${dst}"
    docker run --rm \
      -v "${src_dir}:/src:ro${vol_opts}" \
      -v "${dst_dir}:/dst${vol_opts}" \
      alpine sh -c "apk add --no-cache qemu-img >/dev/null && qemu-img convert -p -f qcow2 -O vmdk -o subformat=streamOptimized,adapter_type=lsilogic /src/${src_base} /dst/${dst_base} && sync"
  else
    echo "qemu-img required (or docker) to convert qcow2 → vmdk" >&2
    echo "hint: dnf install -y qemu-img   # preferred on mgmt hosts" >&2
    exit 1
  fi
  if [[ ! -f "$dst" ]]; then
    echo "convert failed: output missing at ${dst}" >&2
    echo "hint: install host qemu-img (dnf install -y qemu-img) or set VSPHERE_TMPDIR to a Docker-visible path" >&2
    exit 1
  fi
  local sz
  sz="$(stat -c%s "$dst" 2>/dev/null || stat -f%z "$dst")"
  if [[ -z "$sz" || "$sz" -lt 1048576 ]]; then
    echo "convert failed: ${dst} is only ${sz:-0} bytes (expected >= 1MiB)" >&2
    exit 1
  fi
}

qcow_virtual_bytes() {
  local src="$1" out
  if command -v qemu-img >/dev/null 2>&1; then
    out="$(qemu-img info --output=json "$src" 2>/dev/null | python3 -c 'import sys,json; print(json.load(sys.stdin).get("virtual-size",0))' 2>/dev/null || echo 0)"
  elif command -v docker >/dev/null 2>&1; then
    local src_dir src_base vol_opts=""
    src_dir="$(cd "$(dirname "$src")" && pwd)"
    src_base="$(basename "$src")"
    if command -v getenforce >/dev/null 2>&1 && [[ "$(getenforce 2>/dev/null)" != "Disabled" ]]; then
      vol_opts=":Z"
    fi
    out="$(docker run --rm -v "${src_dir}:/src:ro${vol_opts}" alpine \
      sh -c "apk add --no-cache qemu-img >/dev/null && qemu-img info --output=json /src/${src_base}" \
      | python3 -c 'import sys,json; print(json.load(sys.stdin).get("virtual-size",0))' 2>/dev/null || echo 0)"
  else
    out=0
  fi
  echo "${out:-0}"
}

xml_escape() {
  local s="$1"
  s="${s//&/&amp;}"
  s="${s//</&lt;}"
  s="${s//>/&gt;}"
  s="${s//\"/&quot;}"
  s="${s//\'/&apos;}"
  printf '%s' "$s"
}

soap() {
  local action="$1" body="$2"
  "${CURL[@]}" -X POST "${SDK}" \
    -H "Content-Type: text/xml; charset=UTF-8" \
    -H "SOAPAction: ${action}" \
    --data-binary @- <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
  <soapenv:Body>
${body}
  </soapenv:Body>
</soapenv:Envelope>
EOF
}

# wait_task TASK LABEL [soft]
# soft=1 → treat "not found" / missing-file errors as success (quiet).
wait_task() {
  local task="$1" label="${2:-task}" soft="${3:-0}" i state msg
  [[ -n "$task" ]] || return 0
  [[ "$soft" == "1" ]] || echo "==> waiting for ${label}: ${task}"
  for i in $(seq 1 600); do
    local resp
    resp="$(soap "urn:vim25/8.0.3.0" "<RetrieveProperties xmlns=\"urn:vim25\">
  <_this type=\"PropertyCollector\">ha-property-collector</_this>
  <specSet>
    <propSet><type>Task</type><all>false</all><pathSet>info</pathSet></propSet>
    <objectSet><obj type=\"Task\">$(xml_escape "$task")</obj><skip>false</skip></objectSet>
  </specSet>
</RetrieveProperties>")"
    state="$(echo "$resp" | sed -n 's/.*<state>\([^<]*\)<\/state>.*/\1/p' | head -1)"
    if [[ "$state" == "success" ]]; then
      [[ "$soft" == "1" ]] || echo "==> ${label} OK"
      return 0
    fi
    if [[ "$state" == "error" ]]; then
      msg="$(echo "$resp" | sed -n 's/.*<localizedMessage>\([^<]*\)<\/localizedMessage>.*/\1/p' | head -1)"
      if [[ "$soft" == "1" ]] && [[ "${msg}" == *"not found"* || "${msg}" == *"NotFound"* || "${msg}" == *"was not found"* ]]; then
        return 0
      fi
      echo "${label} failed: ${msg:-$resp}" >&2
      return 1
    fi
    sleep 1
  done
  echo "${label} timed out" >&2
  return 1
}

task_moref() {
  # <returnval type="Task">task-123</returnval>
  sed -n 's/.*<returnval[^>]*>\([^<]*\)<\/returnval>.*/\1/p' | head -1
}

echo "==> ESXi ${VSPHERE_URL} datastore=${DATASTORE} network=${NETWORK} name=${NAME} vmid=${VMID}"

echo "==> login as ${VSPHERE_USER}"
LOGIN_RESP="$(soap "urn:vim25/8.0.3.0" "<Login xmlns=\"urn:vim25\">
  <_this type=\"SessionManager\">ha-sessionmgr</_this>
  <userName>$(xml_escape "${VSPHERE_USER}")</userName>
  <password>$(xml_escape "${VSPHERE_PASSWORD}")</password>
</Login>")"
echo "$LOGIN_RESP" | grep -q LoginResponse || {
  echo "login failed: $LOGIN_RESP" >&2
  exit 1
}

# Destroy existing VM with same name (recreate).
EXIST="$(soap "urn:vim25/8.0.3.0" "<RetrievePropertiesEx xmlns=\"urn:vim25\">
  <_this type=\"PropertyCollector\">ha-property-collector</_this>
  <specSet>
    <propSet><type>VirtualMachine</type><all>false</all><pathSet>name</pathSet><pathSet>runtime.powerState</pathSet></propSet>
    <objectSet>
      <obj type=\"Folder\">ha-folder-root</obj>
      <skip>false</skip>
      <selectSet xsi:type=\"TraversalSpec\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">
        <name>visitFolders</name><type>Folder</type><path>childEntity</path><skip>false</skip>
        <selectSet><name>visitFolders</name></selectSet>
        <selectSet><name>dcToVmf</name></selectSet>
        <selectSet><name>crToH</name></selectSet>
        <selectSet><name>HToVm</name></selectSet>
      </selectSet>
      <selectSet xsi:type=\"TraversalSpec\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">
        <name>dcToVmf</name><type>Datacenter</type><path>vmFolder</path><skip>false</skip>
        <selectSet><name>visitFolders</name></selectSet>
      </selectSet>
      <selectSet xsi:type=\"TraversalSpec\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">
        <name>crToH</name><type>ComputeResource</type><path>host</path><skip>false</skip>
        <selectSet><name>HToVm</name></selectSet>
      </selectSet>
      <selectSet xsi:type=\"TraversalSpec\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">
        <name>HToVm</name><type>HostSystem</type><path>vm</path><skip>false</skip>
      </selectSet>
    </objectSet>
  </specSet>
  <options></options>
</RetrievePropertiesEx>")"
VM_MOREF="$(echo "$EXIST" | python3 -c "
import sys,re
xml=sys.stdin.read()
want=$(printf '%s' "$NAME" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')
for m in re.finditer(r'<obj[^>]*type=\"VirtualMachine\">([^<]+)</obj>(.*?)</objects>', xml, re.S):
    props=dict(re.findall(r'<name>([^<]+)</name>\s*<val[^>]*>([^<]*)</val>', m.group(2)))
    if props.get('name')==want:
        print(m.group(1)); break
" 2>/dev/null || true)"

if [[ -n "${VM_MOREF}" ]]; then
  echo "==> VM ${NAME} exists (${VM_MOREF}) — destroying"
  POWER="$(echo "$EXIST" | python3 -c "
import sys,re
xml=sys.stdin.read()
want=$(printf '%s' "$NAME" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')
for m in re.finditer(r'<obj[^>]*type=\"VirtualMachine\">([^<]+)</obj>(.*?)</objects>', xml, re.S):
    props=dict(re.findall(r'<name>([^<]+)</name>\s*<val[^>]*>([^<]*)</val>', m.group(2)))
    if props.get('name')==want:
        print(props.get('runtime.powerState','')); break
" 2>/dev/null || true)"
  if [[ "$POWER" == "poweredOn" ]]; then
    TASK="$(soap "urn:vim25/8.0.3.0" "<PowerOffVM_Task xmlns=\"urn:vim25\"><_this type=\"VirtualMachine\">$(xml_escape "$VM_MOREF")</_this></PowerOffVM_Task>" | task_moref)"
    wait_task "$TASK" "power-off" || true
    sleep 2
  fi
  TASK="$(soap "urn:vim25/8.0.3.0" "<Destroy_Task xmlns=\"urn:vim25\"><_this type=\"VirtualMachine\">$(xml_escape "$VM_MOREF")</_this></Destroy_Task>" | task_moref)"
  wait_task "$TASK" "destroy"
fi

# Convert qcow2 → streamOptimized VMDK (upload size ≈ used data, not full virtual size).
UPLOAD_VMDK="${WORKDIR}/${NAME}-upload.vmdk"
echo "==> converting $(basename "$DISK") → streamOptimized VMDK"
convert_qcow_to_vmdk "${DISK}" "${UPLOAD_VMDK}"
UPLOAD_BYTES="$(stat -c%s "${UPLOAD_VMDK}" 2>/dev/null || stat -f%z "${UPLOAD_VMDK}")"
VIRT_BYTES="$(qcow_virtual_bytes "${DISK}")"
if [[ -z "$VIRT_BYTES" || "$VIRT_BYTES" -lt 1048576 ]]; then
  # Fallback: treat upload file virtual size unknown → use --disk-gb or 50G.
  if [[ -n "${DISK_GB}" ]]; then
    VIRT_BYTES=$((DISK_GB * 1024 * 1024 * 1024))
  else
    VIRT_BYTES=$((50 * 1024 * 1024 * 1024))
  fi
fi
echo "==> upload size=${UPLOAD_BYTES} bytes (virtual=${VIRT_BYTES} bytes)"

# Make directory on datastore (ignore already-exists).
FOLDER_URL="${BASE}/folder/$(python3 -c "import urllib.parse; print(urllib.parse.quote('''${NAME}''', safe=''))")?dcPath=$(python3 -c "import urllib.parse; print(urllib.parse.quote('''${DC_PATH}'''))")&dsName=$(python3 -c "import urllib.parse; print(urllib.parse.quote('''${DATASTORE}'''))")"
echo "==> ensuring datastore folder ${NAME}"
"${CURL[@]}" -X MKCOL "${FOLDER_URL}" >/dev/null 2>&1 || true

# Best-effort delete leftover disks from a prior failed run (missing = ok).
delete_ds_file() {
  local path="$1"
  local resp task
  resp="$(soap "urn:vim25/8.0.3.0" "<DeleteDatastoreFile_Task xmlns=\"urn:vim25\">
  <_this type=\"FileManager\">ha-nfc-file-manager</_this>
  <name>$(xml_escape "$path")</name>
  <datacenter type=\"Datacenter\">ha-datacenter</datacenter>
</DeleteDatastoreFile_Task>" 2>/dev/null || true)"
  task="$(echo "$resp" | task_moref || true)"
  if [[ -n "$task" ]]; then
    wait_task "$task" "delete $(basename "$path")" 1 || true
  fi
}
delete_ds_file "[${DATASTORE}] ${NAME}/${NAME}.vmdk"
delete_ds_file "[${DATASTORE}] ${NAME}/${NAME}-flat.vmdk"
delete_ds_file "[${DATASTORE}] ${NAME}/${NAME}-upload.vmdk"
delete_ds_file "[${DATASTORE}] ${NAME}/${NAME}-upload-flat.vmdk"

upload_file() {
  local src="$1" dest_name="$2"
  local url="${BASE}/folder/$(python3 -c "import urllib.parse; print(urllib.parse.quote('''${NAME}''', safe=''))")/$(python3 -c "import urllib.parse; print(urllib.parse.quote('''${dest_name}''', safe=''))")?dcPath=$(python3 -c "import urllib.parse; print(urllib.parse.quote('''${DC_PATH}'''))")&dsName=$(python3 -c "import urllib.parse; print(urllib.parse.quote('''${DATASTORE}'''))")"
  local bytes
  bytes="$(stat -c%s "${src}" 2>/dev/null || stat -f%z "${src}")"
  echo "==> uploading ${dest_name} (${bytes} bytes)"
  "${CURL[@]}" -X PUT --upload-file "${src}" \
    -H "Content-Type: application/octet-stream" \
    -H "Content-Length: ${bytes}" \
    "${url}" >/dev/null
}

upload_file "${UPLOAD_VMDK}" "${NAME}-upload.vmdk"

SRC_DISK="[${DATASTORE}] ${NAME}/${NAME}-upload.vmdk"
DST_DISK="[${DATASTORE}] ${NAME}/${NAME}.vmdk"
echo "==> CopyVirtualDisk ${SRC_DISK} → ${DST_DISK} (thin VMFS)"
COPY_RESP="$(soap "urn:vim25/8.0.3.0" "<CopyVirtualDisk_Task xmlns=\"urn:vim25\">
  <_this type=\"VirtualDiskManager\">ha-vdiskmanager</_this>
  <sourceName>$(xml_escape "$SRC_DISK")</sourceName>
  <destName>$(xml_escape "$DST_DISK")</destName>
  <destSpec>
    <diskType>thin</diskType>
    <adapterType>lsiLogic</adapterType>
  </destSpec>
</CopyVirtualDisk_Task>")"
COPY_TASK="$(echo "$COPY_RESP" | task_moref)"
[[ -n "$COPY_TASK" ]] || {
  echo "CopyVirtualDisk failed: $COPY_RESP" >&2
  exit 1
}
wait_task "$COPY_TASK" "CopyVirtualDisk"

# Resolve network MoRef by name.
NET_MOREF="$(soap "urn:vim25/8.0.3.0" "<RetrievePropertiesEx xmlns=\"urn:vim25\">
  <_this type=\"PropertyCollector\">ha-property-collector</_this>
  <specSet>
    <propSet><type>Network</type><all>false</all><pathSet>name</pathSet></propSet>
    <objectSet>
      <obj type=\"Folder\">ha-folder-root</obj>
      <skip>false</skip>
      <selectSet xsi:type=\"TraversalSpec\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">
        <name>visitFolders</name><type>Folder</type><path>childEntity</path><skip>false</skip>
        <selectSet><name>visitFolders</name></selectSet>
        <selectSet><name>dcToNet</name></selectSet>
        <selectSet><name>crToNet</name></selectSet>
      </selectSet>
      <selectSet xsi:type=\"TraversalSpec\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">
        <name>dcToNet</name><type>Datacenter</type><path>networkFolder</path><skip>false</skip>
        <selectSet><name>visitFolders</name></selectSet>
      </selectSet>
      <selectSet xsi:type=\"TraversalSpec\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">
        <name>crToNet</name><type>ComputeResource</type><path>network</path><skip>false</skip>
      </selectSet>
    </objectSet>
  </specSet>
  <options></options>
</RetrievePropertiesEx>" | python3 -c "
import sys,re
xml=sys.stdin.read()
want=$(printf '%s' "$NETWORK" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')
for m in re.finditer(r'<obj[^>]*type=\"Network\">([^<]+)</obj>(.*?)</objects>', xml, re.S):
    props=dict(re.findall(r'<name>([^<]+)</name>\s*<val[^>]*>([^<]*)</val>', m.group(2)))
    if props.get('name')==want:
        print(m.group(1)); break
")"
[[ -n "$NET_MOREF" ]] || {
  echo "network not found: ${NETWORK}" >&2
  exit 1
}

CAP_KB=$((VIRT_BYTES / 1024))
if [[ -n "${DISK_GB}" ]]; then
  WANT_KB=$((DISK_GB * 1024 * 1024))
  if [[ "$WANT_KB" -gt "$CAP_KB" ]]; then
    CAP_KB="$WANT_KB"
  fi
fi

DISK_PATH="$DST_DISK"
echo "==> CreateVM_Task name=${NAME} memory=${MEMORY} cores=${CORES} disk=${DISK_PATH} capacityKB=${CAP_KB}"

CREATE_BODY="<CreateVM_Task xmlns=\"urn:vim25\">
  <_this type=\"Folder\">ha-folder-vm</_this>
  <config>
    <name>$(xml_escape "$NAME")</name>
    <guestId>otherLinux64Guest</guestId>
    <files>
      <vmPathName>[$(xml_escape "$DATASTORE")] $(xml_escape "$NAME")/$(xml_escape "$NAME").vmx</vmPathName>
    </files>
    <numCPUs>${CORES}</numCPUs>
    <memoryMB>${MEMORY}</memoryMB>
    <deviceChange>
      <operation>add</operation>
      <device xsi:type=\"VirtualLsiLogicController\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">
        <key>1000</key>
        <busNumber>0</busNumber>
        <sharedBus>noSharing</sharedBus>
      </device>
    </deviceChange>
    <deviceChange>
      <operation>add</operation>
      <device xsi:type=\"VirtualDisk\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">
        <key>2000</key>
        <backing xsi:type=\"VirtualDiskFlatVer2BackingInfo\">
          <fileName>$(xml_escape "$DISK_PATH")</fileName>
          <diskMode>persistent</diskMode>
        </backing>
        <controllerKey>1000</controllerKey>
        <unitNumber>0</unitNumber>
        <capacityInKB>${CAP_KB}</capacityInKB>
      </device>
    </deviceChange>
    <deviceChange>
      <operation>add</operation>
      <device xsi:type=\"VirtualE1000e\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">
        <key>4000</key>
        <backing xsi:type=\"VirtualEthernetCardNetworkBackingInfo\">
          <deviceName>$(xml_escape "$NETWORK")</deviceName>
          <network type=\"Network\">$(xml_escape "$NET_MOREF")</network>
        </backing>
        <connectable>
          <startConnected>true</startConnected>
          <allowGuestControl>true</allowGuestControl>
          <connected>true</connected>
        </connectable>
        <addressType>generated</addressType>
      </device>
    </deviceChange>
    <deviceChange>
      <operation>add</operation>
      <device xsi:type=\"VirtualSerialPort\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">
        <key>3000</key>
        <backing xsi:type=\"VirtualSerialPortURIBackingInfo\">
          <serviceURI>telnet://:$(xml_escape "${SERIAL_PORT:-$((23000 + VMID))}")</serviceURI>
          <direction>server</direction>
        </backing>
        <connectable>
          <startConnected>true</startConnected>
          <allowGuestControl>true</allowGuestControl>
          <connected>true</connected>
        </connectable>
        <yieldOnPoll>true</yieldOnPoll>
      </device>
    </deviceChange>
    <bootOptions>
      <efiSecureBootEnabled>false</efiSecureBootEnabled>
    </bootOptions>
    <firmware>efi</firmware>
  </config>
  <pool type=\"ResourcePool\">ha-root-pool</pool>
  <host type=\"HostSystem\">ha-host</host>
</CreateVM_Task>"

CREATE_RESP="$(soap "urn:vim25/8.0.3.0" "$CREATE_BODY")"
TASK="$(echo "$CREATE_RESP" | task_moref)"
[[ -n "$TASK" ]] || {
  echo "CreateVM failed: $CREATE_RESP" >&2
  exit 1
}
wait_task "$TASK" "CreateVM"

find_vm_moref() {
  local want_name="$1"
  soap "urn:vim25/8.0.3.0" "<RetrievePropertiesEx xmlns=\"urn:vim25\">
  <_this type=\"PropertyCollector\">ha-property-collector</_this>
  <specSet>
    <propSet><type>VirtualMachine</type><all>false</all><pathSet>name</pathSet></propSet>
    <objectSet>
      <obj type=\"Folder\">ha-folder-vm</obj>
      <skip>false</skip>
      <selectSet xsi:type=\"TraversalSpec\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">
        <name>visitFolders</name><type>Folder</type><path>childEntity</path><skip>false</skip>
        <selectSet><name>visitFolders</name></selectSet>
      </selectSet>
    </objectSet>
  </specSet>
  <options></options>
</RetrievePropertiesEx>" | python3 -c "
import sys,re
xml=sys.stdin.read()
want=$(printf '%s' "$want_name" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')
for m in re.finditer(r'<obj[^>]*type=\"VirtualMachine\">([^<]+)</obj>(.*?)</objects>', xml, re.S):
    props=dict(re.findall(r'<name>([^<]+)</name>\s*<val[^>]*>([^<]*)</val>', m.group(2)))
    if props.get('name')==want:
        print(m.group(1)); break
"
}

# Register VM for host autostart after ESXi reboot (HostAutoStartManager).
enable_vm_autostart() {
  local moref="$1" order="$2"
  echo "==> enable host autostart for ${NAME} (order=${order})"
  local resp
  resp="$(soap "urn:vim25/8.0.3.0" "<ReconfigureAutostart xmlns=\"urn:vim25\">
  <_this type=\"HostAutoStartManager\">ha-autostart-mgr</_this>
  <spec>
    <defaults>
      <enabled>true</enabled>
      <startDelay>60</startDelay>
      <stopDelay>60</stopDelay>
      <waitForHeartbeat>false</waitForHeartbeat>
      <stopAction>PowerOff</stopAction>
    </defaults>
    <powerInfo>
      <key type=\"VirtualMachine\">$(xml_escape "$moref")</key>
      <startOrder>${order}</startOrder>
      <startDelay>-1</startDelay>
      <waitForHeartbeat>systemDefault</waitForHeartbeat>
      <startAction>powerOn</startAction>
      <stopDelay>-1</stopDelay>
      <stopAction>systemDefault</stopAction>
    </powerInfo>
  </spec>
</ReconfigureAutostart>")"
  if echo "$resp" | grep -qi 'Fault\|faultstring'; then
    echo "warn: autostart configure failed: $resp" >&2
  fi
}

VM_MOREF="$(find_vm_moref "$NAME")"
[[ -n "$VM_MOREF" ]] || {
  echo "created VM but could not resolve MoRef" >&2
  exit 1
}
# Prefer VMID as start order so CP (lower id) comes up before workers.
enable_vm_autostart "$VM_MOREF" "${VMID}"

if [[ "$START" == "1" ]]; then
  echo "==> powering on ${NAME} (${VM_MOREF})"
  TASK="$(soap "urn:vim25/8.0.3.0" "<PowerOnVM_Task xmlns=\"urn:vim25\"><_this type=\"VirtualMachine\">$(xml_escape "$VM_MOREF")</_this></PowerOnVM_Task>" | task_moref)"
  wait_task "$TASK" "power-on"
fi

echo "==> done: ${NAME}"
echo "    Autostart: enabled (powers on after ESXi host reboot)."
echo "    Host Client often stays on 'EFI stub: Loaded initrd...' until vmwgfx/simpledrm load — that alone is not failure."
echo "    Serial (if ESXi firewall allows): telnet <esxi-ip> $((${SERIAL_PORT:-$((23000 + VMID))}))"
echo "    Prefer: wait for DHCP / :50000 / lab-up (first boot can take several minutes)."
