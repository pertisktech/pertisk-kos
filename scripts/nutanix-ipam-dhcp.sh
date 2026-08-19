#!/usr/bin/env bash
# MAC-filtered DHCPv4 on mgmt for AHV IPAM reservations (not guest leases).
#
# Usage:
#   ./scripts/nutanix-ipam-dhcp.sh MAC IP [GATEWAY] [PREFIX]
#
# Binds UDP/67 (needs root). Offers only listed MACs. Re-reads the lease file
# so later VMs can be added without restarting.
set -euo pipefail

MAC="${1:-}"
IP="${2:-}"
GW="${3:-}"
PREFIX="${4:-24}"

if [[ ! "$MAC" =~ ^([0-9a-fA-F]{2}:){5}[0-9a-fA-F]{2}$ ]]; then
  echo "usage: $0 MAC IP [GATEWAY] [PREFIX]" >&2
  exit 1
fi
if [[ ! "$IP" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "invalid IP: ${IP}" >&2
  exit 1
fi

MAC="$(echo "$MAC" | tr 'A-Z' 'a-z')"
if [[ ! "$GW" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  GW="$(ip -4 route show default 2>/dev/null | awk '{
    for (i = 1; i < NF; i++) if ($i == "via") { print $(i+1); exit }
  }')"
fi
if [[ ! "$GW" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] || [[ "$GW" != "${IP%.*}."* ]]; then
  GW="${IP%.*}.1"
fi
[[ "$PREFIX" =~ ^[0-9]+$ ]] || PREFIX=24

LEASES="/var/tmp/pertisk-ipam-dhcp.leases"
PY="/var/tmp/pertisk-ipam-dhcp.py"
PIDF="/var/tmp/pertisk-ipam-dhcp.pid"
mkdir -p /var/tmp
touch "$LEASES"
grep -vi "^${MAC} " "$LEASES" > "${LEASES}.new" || true
echo "${MAC} ${IP} ${GW} ${PREFIX}" >> "${LEASES}.new"
mv "${LEASES}.new" "$LEASES"

SERVER="$(hostname -I 2>/dev/null | awk '{print $1}')"
[[ "$SERVER" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] || SERVER="$GW"

iptables -C INPUT -p udp --dport 67 -j ACCEPT 2>/dev/null \
  || iptables -I INPUT -p udp --dport 67 -j ACCEPT 2>/dev/null || true
if command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
  firewall-cmd --quiet --add-port=67/udp 2>/dev/null || true
fi

cat > "$PY" <<'PY'
import socket, struct, sys, time
leases_path, server_ip = sys.argv[1], sys.argv[2]
deadline = time.time() + 60 * 60

def load():
    out = {}
    try:
        with open(leases_path) as f:
            for line in f:
                p = line.split()
                if len(p) >= 4:
                    out[p[0].lower()] = (p[1], p[2], int(p[3]))
    except OSError:
        pass
    return out

def ip4(s):
    return socket.inet_aton(s)

def netmask(prefix):
    return struct.pack("!I", (0xFFFFFFFF << (32 - prefix)) & 0xFFFFFFFF)

def parse_opts(pkt):
    opts = {}
    if pkt[236:240] != b"\x63\x82\x53\x63":
        return opts
    i = 240
    while i < len(pkt):
        t = pkt[i]
        if t == 255:
            break
        if t == 0:
            i += 1
            continue
        if i + 1 >= len(pkt):
            break
        ln = pkt[i + 1]
        opts[t] = pkt[i + 2 : i + 2 + ln]
        i += 2 + ln
    return opts

def build_reply(req, msgtype, yi, gw, mask, server):
    buf = bytearray(240)
    buf[0] = 2
    buf[1] = 1
    buf[2] = 6
    buf[4:8] = req[4:8]
    buf[10:12] = req[10:12]
    buf[16:20] = yi
    buf[20:24] = server
    buf[24:28] = req[24:28]
    buf[28:44] = req[28:44]
    buf[236:240] = b"\x63\x82\x53\x63"
    lease = struct.pack("!I", 3600)
    opts = (
        bytes([53, 1, msgtype, 1, 4])
        + mask
        + bytes([3, 4])
        + gw
        + bytes([6, 4])
        + gw
        + bytes([51, 4])
        + lease
        + bytes([54, 4])
        + server
        + bytes([255])
    )
    pkt = bytes(buf) + opts
    if len(pkt) < 300:
        pkt += b"\x00" * (300 - len(pkt))
    return pkt

sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
try:
    sock.bind(("", 67))
except OSError as e:
    print("bind :67 failed:", e, file=sys.stderr)
    sys.exit(1)
sock.settimeout(2.0)
server = ip4(server_ip)
print("pertisk-ipam-dhcp :67 server=", server_ip, file=sys.stderr)
while time.time() < deadline:
    try:
        data, _addr = sock.recvfrom(2048)
    except socket.timeout:
        continue
    if len(data) < 240 or data[0] != 1 or data[2] != 6:
        continue
    opts = parse_opts(data)
    mt = opts.get(53, b"\x00")[:1]
    mac = ":".join(f"{b:02x}" for b in data[28:34])
    spec = load().get(mac)
    if not spec:
        continue
    yi_s, gw_s, prefix = spec
    yi, gw, mask = ip4(yi_s), ip4(gw_s), netmask(prefix)
    if mt == b"\x01":
        reply, kind = build_reply(data, 2, yi, gw, mask, server), "offer"
    elif mt == b"\x03":
        reply, kind = build_reply(data, 5, yi, gw, mask, server), "ack"
    else:
        continue
    dest = ("255.255.255.255", 68)
    if data[24:28] != b"\x00\x00\x00\x00":
        dest = (socket.inet_ntoa(data[24:28]), 67)
    sock.sendto(reply, dest)
    print(f"{mac} {kind} {yi_s}", file=sys.stderr)
PY

if [[ -f "$PIDF" ]] && kill -0 "$(cat "$PIDF" 2>/dev/null)" 2>/dev/null; then
  echo "==> IPAM DHCP helper already running; added ${MAC} → ${IP}/${PREFIX} gw=${GW}" >&2
  exit 0
fi

echo "==> IPAM DHCP helper ${MAC} → ${IP}/${PREFIX} gw=${GW} (UDP/67, 60min)" >&2
nohup python3 "$PY" "$LEASES" "$SERVER" >/var/tmp/pertisk-ipam-dhcp.log 2>&1 &
echo $! >"$PIDF"
sleep 0.3
if ! kill -0 "$(cat "$PIDF" 2>/dev/null)" 2>/dev/null; then
  echo "warn: IPAM DHCP helper exited — see /var/tmp/pertisk-ipam-dhcp.log (need root for :67?)" >&2
  exit 1
fi
