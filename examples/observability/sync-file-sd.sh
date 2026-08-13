#!/bin/sh
# Write compose/file_sd/nodes.yml from pertisk-mgmt SQLite (ready nodes with IPv4).
# Run on the mgmt / compose host. Prometheus file_sd reloads ~30s.
#
#   ./sync-file-sd.sh
#   MGMT_DB=/var/lib/pertisk-mgmt/mgmt.db ./sync-file-sd.sh
#   SYNC_INTERVAL=30 ./sync-file-sd.sh   # loop (compose sidecar)
set -eu

ROOT="$(CDPATH= cd "$(dirname "$0")" && pwd)"
OUT="${FILE_SD_OUT:-$ROOT/compose/file_sd/nodes.yml}"
DB="${MGMT_DB:-/var/lib/pertisk-mgmt/mgmt.db}"
INTERVAL="${SYNC_INTERVAL:-0}"

sync_once() {
  if [ ! -f "$DB" ]; then
    echo "mgmt db not found: $DB" >&2
    return 1
  fi
  python3 - "$DB" "$OUT" <<'PY'
import json, os, sqlite3, sys

db, out = sys.argv[1], sys.argv[2]
c = sqlite3.connect(db)
rows = list(
    c.execute(
        """
        SELECT c.name, n.name, n.role, n.ip
        FROM nodes n JOIN clusters c ON c.id = n.cluster_id
        WHERE n.ip IS NOT NULL AND n.ip != '' AND n.status = 'ready'
        ORDER BY c.name, n.name
        """
    )
)

def ystr(s):
    s = "" if s is None else str(s)
    if not s or any(ch in s for ch in " #:{}[]&*?|>!%@`'\",") or s.lower() in (
        "true", "false", "null", "yes", "no",
    ):
        return json.dumps(s)
    return s

os.makedirs(os.path.dirname(out) or ".", exist_ok=True)
lines = ["# generated from pertisk-mgmt nodes — re-run sync-file-sd.sh after scale"]
for cluster, name, role, ip in rows:
    lines += [
        "- targets:",
        f"    - {ip}:50001",
        "  labels:",
        f"    cluster: {ystr(cluster)}",
        f"    role: {ystr(role)}",
        f"    hostname: {ystr(name)}",
    ]
if not rows:
    lines.append("[]")
tmp = out + ".tmp"
open(tmp, "w").write("\n".join(lines) + "\n")
os.replace(tmp, out)
print(f"wrote {out} ({len(rows)} targets)")
PY
}

if [ "$INTERVAL" -gt 0 ] 2>/dev/null; then
  while true; do
    sync_once || true
    sleep "$INTERVAL"
  done
fi

sync_once
