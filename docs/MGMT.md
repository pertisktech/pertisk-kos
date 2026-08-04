# Pertisk Management UI

Single-port control plane: **Rust API** (`pertisk-mgmt`) + **React UI** (Adminator-inspired shell).

## Quick start

```bash
# Seed admin (optional; defaults admin/admin)
export MGMT_ADMIN_USER=admin
export MGMT_ADMIN_PASSWORD=admin
#export MGMT_SECRET_KEY=$(openssl rand -hex 32)
export MGMT_SECRET_KEY=dev-stable-key
make mgmt
./out/bin/pertisk-mgmt --listen 0.0.0.0:8080 --db ./data/mgmt.db
# open http://127.0.0.1:8080
```

Dev (UI hot reload + API):

```bash
# terminal 1
MGMT_ADMIN_PASSWORD=admin cargo run -p pertisk-mgmt -- --listen 127.0.0.1:8080

# terminal 2
cd web/mgmt-ui && npm run dev   # http://127.0.0.1:5173 proxies /api → :8080
```

## Auth

| Env | Description |
|-----|-------------|
| `AUTH_MODE` | `local` (default), `auth0`, or `both` |
| `MGMT_ADMIN_USER` / `MGMT_ADMIN_PASSWORD` | Seeded local admin |
| `MGMT_SECRET_KEY` | JWT + AES key (hex 64 chars or any string) |
| `AUTH0_DOMAIN` / `AUTH0_CLIENT_ID` / `AUTH0_CLIENT_SECRET` | SSO |
| `MGMT_PUBLIC_URL` | Public base URL for OIDC callback |
| `MGMT_METRICS_TOKEN` | Optional Bearer when scraping guest `:50001/metrics` |
| `MGMT_PERTISKCTL` | Path to `pertiskctl` (default `./out/bin/pertiskctl`) |

Auth0 role claim: `https://pertisk.io/role` or `role` → `admin` \| `operator` \| `viewer`.

## Node detail

Cluster → Nodes → click a node name → `/clusters/:id/nodes/:nid`.

The page shows inventory (VMID, IPs, K8s, hardware), live Machine Health, and charts:

| Source | How mgmt collects it |
|--------|----------------------|
| Health | `pertiskctl -e {ip}:50000 health` |
| Gauges + API metrics | HTTP scrape `http://{ip}:50001/metrics` (new series: `pertisk_api_requests_total`, duration sum/count) |
| CPU / memory % | `kubectl top node` via cluster kubeconfig (needs metrics-server) |

Charts poll every ~4s and keep ~60 samples **in the browser** only — refresh clears history. Soft errors (unreachable guest, missing metrics-server) show under each section without failing the page.

A **Logs** panel at the bottom tails `pertiskd` / `containerd` / `kubelet` / `dmesg` via `pertiskctl logs` (`GET /api/clusters/:id/nodes/:nid/logs`).

## Proxmox provider

UI → Providers → add URL, API token, node, storage (same fields as [PROXMOX.md](./PROXMOX.md)).

Secrets are encrypted at rest with `MGMT_SECRET_KEY`. For lab self-signed TLS, set **Insecure TLS = Yes** on the provider (same as `PROXMOX_INSECURE=1`).

## Clusters (HA)

Create with **M control planes** + **N workers**. When `M > 1`, **VIP** is required (kube-vip), matching:

```bash
./scripts/proxmox-lab-up.sh --controlplanes 3 --vip 10.1.1.250 --workers 2 --cni cilium
```

Use a **free** L2 IPv4 for `--vip` (not already answering ping). Guest images need the `af_packet` module for kube-vip ARP (`make fetch-kernel` + rebuild cloud image).

Jobs shell `MGMT_LAB_UP` (default `./scripts/proxmox-lab-up.sh` locally, or `/usr/share/pertisk-mgmt/scripts/proxmox-lab-up.sh` in the RPM) with provider credentials.

If `MGMT_LAB_UP` is missing, create fails unless `MGMT_ALLOW_LAB_STUB=1` (UI-only stub). RPM installs pack scripts + examples under `/usr/share/pertisk-mgmt/`; place cloud qcow2 under `/var/lib/pertisk-mgmt/images/` (or set `PROXMOX_DISK`) because create uses `--skip-build`.

While create runs, the **Nodes** tab lists planned CP/worker rows as `provisioning` immediately; status (and IP when known) updates from lab-up logs as Proxmox VMs are created and nodes join. Failed creates mark unfinished nodes as `error`.

## Docker

```bash
docker build -f crates/pertisk-mgmt/Dockerfile -t pertisk-mgmt .
docker run -p 8080:8080 -e MGMT_ADMIN_PASSWORD=admin -e MGMT_SECRET_KEY=dev \
  -v pertisk-mgmt-data:/data pertisk-mgmt
```

## RPM deploy (linux/amd64)

Package the management API + embedded UI for RHEL/Rocky/Alma (Docker build for `linux/amd64`). Installs `/usr/bin/pertisk-mgmt`, `/usr/bin/pertiskctl`, scripts under `/usr/share/pertisk-mgmt/`, systemd unit (`User=pertisk-mgmt`), and data dir `/var/lib/pertisk-mgmt`. Requires Docker on the **build** host; `kubectl` is recommended on the **target** for node sync / top.

