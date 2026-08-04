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

Jobs shell `MGMT_LAB_UP` (default `./scripts/proxmox-lab-up.sh`) with provider credentials.

While create runs, the **Nodes** tab lists planned CP/worker rows as `provisioning` immediately; status (and IP when known) updates from lab-up logs as Proxmox VMs are created and nodes join. Failed creates mark unfinished nodes as `error`.

## Docker

```bash
docker build -f crates/pertisk-mgmt/Dockerfile -t pertisk-mgmt .
docker run -p 8080:8080 -e MGMT_ADMIN_PASSWORD=admin -e MGMT_SECRET_KEY=dev \
  -v pertisk-mgmt-data:/data pertisk-mgmt
```

## RPM (linux/amd64)

Package the management API + embedded UI for RHEL/Rocky/Alma (built via Docker for `linux/amd64`):

```bash
make mgmt-rpm          # or: make rpm
# → out/rpm/pertisk-mgmt-<version>-1.x86_64.rpm

sudo rpm -Uvh out/rpm/pertisk-mgmt-*.rpm
sudo systemctl enable --now pertisk-mgmt
# edit /etc/pertisk-mgmt/pertisk-mgmt.env (set MGMT_SECRET_KEY), then:
sudo systemctl restart pertisk-mgmt
# open http://<host>:8080
```

Installs `/usr/bin/pertisk-mgmt`, `/usr/bin/pertiskctl`, systemd unit, and data dir `/var/lib/pertisk-mgmt`. Requires Docker on the build host; `kubectl` is recommended on the target for node sync / top.

## RBAC

| Role | Capabilities |
|------|----------------|
| `viewer` | Read clusters/providers |
| `operator` | Create/update/delete clusters, providers, nodes, upgrades |
| `admin` | Operator + delete providers |
