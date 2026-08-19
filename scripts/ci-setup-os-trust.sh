#!/usr/bin/env bash
# Install OS A/B signing keys for guest-release / os-bundle.
#
# Production (stable upgrades across tags): set GitHub Actions secrets
#   OS_TRUST_SK  — hex from out/secrets/os-trust.sk  (make os-trust)
#   OS_TRUST_PK  — hex from out/secrets/os-trust.pk
#
# If both secrets are unset, signed OS bundles are skipped (qcow2 still builds).
# Never print the secret key.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="${ROOT}/out/secrets"
SK="${DEST}/os-trust.sk"
PK="${DEST}/os-trust.pk"
STAMP="${DEST}/.os-bundle-ready"

mkdir -p "$DEST"
umask 077

trim() {
  local s="${1:-}"
  s="${s#"${s%%[![:space:]]*}"}"
  s="${s%"${s##*[![:space:]]}"}"
  printf '%s' "$s"
}

valid_hex64() {
  [[ "${#1}" -eq 64 && "$1" =~ ^[0-9a-fA-F]+$ ]]
}

mark_ready() {
  echo ready >"$STAMP"
  if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    echo "os_bundle=true" >>"$GITHUB_OUTPUT"
  fi
}

skip_bundles() {
  rm -f "$STAMP"
  echo "::warning::OS_TRUST_SK / OS_TRUST_PK not set — skipping signed OS bundles. \
Pin keys from \`make os-trust\` as repo secrets so A/B upgrades stay verifiable across releases."
  if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    echo "os_bundle=false" >>"$GITHUB_OUTPUT"
  fi
  exit 0
}

SK_VAL="$(trim "${OS_TRUST_SK:-}")"
PK_VAL="$(trim "${OS_TRUST_PK:-}")"

if [[ -n "$SK_VAL" && -n "$PK_VAL" ]]; then
  if ! valid_hex64 "$SK_VAL"; then
    echo "::error::OS_TRUST_SK must be 64 hex chars (make os-trust → os-trust.sk)" >&2
    exit 1
  fi
  if ! valid_hex64 "$PK_VAL"; then
    echo "::error::OS_TRUST_PK must be 64 hex chars (make os-trust → os-trust.pk)" >&2
    exit 1
  fi
  if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
    echo "::add-mask::${SK_VAL}"
  fi
  printf '%s\n' "$SK_VAL" >"$SK"
  printf '%s\n' "$PK_VAL" >"$PK"
  chmod 600 "$SK" "$PK"
  echo "os-trust keys from secrets → ${DEST}"
  mark_ready
  exit 0
fi

if [[ -n "$SK_VAL" || -n "$PK_VAL" ]]; then
  echo "::error::set both OS_TRUST_SK and OS_TRUST_PK (hex), or neither" >&2
  exit 1
fi

# Local: reuse make os-trust output. CI must use secrets only (self-hosted
# runners can have leftover keys from a previous job).
if [[ -z "${GITHUB_ACTIONS:-}" && -f "$SK" && -f "$PK" ]]; then
  SK_FILE="$(trim "$(cat "$SK")")"
  PK_FILE="$(trim "$(cat "$PK")")"
  if valid_hex64 "$SK_FILE" && valid_hex64 "$PK_FILE"; then
    echo "os-trust keys from ${DEST} (existing files)"
    mark_ready
    exit 0
  fi
fi

skip_bundles
