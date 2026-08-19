#!/usr/bin/env python3
"""Decrypt the Nutanix provider from mgmt.db and run nutanix-upload-vm.sh --repair-name.

Never prints the password. Must run as root on the mgmt host.
"""
from __future__ import annotations

import hashlib
import os
import sqlite3
import subprocess
import sys
from base64 import b64decode
from pathlib import Path

try:
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM
except ImportError:
    sys.stderr.write("install python3-cryptography\n")
    sys.exit(1)

ENV_FILE = Path("/etc/pertisk-mgmt/pertisk-mgmt.env")
DB = Path("/var/lib/pertisk-mgmt/mgmt.db")
UPLOAD = Path("/usr/share/pertisk-mgmt/scripts/nutanix-upload-vm.sh")
PREFIX = os.environ.get("REPAIR_PREFIX", "lab-ha-nutanix")


def load_env(path: Path) -> dict[str, str]:
    out: dict[str, str] = {}
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        out[k.strip()] = v.strip().strip('"').strip("'")
    return out


def derive_key(raw: str) -> bytes:
    if len(raw) == 64 and all(c in "0123456789abcdefABCDEF" for c in raw):
        try:
            return bytes.fromhex(raw)
        except ValueError:
            pass
    return hashlib.sha256(raw.encode()).digest()


def decrypt(key: bytes, encoded: str) -> str:
    raw = b64decode(encoded)
    nonce, ct = raw[:12], raw[12:]
    return AESGCM(key).decrypt(nonce, ct, None).decode()


def main() -> int:
    env = load_env(ENV_FILE)
    key = derive_key(env["MGMT_SECRET_KEY"])
    conn = sqlite3.connect(str(DB))
    row = conn.execute(
        "SELECT url, token_id, token_secret_enc, storage, bridge, insecure "
        "FROM providers WHERE kind='nutanix' LIMIT 1"
    ).fetchone()
    if not row:
        sys.stderr.write("no nutanix provider in mgmt.db\n")
        return 1
    url, user, enc, storage, network, insecure = row
    password = decrypt(key, enc)
    os.environ["NUTANIX_URL"] = url
    os.environ["NUTANIX_USER"] = user
    os.environ["NUTANIX_PASSWORD"] = password
    os.environ["NUTANIX_STORAGE"] = storage
    os.environ["NUTANIX_NETWORK"] = network
    os.environ["NUTANIX_INSECURE"] = "1" if insecure else os.environ.get("NUTANIX_INSECURE", "1")
    os.environ["LAB_GATEWAY"] = os.environ.get("LAB_GATEWAY") or env.get("LAB_GATEWAY") or "10.1.1.10"

    names = sys.argv[1:]
    if not names:
        import json
        import urllib.request
        import ssl

        ctx = ssl._create_unverified_context()
        req = urllib.request.Request(
            f"{url.rstrip('/')}/api/nutanix/v2.0/vms/",
            headers={"Accept": "application/json"},
        )
        import base64

        tok = base64.b64encode(f"{user}:{password}".encode()).decode()
        req.add_header("Authorization", f"Basic {tok}")
        with urllib.request.urlopen(req, context=ctx, timeout=30) as resp:
            data = json.load(resp)
        ents = data.get("entities") or []
        names = [e["name"] for e in ents if str(e.get("name") or "").startswith(PREFIX)]
    if not names:
        sys.stderr.write(f"no VMs matching prefix {PREFIX}\n")
        return 1
    rc = 0
    for name in names:
        print(f"==> repair netcfg {name}", file=sys.stderr)
        r = subprocess.run(
            ["bash", str(UPLOAD), "--repair-name", name],
            env=os.environ.copy(),
        )
        if r.returncode != 0:
            rc = r.returncode
            print(f"warn: repair {name} exited {r.returncode}", file=sys.stderr)
    return rc


if __name__ == "__main__":
    sys.exit(main())
