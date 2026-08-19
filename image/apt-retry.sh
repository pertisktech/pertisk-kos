#!/bin/sh
# Retry `apt-get install` when Debian/Ubuntu mirrors or DNS flake.
# Usage (inside debian:* or ubuntu:* container): sh /apt-retry.sh pkg [pkg...]
set -eu

if [ "$#" -lt 1 ]; then
  echo "usage: apt-retry.sh pkg [pkg...]" >&2
  exit 1
fi

want="$*"
export DEBIAN_FRONTEND=noninteractive

. /etc/os-release
id="${ID:-}"
codename="${VERSION_CODENAME:-}"

write_debian() {
  main="$1"
  sec="$2"
  cat >/etc/apt/sources.list <<EOF
deb ${main} ${codename} main
deb ${main} ${codename}-updates main
deb ${sec} ${codename}-security main
EOF
  # Newer images also ship DEB822 files; drop them so only this list is used.
  rm -f /etc/apt/sources.list.d/debian.sources \
    /etc/apt/sources.list.d/debian.list 2>/dev/null || true
}

n=0
max=12
while [ "$n" -lt "$max" ]; do
  n=$((n + 1))
  if [ "$id" = debian ] && [ -n "$codename" ]; then
    case $(( (n - 1) % 6 )) in
      0)
        write_debian http://deb.debian.org/debian http://deb.debian.org/debian-security
        ;;
      1)
        write_debian http://ftp.debian.org/debian http://security.debian.org/debian-security
        ;;
      2)
        write_debian http://cdn-aws.deb.debian.org/debian http://cdn-aws.deb.debian.org/debian-security
        ;;
      3)
        write_debian http://ftp.us.debian.org/debian http://security.debian.org/debian-security
        ;;
      4)
        write_debian http://ftp.de.debian.org/debian http://security.debian.org/debian-security
        ;;
      5)
        write_debian http://mirror.csclub.uwaterloo.ca/debian http://security.debian.org/debian-security
        ;;
    esac
  fi
  echo "==> apt install (attempt ${n}/${max}) os=${id} ${codename:-?} pkgs=${want}" >&2
  # apt-get update can exit 0 after failed fetches ("old ones used instead"),
  # so install failure is what drives the retry.
  if apt-get -o Acquire::Retries=3 update -qq \
    && apt-get -o Acquire::Retries=3 install -y -qq ${want}; then
    exit 0
  fi
  if [ "$n" -lt 6 ]; then
    sleep=$((n * 4))
  else
    sleep=24
  fi
  echo "==> apt install failed; retry in ${sleep}s" >&2
  sleep "$sleep"
done

echo "apt install failed after ${max} attempts: ${want}" >&2
exit 1
