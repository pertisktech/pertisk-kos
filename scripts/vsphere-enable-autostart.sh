#!/usr/bin/env bash
# Enable ESXi host Autostart for VMs (power on after host reboot).
#
#   export VSPHERE_URL=https://10.1.1.20
#   export VSPHERE_USER=root
#   export VSPHERE_PASSWORD='…'
#   export VSPHERE_INSECURE=1
#
#   ./scripts/vsphere-enable-autostart.sh              # all VMs
#   ./scripts/vsphere-enable-autostart.sh --prefix lab-ha-vsphere
set -euo pipefail

PREFIX=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix) PREFIX="$2"; shift 2 ;;
    -h | --help)
      echo "Usage: $0 [--prefix NAME]"
      exit 0
      ;;
    *)
      echo "unknown arg: $1" >&2
      exit 1
      ;;
  esac
done

: "${VSPHERE_URL:?set VSPHERE_URL}"
: "${VSPHERE_USER:?set VSPHERE_USER}"
: "${VSPHERE_PASSWORD:?set VSPHERE_PASSWORD}"

BASE="${VSPHERE_URL%/}"
SDK="${BASE}/sdk"
COOKIE_JAR="$(mktemp)"
trap 'rm -f "${COOKIE_JAR}"' EXIT

CURL=(curl -sS)
[[ "${VSPHERE_INSECURE:-0}" == "1" ]] && CURL+=(-k)
CURL+=(-b "${COOKIE_JAR}" -c "${COOKIE_JAR}")

xml_escape() {
  python3 -c 'import sys,xml.sax.saxutils as x; print(x.escape(sys.argv[1]))' "$1"
}

soap() {
  local action="$1" body="$2"
  "${CURL[@]}" -X POST "${SDK}" \
    -H "Content-Type: text/xml; charset=UTF-8" \
    -H "SOAPAction: ${action}" \
    --data-binary @- <<EOF
<?xml version="1.0"?>
<soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
  <soapenv:Body>${body}</soapenv:Body>
</soapenv:Envelope>
EOF
}

echo "==> login ${VSPHERE_USER}@${BASE}"
soap "urn:vim25/8.0.3.0" "<Login xmlns=\"urn:vim25\">
  <_this type=\"SessionManager\">ha-sessionmgr</_this>
  <userName>$(xml_escape "$VSPHERE_USER")</userName>
  <password>$(xml_escape "$VSPHERE_PASSWORD")</password>
</Login>" | grep -q LoginResponse || {
  echo "login failed" >&2
  exit 1
}

LIST_XML="$(soap "urn:vim25/8.0.3.0" "<RetrievePropertiesEx xmlns=\"urn:vim25\">
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
</RetrievePropertiesEx>")"

export PREFIX
POWER_BODY="$(printf '%s' "$LIST_XML" | python3 -c "
import os, re, sys
xml = sys.stdin.read()
prefix = os.environ.get('PREFIX', '').strip()
vms = []
for m in re.finditer(r'<obj[^>]*type=\"VirtualMachine\">([^<]+)</obj>(.*?)</objects>', xml, re.S):
    name_m = re.search(r'<name>name</name>\s*<val[^>]*>([^<]*)</val>', m.group(2))
    if not name_m:
        continue
    name = name_m.group(1)
    if prefix and not name.startswith(prefix):
        continue
    vms.append((m.group(1), name))
vms.sort(key=lambda x: x[1])
if not vms:
    print('no VMs matched', file=sys.stderr)
    sys.exit(2)
parts = []
for i, (moref, name) in enumerate(vms, start=1):
    print(f'  [{i}] {name} ({moref})', file=sys.stderr)
    # startOrder=-1 ("any") — positive contiguous orders are fragile on ESXi
    parts.append(
        '<powerInfo>'
        f'<key type=\"VirtualMachine\">{moref}</key>'
        '<startOrder>-1</startOrder>'
        '<startDelay>-1</startDelay>'
        '<waitForHeartbeat>no</waitForHeartbeat>'
        '<startAction>powerOn</startAction>'
        '<stopDelay>-1</stopDelay>'
        '<stopAction>systemDefault</stopAction>'
        '</powerInfo>'
    )
print(''.join(parts))
")"

echo "==> ReconfigureAutostart (enabled=true)"
RESP="$(soap "urn:vim25/8.0.3.0" "<ReconfigureAutostart xmlns=\"urn:vim25\">
  <_this type=\"HostAutoStartManager\">ha-autostart-mgr</_this>
  <spec>
    <defaults>
      <enabled>true</enabled>
      <startDelay>60</startDelay>
      <stopDelay>60</stopDelay>
      <waitForHeartbeat>false</waitForHeartbeat>
      <stopAction>PowerOff</stopAction>
    </defaults>
    ${POWER_BODY}
  </spec>
</ReconfigureAutostart>")"
if echo "$RESP" | grep -qi 'Fault\|faultstring'; then
  echo "failed: $RESP" >&2
  exit 1
fi

CFG="$(soap "urn:vim25/8.0.3.0" "<RetrievePropertiesEx xmlns=\"urn:vim25\">
  <_this type=\"PropertyCollector\">ha-property-collector</_this>
  <specSet>
    <propSet><type>HostAutoStartManager</type><all>false</all><pathSet>config</pathSet></propSet>
    <objectSet><obj type=\"HostAutoStartManager\">ha-autostart-mgr</obj></objectSet>
  </specSet>
  <options></options>
</RetrievePropertiesEx>")"
echo "$CFG" | grep -q '<enabled>true</enabled>' || {
  echo "verify failed: defaults.enabled not true" >&2
  exit 1
}
COUNT="$(echo "$CFG" | grep -c '<powerInfo>' || true)"
echo "==> done: host autostart enabled, ${COUNT} VM(s) in powerInfo"
echo "    Host Client: Manage → System → Autostart"
