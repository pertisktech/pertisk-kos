#!/usr/bin/env bash
# Back-compat wrapper: amd64 RPM+DEB (lab deploy). Full matrix: scripts/build-mgmt-pkg.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PKG_PLATFORMS="${PKG_PLATFORMS:-linux/amd64}"
exec "${ROOT}/scripts/build-mgmt-pkg.sh"
