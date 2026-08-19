#!/usr/bin/env bash
# Create or update a GitHub Release and upload package + guest assets (self-hosted runners).
set -euo pipefail

: "${VERSION:?VERSION required}"
: "${TAG:?TAG required}"
: "${PACKAGES_DIR:?PACKAGES_DIR required}"
: "${GITHUB_TOKEN:?GITHUB_TOKEN required}"
NOTES_FILE="${NOTES_FILE:-}"

export GH_TOKEN="$GITHUB_TOKEN"

ensure_gh() {
  local dir="${HOME}/.local/bin"
  if [[ -x "${dir}/gh" ]]; then
    export PATH="${dir}:${PATH}"
    return 0
  fi
  if command -v gh >/dev/null 2>&1; then
    return 0
  fi

  local version="${GH_VERSION:-2.86.0}"
  mkdir -p "$dir"
  local arch
  case "$(uname -m)" in
    x86_64|amd64) arch=amd64 ;;
    aarch64|arm64) arch=arm64 ;;
    *)
      echo "::error::unsupported architecture for gh: $(uname -m)" >&2
      return 1
      ;;
  esac

  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN
  curl -fsSL "https://github.com/cli/cli/releases/download/v${version}/gh_${version}_linux_${arch}.tar.gz" \
    -o "${tmp}/gh.tgz"
  tar -xzf "${tmp}/gh.tgz" -C "$tmp"
  install -m 755 "${tmp}/gh_${version}_linux_${arch}/bin/gh" "${dir}/gh"
  export PATH="${dir}:${PATH}"
  "${dir}/gh" --version
}

ensure_gh
export PATH="${HOME}/.local/bin:${PATH}"

if ! command -v gh >/dev/null 2>&1; then
  echo "::error::GitHub CLI (gh) is required to publish the release" >&2
  exit 1
fi

if [[ -n "$NOTES_FILE" && ! -f "$NOTES_FILE" ]]; then
  echo "::error::Release notes file not found: $NOTES_FILE" >&2
  exit 1
fi

TITLE="Release ${VERSION}"
if gh release view "$TAG" >/dev/null 2>&1; then
  echo "Updating existing release ${TAG}"
  if [[ -n "$NOTES_FILE" ]]; then
    gh release edit "$TAG" --title "$TITLE" --notes-file "$NOTES_FILE"
  else
    gh release edit "$TAG" --title "$TITLE"
  fi
else
  echo "Creating release ${TAG}"
  if [[ -n "$NOTES_FILE" ]]; then
    gh release create "$TAG" --title "$TITLE" --notes-file "$NOTES_FILE"
  else
    gh release create "$TAG" --title "$TITLE" --generate-notes
  fi
fi

shopt -s nullglob
assets=()
while IFS= read -r -d '' f; do
  assets+=("$f")
done < <(find "$PACKAGES_DIR" \( \
  -name '*.rpm' -o -name '*.deb' -o -name 'SHA256SUMS.txt' -o -name 'pertiskctl-linux-*' \
  -o -name 'pertisk-cloud-*.qcow2' -o -name 'os-bundle-*.zip' -o -name 'os-trust.pk' \
\) -type f -print0 | sort -z)

if [[ "${#assets[@]}" -eq 0 ]]; then
  echo "::error::No release assets to upload under ${PACKAGES_DIR}" >&2
  exit 1
fi

echo "Uploading ${#assets[@]} asset(s) to ${TAG}"
gh release upload "$TAG" "${assets[@]}" --clobber

echo "=== Published release ${TAG} ==="
gh release view "$TAG" --json name,tagName,url --jq '"\(.name) \(.tagName) \(.url)"'