### 1. Build

```bash
make rpm VERSION=0.1.3          # or: make mgmt-rpm
# → out/rpm/pertisk-mgmt-<version>-1.x86_64.rpm
```

### 2. Install on the mgmt host

Example target: AlmaLinux at `almalinux@10.1.1.12`.

```bash
MGMT_HOST=almalinux@10.1.1.12
RPM=out/rpm/pertisk-mgmt-0.1.3-1.x86_64.rpm

scp "$RPM" "${MGMT_HOST}:/tmp/"
ssh "$MGMT_HOST" 'sudo rpm -Uvh /tmp/pertisk-mgmt-*-1.x86_64.rpm'
ssh "$MGMT_HOST" 'sudo systemctl enable --now pertisk-mgmt'
```

`rpm -Uvh` may write `/etc/pertisk-mgmt/pertisk-mgmt.env.rpmnew` when the env file already exists — keep your edited env; merge new keys from `.rpmnew` if needed.

### 3. Configure env

On the mgmt host, edit `/etc/pertisk-mgmt/pertisk-mgmt.env`:

```bash
sudoedit /etc/pertisk-mgmt/pertisk-mgmt.env
# set a stable MGMT_SECRET_KEY (changing it invalidates encrypted provider secrets)
# MGMT_LAB_UP=/usr/share/pertisk-mgmt/scripts/proxmox-lab-up.sh   # RPM default
# optional: PERTISK_IMAGES_DIR=/var/lib/pertisk-mgmt/images
# optional: PROXMOX_SSH=root@10.1.1.197   # else auto-derived from provider URL IP
sudo systemctl restart pertisk-mgmt
# open http://<mgmt-host>:8080
```

### 4. Cloud images (required for Create Cluster)

Create jobs always pass `--skip-build`. Lab-up looks under `/var/lib/pertisk-mgmt/images` (`PERTISK_IMAGES_DIR` / `MGMT_IMAGES_DIR`), then falls back to a base qcow2 if role-sized files are missing.

```bash
# On the build machine (after make cloud / lab-up once):
MGMT_HOST=almalinux@10.1.1.12
scp out/pertisk-cloud-amd64.qcow2 \
    out/pertisk-cloud-amd64-50g.qcow2 \
    out/pertisk-cloud-amd64-75g.qcow2 \
    "${MGMT_HOST}:/tmp/"

ssh "$MGMT_HOST" 'sudo bash -c "
  mkdir -p /var/lib/pertisk-mgmt/images
  mv /tmp/pertisk-cloud-amd64*.qcow2 /var/lib/pertisk-mgmt/images/
  chown -R pertisk-mgmt:pertisk-mgmt /var/lib/pertisk-mgmt/images
  ls -lh /var/lib/pertisk-mgmt/images
"'
```

Prefer role-sized images matching UI disk sizes (`*-50g.qcow2`, `*-75g.qcow2`). Base `pertisk-cloud-amd64.qcow2` is the fallback.

### 5. SSH from `pertisk-mgmt` → Proxmox (required for disk import)

Lab-up uses `PROXMOX_SSH` (`scp` + `qm importdisk`, MAC→ARP). The service runs as **`pertisk-mgmt`**, not your login user. Auto-sets `PROXMOX_SSH=root@<ip>` when the provider URL host is an IP.

```bash
MGMT_HOST=almalinux@10.1.1.12
PVE=root@10.1.1.197   # match your Proxmox provider URL

# Generate a key for the service user (once)
ssh "$MGMT_HOST" 'sudo -u pertisk-mgmt -H bash -c "
  mkdir -p ~/.ssh && chmod 700 ~/.ssh
  [[ -f ~/.ssh/id_ed25519 ]] || ssh-keygen -t ed25519 -N \"\" -f ~/.ssh/id_ed25519 -C pertisk-mgmt@mgmt
  cat ~/.ssh/id_ed25519.pub
"'

# Install that pubkey on PVE (from a host that already has root SSH to PVE)
PUB=$(ssh "$MGMT_HOST" 'sudo -u pertisk-mgmt -H cat /var/lib/pertisk-mgmt/.ssh/id_ed25519.pub')
ssh "$PVE" "grep -qxF '$PUB' /root/.ssh/authorized_keys || echo '$PUB' >> /root/.ssh/authorized_keys"

# Verify
ssh "$MGMT_HOST" 'sudo -u pertisk-mgmt -H ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new '"$PVE"' hostname'
```

Without this step, create fails at `SCP + qm importdisk` with `Connection closed` / `Permission denied`.

### 6. UI checklist

1. Providers → add Proxmox URL/token/node/storage (`Insecure TLS` if lab self-signed).
2. Clusters → Create (VIP required when control planes > 1).
3. Watch job logs + Nodes tab (`provisioning` → IPs as VMs come up).

## RBAC

| Role | Capabilities |
|------|----------------|
| `viewer` | Read clusters/providers |
| `operator` | Create/update/delete clusters, providers, nodes, upgrades |
| `admin` | Operator + delete providers |
