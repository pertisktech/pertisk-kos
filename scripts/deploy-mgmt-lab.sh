#!/usr/bin/env bash
# Mgmt-only deploy (Omni-style Proxmox API — no scp to the PVE node):
#   1) local build cloud images (+ optional RPM)
#   2) install RPM on mgmt host
#   3) copy qcow2 → mgmt /var/lib/pertisk-mgmt/images/
#   4) Create cluster uploads disk via Proxmox API (provider token)
#
# Examples:
#   ./scripts/deploy-mgmt-lab.sh --mgmt user@mgmt.example.com
#   ./scripts/deploy-mgmt-lab.sh --mgmt user@mgmt.example.com --skip-build --skip-rpm
#   ./scripts/deploy-mgmt-lab.sh --mgmt user@mgmt.example.com --with-ssh --pve pve.example.com
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ARCH="${PERTISK_ARCH:-${ARCH:-amd64}}"
MGMT_HOST="${MGMT_HOST:-}"
PVE_HOST="${PVE_HOST:-}"
VERSION="${VERSION:-}"
SKIP_BUILD=0
SKIP_RPM=0
SKIP_IMAGES=0
WITH_SSH=0
CP_GB="${CP_GB:-50}"
WORKER_GB="${WORKER_GB:-75}"
LAB_SUBNET="${LAB_SUBNET:-10.1.1.0/24}"

usage() {
  sed -n '2,12p' "$0"
  cat <<EOF

Flags:
  --mgmt USER@HOST     mgmt SSH target (required; or env MGMT_HOST)
  --version VER        RPM version for make rpm (default: from make version)
  --skip-build         reuse existing out/pertisk-cloud-*.qcow2
  --skip-rpm           do not build/install RPM
  --skip-images        do not stage/copy qcow2
  --with-ssh           also configure PROXMOX_SSH + keys (optional)
  --pve HOST|root@HOST required with --with-ssh
  --subnet CIDR        LAB_SUBNET for MAC→IP without SSH (default ${LAB_SUBNET})
  --cp-gb N            (default ${CP_GB})
  --worker-gb N        (default ${WORKER_GB})
  --arch ARCH          amd64|arm64 (default ${ARCH}; env ARCH/PERTISK_ARCH)
  -h, --help

Env:
  MGMT_PUBLIC_URL      force Public URL on deploy; otherwise keep existing
                       /etc/pertisk-mgmt/pertisk-mgmt.env (or default http://<mgmt-ip>:8080)
EOF
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --mgmt) MGMT_HOST="$2"; shift 2 ;;
    --pve) PVE_HOST="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    --subnet) LAB_SUBNET="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --skip-rpm) SKIP_RPM=1; shift ;;
    --skip-images) SKIP_IMAGES=1; shift ;;
    --with-ssh) WITH_SSH=1; shift ;;
    --cp-gb) CP_GB="$2"; shift 2 ;;
    --worker-gb) WORKER_GB="$2"; shift 2 ;;
    --arch) ARCH="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "unknown arg: $1" >&2; usage ;;
  esac
done

case "$(printf '%s' "$ARCH" | tr '[:upper:]' '[:lower:]')" in
  amd64|x86_64|x64) ARCH=amd64 ;;
  arm64|aarch64) ARCH=arm64 ;;
  *) echo "unsupported --arch=${ARCH} (use amd64|arm64)" >&2; exit 1 ;;
esac
export PERTISK_ARCH="$ARCH" ARCH="$ARCH"

[[ -n "$MGMT_HOST" ]] || { echo "ERROR: set --mgmt USER@HOST" >&2; exit 1; }
if [[ "$WITH_SSH" == "1" && -z "$PVE_HOST" ]]; then
  echo "ERROR: --with-ssh requires --pve HOST" >&2
  exit 1
fi

if [[ -z "$VERSION" ]]; then
  VERSION="$(make -C "$ROOT" -s version 2>/dev/null || echo 0.1.0)"
fi

PVE_SSH=""
if [[ -n "$PVE_HOST" ]]; then
  if [[ "$PVE_HOST" == *@* ]]; then
    PVE_SSH="$PVE_HOST"
  else
    PVE_SSH="root@${PVE_HOST}"
  fi
fi

