#!/usr/bin/env bash
# Write compose/file_sd/nodes.yml from pertisk-mgmt SQLite (ready nodes with IPv4).
# Run on the mgmt / compose host. Prometheus file_sd reloads ~30s (or restart prometheus).
#
#   ./sync-file-sd.sh
#   MGMT_DB=/var/lib/pertisk-mgmt/mgmt.db ./sync-file-sd.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
OUT="${ROOT}/compose/file_sd/nodes.yml"
DB="${MGMT_DB:-/var/lib/pertisk-mgmt/mgmt.db}"

[[ -f "$DB" ]] || { echo "mgmt db not found: $DB" >&2; exit 1; }

python3 - "$DB" "$OUT" <<'PY'
import sqlite3, sys
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
lines = ["# generated from pertisk-mgmt nodes — re-run sync-file-sd.sh after scale"]
for cluster, name, role, ip in rows:
    lines += [
        "- targets:",
        f"    - {ip}:50001",
        "  labels:",
        f"    cluster: {cluster}",
        f"    role: {role}",
        f"    hostname: {name}",
    ]
if not rows:
    lines.append("[]")
open(out, "w").write("\n".join(lines) + "\n")
print(f"wrote {out} ({len(rows)} targets)")
PY
