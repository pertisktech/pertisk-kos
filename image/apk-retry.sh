#!/bin/sh
# Retry `apk add` across Alpine CDN + fallback mirrors.
# Usage (inside alpine:* container): sh /apk-retry.sh pkg [pkg...]
set -eu

if [ "$#" -lt 1 ]; then
  echo "usage: apk-retry.sh pkg [pkg...]" >&2
  exit 1
fi

want="$*"

. /etc/os-release
ver=$(echo "$VERSION_ID" | cut -d. -f1,2)

# First entry is Alpine's default CDN. Others are used when it 5xx/times out.
set -- \
  "https://dl-cdn.alpinelinux.org/alpine" \
  "https://mirrors.edge.kernel.org/alpine" \
  "https://uk.alpinelinux.org/alpine" \
  "https://mirror.csclub.uwaterloo.ca/alpine" \
  "https://dl-4.alpinelinux.org/alpine"

n=0
max=12
mirrors=$#
while [ "$n" -lt "$max" ]; do
  n=$((n + 1))
  idx=$(( (n - 1) % mirrors + 1 ))
  eval "base=\${$idx}"
  printf '%s\n' \
    "${base}/v${ver}/main" \
    "${base}/v${ver}/community" \
    >/etc/apk/repositories
  echo "==> apk add (attempt ${n}/${max}) alpine=${ver} mirror=${base}" >&2
  # shellcheck disable=SC2086
  if apk add --no-cache ${want}; then
    exit 0
  fi
  if [ "$n" -lt 6 ]; then
    sleep=$((n * 4))
  else
    sleep=24
  fi
  echo "==> apk add failed; retry in ${sleep}s" >&2
  sleep "$sleep"
done

echo "apk add failed after ${max} attempts: ${want}" >&2
exit 1