echo "==> pipeline: local build → RPM @ ${MGMT_HOST} → images on mgmt → create via Proxmox API"

# --- 1) images ---
if [[ "$SKIP_IMAGES" != "1" ]]; then
  STAGE_ARGS=(--arch "$ARCH" --cp-gb "$CP_GB" --worker-gb "$WORKER_GB" --dest "${ROOT}/out")
  [[ "$SKIP_BUILD" == "1" ]] && STAGE_ARGS+=(--skip-build)
  echo "==> [1/3] stage cloud images"
  "$ROOT/scripts/stage-cloud-images.sh" "${STAGE_ARGS[@]}"
else
  echo "==> [1/3] skip images"
fi

# --- 2) RPM ---
if [[ "$SKIP_RPM" != "1" ]]; then
  echo "==> [2/3] build + install RPM VERSION=${VERSION}"
  make -C "$ROOT" rpm VERSION="$VERSION"
  RPM="$(ls -1t "${ROOT}/out/rpm/pertisk-mgmt-${VERSION}"-*.rpm 2>/dev/null | head -1 || true)"
  [[ -n "$RPM" && -f "$RPM" ]] || RPM="$(ls -1t "${ROOT}/out/rpm/pertisk-mgmt-"*.rpm 2>/dev/null | head -1 || true)"
  [[ -n "$RPM" && -f "$RPM" ]] || { echo "ERROR: no RPM in out/rpm/" >&2; exit 1; }
  scp "$RPM" "${MGMT_HOST}:/tmp/pertisk-mgmt.rpm"
  # Same NEVRA is already installed after a prior deploy (e.g. amd64 then arm64-only
  # with the same VERSION) — rpm -Uvh alone exits non-zero; --replacepkgs refreshes.
  ssh "$MGMT_HOST" 'sudo bash -c "
    set -euo pipefail
    if rpm -q pertisk-mgmt >/dev/null 2>&1; then
      have=\$(rpm -q --qf \"%{NAME}-%{VERSION}-%{RELEASE}.%{ARCH}\" pertisk-mgmt)
      want=\$(rpm -qp --qf \"%{NAME}-%{VERSION}-%{RELEASE}.%{ARCH}\" /tmp/pertisk-mgmt.rpm)
      if [[ \"\$have\" == \"\$want\" ]]; then
        echo \"RPM \$want already installed — reinstall (--replacepkgs)\"
        rpm -Uvh --replacepkgs /tmp/pertisk-mgmt.rpm
      else
        rpm -Uvh /tmp/pertisk-mgmt.rpm
      fi
    else
      rpm -Uvh /tmp/pertisk-mgmt.rpm
    fi
    systemctl enable pertisk-mgmt
  "'
else
  echo "==> [2/3] skip RPM"
fi

# --- 3) copy images → mgmt only ---
if [[ "$SKIP_IMAGES" != "1" ]]; then
  echo "==> [3/3] copy qcow2 → ${MGMT_HOST}:/var/lib/pertisk-mgmt/images/"
  scp "${ROOT}/out/pertisk-cloud-${ARCH}.qcow2" \
      "${ROOT}/out/pertisk-cloud-${ARCH}-${CP_GB}g.qcow2" \
      "${ROOT}/out/pertisk-cloud-${ARCH}-${WORKER_GB}g.qcow2" \
      "${MGMT_HOST}:/tmp/"
  ssh "$MGMT_HOST" "sudo bash -c '
    mkdir -p /var/lib/pertisk-mgmt/images
    mv /tmp/pertisk-cloud-${ARCH}*.qcow2 /var/lib/pertisk-mgmt/images/
    chown -R pertisk-mgmt:pertisk-mgmt /var/lib/pertisk-mgmt/images
    ls -lh /var/lib/pertisk-mgmt/images
  '"
fi

# --- env: API-only by default (like Omni infra provider) ---
echo "==> configure API disk import (PROXMOX_NO_SSH=1, upload→local)"
# Derive a guest-reachable Public URL from --mgmt host (avoid http://0.0.0.0:8080).
# Do not overwrite a customized MGMT_PUBLIC_URL on every RPM deploy — only set when
# the caller exports MGMT_PUBLIC_URL, or the env file has no value yet.
MGMT_IP="${MGMT_HOST##*@}"
MGMT_IP="${MGMT_IP%%:*}"
DEFAULT_PUBLIC_URL="http://${MGMT_IP}:8080"
if [[ -n "${MGMT_PUBLIC_URL:-}" ]]; then
  PUBLIC_URL_MODE=explicit
else
  PUBLIC_URL_MODE=preserve
  MGMT_PUBLIC_URL="$DEFAULT_PUBLIC_URL"
fi
# Pipe via bash -s so the remote login shell (often zsh + nomatch) never
# parses the script. Nested `bash -c '… sed -i '/pattern/' …'` breaks quotes.
ssh "$MGMT_HOST" "sudo bash -s" <<EOF
set -euo pipefail
ENV=/etc/pertisk-mgmt/pertisk-mgmt.env
touch "\$ENV"
set_kv() {
  local k="\$1" v="\$2"
  if grep -q "^\${k}=" "\$ENV" 2>/dev/null; then
    sed -i "s|^\${k}=.*|\${k}=\${v}|" "\$ENV"
  elif grep -q "^# *\${k}=" "\$ENV" 2>/dev/null; then
    sed -i "s|^# *\${k}=.*|\${k}=\${v}|" "\$ENV"
  else
    echo "\${k}=\${v}" >> "\$ENV"
  fi
}
set_kv PERTISK_IMAGES_DIR /var/lib/pertisk-mgmt/images
set_kv PROXMOX_NO_SSH 1
set_kv PROXMOX_UPLOAD_STORAGE local
set_kv LAB_SUBNET ${LAB_SUBNET}
existing_public="\$(grep -E "^MGMT_PUBLIC_URL=" "\$ENV" 2>/dev/null | head -1 | cut -d= -f2- || true)"
if [[ "${PUBLIC_URL_MODE}" == "explicit" ]]; then
  set_kv MGMT_PUBLIC_URL ${MGMT_PUBLIC_URL}
  echo "MGMT_PUBLIC_URL=${MGMT_PUBLIC_URL} (from deploy env)"
elif [[ -n "\$existing_public" ]]; then
  echo "MGMT_PUBLIC_URL=\${existing_public} (preserved)"
else
  set_kv MGMT_PUBLIC_URL ${MGMT_PUBLIC_URL}
  echo "MGMT_PUBLIC_URL=${MGMT_PUBLIC_URL} (default; was unset)"
fi
# Drop duplicate / stale PROXMOX_SSH lines (deploy used to comment+append forever).
sed -i '/^[[:space:]]*#*[[:space:]]*PROXMOX_SSH=/d' "\$ENV" 2>/dev/null || true
if [[ "${WITH_SSH}" == "1" ]]; then
  : # set below after PROXMOX_NO_SSH flip
else
  echo "# PROXMOX_SSH=root@pve" >> "\$ENV"
fi
EOF
# Reflect what landed on the host for the summary line below.
MGMT_PUBLIC_URL="$(ssh "$MGMT_HOST" "sudo grep -E '^MGMT_PUBLIC_URL=' /etc/pertisk-mgmt/pertisk-mgmt.env 2>/dev/null | head -1 | cut -d= -f2-" || true)"
MGMT_PUBLIC_URL="${MGMT_PUBLIC_URL:-$DEFAULT_PUBLIC_URL}"
echo "==> MGMT_PUBLIC_URL=${MGMT_PUBLIC_URL}"

if [[ "$WITH_SSH" == "1" && -n "$PVE_SSH" ]]; then
  echo "==> optional SSH mode PROXMOX_SSH=${PVE_SSH}"
  ssh "$MGMT_HOST" "sudo bash -s" <<EOF
set -euo pipefail
ENV=/etc/pertisk-mgmt/pertisk-mgmt.env
set_kv() {
  local k="\$1" v="\$2"
  if grep -q "^\${k}=" "\$ENV" 2>/dev/null; then
    sed -i "s|^\${k}=.*|\${k}=\${v}|" "\$ENV"
  else
    echo "\${k}=\${v}" >> "\$ENV"
  fi
}
set_kv PROXMOX_NO_SSH 0
sed -i '/^[[:space:]]*#*[[:space:]]*PROXMOX_SSH=/d' "\$ENV" 2>/dev/null || true
echo "PROXMOX_SSH=${PVE_SSH}" >> "\$ENV"
EOF
  ssh "$MGMT_HOST" 'sudo -u pertisk-mgmt -H bash -c "
    mkdir -p ~/.ssh && chmod 700 ~/.ssh
    [[ -f ~/.ssh/id_ed25519 ]] || ssh-keygen -t ed25519 -N \"\" -f ~/.ssh/id_ed25519 -C pertisk-mgmt@mgmt
  "'
  PUB="$(ssh "$MGMT_HOST" 'sudo -u pertisk-mgmt -H cat /var/lib/pertisk-mgmt/.ssh/id_ed25519.pub')"
  if ssh -o BatchMode=yes -o ConnectTimeout=5 "$PVE_SSH" true 2>/dev/null; then
    ssh "$PVE_SSH" "mkdir -p /root/.ssh && chmod 700 /root/.ssh
      grep -qxF '$PUB' /root/.ssh/authorized_keys 2>/dev/null || echo '$PUB' >> /root/.ssh/authorized_keys
      chmod 600 /root/.ssh/authorized_keys"
  else
    echo "WARNING: cannot SSH to ${PVE_SSH} — install pubkey manually: $PUB" >&2
  fi
fi

ssh "$MGMT_HOST" 'sudo systemctl restart pertisk-mgmt && sudo systemctl --no-pager -l status pertisk-mgmt | head -20' || true

# kubectl + helm are required for lab-up CNI (cilium default uses helm).
# Install under /usr/bin — RHEL sudo secure_path omits /usr/local/bin.
echo "==> ensure kubectl + helm on mgmt"
ssh "$MGMT_HOST" 'sudo bash -c "
  set -euo pipefail
  export PATH=\"/usr/bin:/usr/local/bin:\$PATH\"
  arch=\$(uname -m)
  case \"\$arch\" in
    x86_64|amd64) karch=amd64 ;;
    aarch64|arm64) karch=arm64 ;;
    *) echo \"unsupported arch: \$arch\" >&2; exit 1 ;;
  esac
  if ! command -v kubectl >/dev/null 2>&1; then
    echo \"installing kubectl…\"
    ver=\$(curl -fsSL https://dl.k8s.io/release/stable.txt)
    curl -fsSLo /usr/bin/kubectl \"https://dl.k8s.io/release/\${ver}/bin/linux/\${karch}/kubectl\"
    chmod 755 /usr/bin/kubectl
  fi
  kubectl version --client 2>/dev/null | head -n 2
  if ! command -v helm >/dev/null 2>&1; then
    echo \"installing helm…\"
    curl -fsSL https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash
    # get-helm-3 defaults to /usr/local/bin; link into sudo PATH on RHEL.
    if [[ -x /usr/local/bin/helm && ! -e /usr/bin/helm ]]; then
      ln -sf /usr/local/bin/helm /usr/bin/helm
    fi
  fi
  helm version
  # CNI templates (flannel/calico kube-proxy) — present after RPM; repair if missing.
  if [[ ! -f /usr/share/pertisk-mgmt/examples/cni/kube-proxy.yaml ]]; then
    echo \"WARNING: /usr/share/pertisk-mgmt/examples/cni missing — redeploy mgmt RPM\" >&2
  else
    ls /usr/share/pertisk-mgmt/examples/cni/
  fi
"'

cat <<EOF

======== deploy done ========
mgmt:     ${MGMT_HOST}
public:   ${MGMT_PUBLIC_URL}
images:   /var/lib/pertisk-mgmt/images/pertisk-cloud-${ARCH}*.qcow2
disk:     Proxmox API upload → local → import-from → provider storage
          (no scp to PVE; like Omni infra provider)
$([ "$WITH_SSH" == "1" ] && echo "ssh:      PROXMOX_SSH=${PVE_SSH} (arm64 qm create / arch=aarch64)" || true)

Next:
  1. UI → Providers → add Proxmox (URL / API token / node / storage / guest arch)
     Ensure storage "local" allows content type Import (Datacenter → Storage).
  2. For arm64: verify  sudo -u pertisk-mgmt -H ssh -o BatchMode=yes root@<pve> true
  3. Clusters → Create (CNI=cilium needs helm on mgmt — installed above)
  4. Job log: arch=arm64 + qm create via SSH (or API upload for amd64)

EOF
