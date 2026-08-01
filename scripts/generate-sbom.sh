#!/usr/bin/env bash
# Generate CycloneDX SBOMs for the Cargo workspace into out/sbom/.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out/sbom"
mkdir -p "${OUT}"

if [[ ! -f "${ROOT}/Cargo.lock" ]]; then
  (cd "${ROOT}" && cargo generate-lockfile)
fi

if ! command -v cargo-cyclonedx >/dev/null 2>&1; then
  echo "==> installing cargo-cyclonedx"
  cargo install cargo-cyclonedx --locked --version 0.5.7
fi

echo "==> CycloneDX JSON (per package)"
(
  cd "${ROOT}"
  cargo cyclonedx --format json --all
)

while IFS= read -r -d '' f; do
  cp "$f" "${OUT}/$(basename "$f")"
done < <(find "${ROOT}/crates" -name '*.cdx.json' -print0)

# Also emit a merged inventory via cargo metadata.
(cd "${ROOT}" && cargo metadata --format-version 1 > "${OUT}/cargo-metadata.json")

count="$(find "${OUT}" -name '*.cdx.json' | wc -l | tr -d ' ')"
if [[ "${count}" -eq 0 ]]; then
  echo "ERROR: no CycloneDX JSON produced" >&2
  exit 1
fi

echo "==> SBOM artifacts (${count} cdx files)"
ls -la "${OUT}"
