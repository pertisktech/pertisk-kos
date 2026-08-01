#!/usr/bin/env bash
# Generate mTLS materials for pertiskd + pertiskctl (dev only).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${1:-${ROOT}/out/mtls}"
mkdir -p "${OUT}"

if ! command -v openssl >/dev/null 2>&1; then
  echo "openssl required" >&2
  exit 1
fi

echo "==> writing certs to ${OUT}"

# CA
openssl req -x509 -newkey rsa:2048 -nodes -keyout "${OUT}/ca.key" -out "${OUT}/ca.crt" \
  -days 3650 -subj "/CN=Pertisk Dev CA" >/dev/null 2>&1

# Server
openssl req -newkey rsa:2048 -nodes -keyout "${OUT}/server.key" -out "${OUT}/server.csr" \
  -subj "/CN=pertiskd" >/dev/null 2>&1
cat > "${OUT}/server.ext" <<EOF
subjectAltName = DNS:pertiskd,IP:127.0.0.1,IP:0.0.0.0
extendedKeyUsage = serverAuth
EOF
openssl x509 -req -in "${OUT}/server.csr" -CA "${OUT}/ca.crt" -CAkey "${OUT}/ca.key" \
  -CAcreateserial -out "${OUT}/server.crt" -days 825 -extfile "${OUT}/server.ext" >/dev/null 2>&1

# Client
openssl req -newkey rsa:2048 -nodes -keyout "${OUT}/client.key" -out "${OUT}/client.csr" \
  -subj "/CN=pertiskctl" >/dev/null 2>&1
cat > "${OUT}/client.ext" <<EOF
extendedKeyUsage = clientAuth
EOF
openssl x509 -req -in "${OUT}/client.csr" -CA "${OUT}/ca.crt" -CAkey "${OUT}/ca.key" \
  -CAcreateserial -out "${OUT}/client.crt" -days 825 -extfile "${OUT}/client.ext" >/dev/null 2>&1

rm -f "${OUT}/server.csr" "${OUT}/client.csr" "${OUT}/server.ext" "${OUT}/client.ext" "${OUT}/ca.srl"

echo "Server: --tls-ca ${OUT}/ca.crt --tls-cert ${OUT}/server.crt --tls-key ${OUT}/server.key"
echo "Client: --ca ${OUT}/ca.crt --cert ${OUT}/client.crt --key ${OUT}/client.key"
