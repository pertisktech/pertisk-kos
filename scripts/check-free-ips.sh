#!/usr/bin/env bash
# Scan a subnet and list free IPv4 addresses (no ICMP reply), so you can pick a
# --static-base / --static-subnet before running cluster create.
#
# Examples:
#   ./scripts/check-free-ips.sh 10.1.1.0/24
#   ./scripts/check-free-ips.sh 10.1.1.0/24 --count 6
#   ./scripts/check-free-ips.sh 10.1.1.0/24 --exclude 10.1.1.111,10.1.1.10
set -euo pipefail

CIDR="${1:-}"
shift || true
COUNT=10
EXCLUDE=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --count) COUNT="$2"; shift 2 ;;
    --exclude) EXCLUDE="$2"; shift 2 ;;
    -h | --help)
      sed -n '2,8p' "$0"
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done

[[ -n "$CIDR" ]] || {
  echo "usage: $0 CIDR [--count N] [--exclude IP[,IP...]]" >&2
  exit 1
}
command -v python3 >/dev/null || { echo "python3 required" >&2; exit 1; }

echo "==> scanning ${CIDR} (excluding gateway${EXCLUDE:+, }${EXCLUDE})..." >&2

python3 - "$CIDR" "$COUNT" "$EXCLUDE" <<'PY'
import ipaddress, subprocess, sys
from concurrent.futures import ThreadPoolExecutor

net = ipaddress.ip_network(sys.argv[1], strict=False)
count = int(sys.argv[2])
excluded = {ipaddress.ip_address(s.strip()) for s in sys.argv[3].split(",") if s.strip()}
gateway = net.network_address + 1

def alive(ip):
    return subprocess.call(
        ["ping", "-c", "1", "-W", "1", str(ip)],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    ) == 0

candidates = [h for h in net.hosts() if h != gateway and h not in excluded]
with ThreadPoolExecutor(max_workers=32) as ex:
    results = list(zip(candidates, ex.map(alive, candidates)))

used = [str(ip) for ip, is_alive in results if is_alive]
free = [str(ip) for ip, is_alive in results if not is_alive]

print(f"free ({len(free)}/{len(candidates)} scanned):")
for ip in free[:count]:
    print(f"  {ip}/{net.prefixlen}")
if len(free) > count:
    print(f"  ... and {len(free) - count} more")
print(f"in use ({len(used)}):")
for ip in used[:20]:
    print(f"  {ip}")
if len(used) > 20:
    print(f"  ... and {len(used) - 20} more")
PY
