# Pertisk Management UI

Single-port control plane: **Rust API** (`pertisk-mgmt`) + **React UI** (Adminator-inspired shell).

## Quick start

```bash
# Seed admin (optional; defaults admin/admin)
export MGMT_ADMIN_USER=admin
export MGMT_ADMIN_PASSWORD=admin
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
| `MGMT_METRICS_TLS_CA` / `MGMT_METRICS_TLS_CERT` / `MGMT_METRICS_TLS_KEY` | Optional client mTLS for `https://{ip}:50001/metrics` (all three required together) |
| `MGMT_PERTISKCTL` | Path to `pertiskctl` (default `./out/bin/pertiskctl`) |

Auth0 role claim: `https://pertisk.io/role` or `role` → `admin` \| `operator` \| `viewer`.

## Dashboard

Home (`/`) shows cluster counts plus a **Cluster resources** section: one card per cluster with CPU, memory, and disk donut charts.

| Metric | Source |
|--------|--------|
| CPU / memory usage | `kubectl top nodes` (needs metrics-server) vs provisioned cores / memory from inventory |
| Disk | kubelet stats summary (`/proxy/stats/summary` filesystem) when reachable; else provisioned `disk_gb` totals without % |

Polls `GET /api/dashboard/resources` about every 15s. Cluster list / job status updates push via **SSE** (`GET /api/events?token=…`) with a slow poll fallback. Click a card to open the cluster.

## Nodes tab

Cluster detail **Nodes** shows lifecycle status (`ready` / `provisioning` / …) plus live **online** / **offline** from a TCP probe to each guest Machine API (`:50000`).

## Node detail

Cluster → Nodes → click a node name → `/clusters/:id/nodes/:nid`.

The page shows inventory (VMID, IPs, K8s, hardware), live Machine Health, and charts:

| Source | How mgmt collects it |
|--------|----------------------|
| Health | `pertiskctl -e {ip}:50000 health` |
| Gauges + API metrics | HTTP(S) scrape `{http\|https}://{ip}:50001/metrics` (HTTPS + client cert when `MGMT_METRICS_TLS_*` set) |
| CPU / memory % | `kubectl top node` via cluster kubeconfig (needs metrics-server) |

Charts poll every ~4s and keep ~60 samples **in the browser** only. Soft errors show under each section.

A **Logs** panel tails `pertiskd` / `containerd` / `kubelet` / `dmesg` via `pertiskctl logs` (unary poll). For live follow on the node CLI: `pertiskctl logs -f` / `pertiskctl logs -f container:<id>`.

## Cluster K8s tab

When a cluster is **ready** (kubeconfig stored under `{data_dir}/kubeconfigs/{name}/`), the cluster detail **K8s** tab lists workloads via `kubectl` on the management host:

| Kind | Actions |
|------|---------|
| Deployments | list, scale, rollout restart, delete |
| StatefulSets / DaemonSets / Jobs / CronJobs | list, delete |
| Pods | list |

## Cluster Shell tab

**Shell** opens an interactive OS shell **on the management host** (not a guest pod). `KUBECONFIG` is set to this cluster’s admin.conf so you can install apps with:

```bash
kubectl get ns
helm install …
```

Requires `kubectl` and (optionally) `helm` on the mgmt host PATH. Operator/admin only.

API (Bearer JWT; shell needs **operator/admin**):

- `GET /api/clusters/{id}/kubeconfig` — admin kubeconfig YAML download
- `GET /api/clusters/{id}/config-bundle` — ZIP of `{data_dir}/kubeconfigs/{name}/` (`admin.conf`, `worker.yaml`, role MachineConfigs)
- `GET /api/clusters/{id}/k8s/namespaces`
- `GET /api/clusters/{id}/k8s/workloads/{kind}?namespace=`
- `POST /api/clusters/{id}/k8s/deployments/{ns}/{name}/scale`
- `POST /api/clusters/{id}/k8s/deployments/{ns}/{name}/restart`
- `DELETE /api/clusters/{id}/k8s/workloads/{kind}/{ns}/{name}`
- `GET /api/clusters/{id}/k8s/shell?token=` (WebSocket host PTY; `token` = JWT)

Requires `kubectl` on the mgmt host PATH (same as node sync / `kubectl top`).

## Proxmox provider

UI → Providers → add URL, API token, node, storage (same fields as [PROXMOX.md](./PROXMOX.md)).

Secrets are encrypted at rest with `MGMT_SECRET_KEY`. For lab self-signed TLS, set **Insecure TLS = Yes**.

## vSphere (ESXi) provider

UI → Providers → Kind **vSphere (ESXi)** → URL, username/password, host, datastore, network ([VSPHERE.md](./VSPHERE.md)).

Uses SOAP `/sdk` (not REST). Cluster create runs `vsphere-lab-up.sh` / `vsphere-upload-vm.sh` (qcow2 → VMDK upload). Mgmt must share L2 with guests for MAC→IP (`LAB_SUBNET`).

## Clusters (HA)

When `M > 1`, **VIP** is required (kube-vip). Use a **free** L2 IPv4. Guest images need `af_packet` for kube-vip ARP.

Create form **Max pods** (default `250`) is written into machine config as:

```yaml
machine:
  kubelet:
    extraConfig:
      maxPods: 250
```

pertiskd applies that into kubelet’s `KubeletConfiguration`. Omit uses the upstream default (`110`). Join configs copy it from the bootstrapped control-plane.

RPM create uses `--skip-build` and reads qcow2 from `/var/lib/pertisk-mgmt/images/`.

## Docker

```bash
docker build -f crates/pertisk-mgmt/Dockerfile -t pertisk-mgmt .
docker run -p 8080:8080 -e MGMT_ADMIN_PASSWORD=admin -e MGMT_SECRET_KEY=dev \
  -v pertisk-mgmt-data:/data pertisk-mgmt
```

## RPM deploy (mgmt only — no copy to Proxmox)

Same idea as [Omni’s Proxmox infra provider](https://github.com/siderolabs/omni-infra-provider-proxmox): talk to Proxmox over the **API token only**. Images live on the **mgmt** host; create uploads them with `content=import` + `scsi0 … import-from=` (no `scp` / SSH to PVE).

```text
[laptop]  stage-images + rpm
    │
    ├─ RPM + qcow2 ──► [mgmt]  /var/lib/pertisk-mgmt/images/
    │
    └─ UI Create ────► [mgmt] ──HTTPS API──► [Proxmox]
                         upload → local:import/…
                         import-from → local-zfs (VM disk)
```

### One-shot

```bash
./scripts/deploy-mgmt-lab.sh --mgmt almalinux@10.1.1.12 --version 0.1.3
# sets PROXMOX_NO_SSH=1 PROXMOX_UPLOAD_STORAGE=local
```

### Manual

```bash
# 1) laptop
make stage-images VERSION=0.1.3
make rpm VERSION=0.1.3

# 2) mgmt — RPM
MGMT=almalinux@10.1.1.12
scp out/rpm/pertisk-mgmt-0.1.3-1.x86_64.rpm "$MGMT:/tmp/"
ssh "$MGMT" 'sudo rpm -Uvh /tmp/pertisk-mgmt-*.rpm && sudo systemctl enable --now pertisk-mgmt'

# 3) mgmt — images only (not Proxmox)
scp out/pertisk-cloud-amd64*.qcow2 "$MGMT:/tmp/"
ssh "$MGMT" 'sudo bash -c "
  mkdir -p /var/lib/pertisk-mgmt/images
  mv /tmp/pertisk-cloud-amd64*.qcow2 /var/lib/pertisk-mgmt/images/
  chown -R pertisk-mgmt:pertisk-mgmt /var/lib/pertisk-mgmt/images
"'

# 4) /etc/pertisk-mgmt/pertisk-mgmt.env
PROXMOX_NO_SSH=1
PROXMOX_UPLOAD_STORAGE=local
LAB_SUBNET=10.1.1.0/24
PERTISK_IMAGES_DIR=/var/lib/pertisk-mgmt/images
# do NOT set PROXMOX_SSH (or comment it out)

sudo systemctl restart pertisk-mgmt
```

On Proxmox: **Datacenter → Storage → local → Content** must include **Import**. VM disks can still use `local-zfs`.

**IP discovery without SSH:** mgmt must be on the **same L2** as the guests (`LAB_SUBNET=10.1.1.0/24`). Lab-up ping-sweeps that subnet and matches MACs in the local ARP table. If mgmt is routed-only (no L2), set `PROXMOX_SSH=root@<pve>` so ARP is read on the Proxmox bridge.

### UI

1. Providers → Proxmox URL + API token + node + storage (`Insecure TLS` if needed).
2. Clusters → Create (free VIP if CP > 1).
3. Job log: `API upload content=import → storage=local` then `import-from` — **not** `SCP + qm importdisk`.

### Optional SSH mode

Only if you want `scp` + `qm importdisk`:

```bash
PROXMOX_SSH=root@10.1.1.195
# unset PROXMOX_NO_SSH
```

Or `./scripts/deploy-mgmt-lab.sh --mgmt … --with-ssh --pve 10.1.1.195`.

## RBAC

| Role | Capabilities |
|------|----------------|
| `viewer` | Read clusters/providers/machines/templates/audit |
| `operator` | Create/update/delete clusters, providers, nodes, upgrades, templates |
| `admin` | Operator + delete providers |

## Phase D — fleet views

| Page | API | Notes |
|------|-----|--------|
| **Machines** | `GET /api/machines` | Cross-cluster node inventory; opens node detail |
| **Templates** | `GET/POST /api/templates`, `GET/PUT/DELETE /api/templates/{id}` | Machine-config YAML blueprints; load into cluster Config tab |
| **Audit** | `GET /api/audit?limit=&offset=&action=&resource=` | Management action log (`audit_log` table) |

### D2 — adopt / join tokens

| API | Notes |
|-----|--------|
| `POST /api/clusters/{id}/nodes/adopt` | Join existing host by IP (`role`, `ip`, optional `name`, `source`=`adopted`\|`baremetal`). Job runs `scripts/adopt-node.sh` (no VM create; `nodes.vmid` null). |
| `GET/POST /api/clusters/{id}/join-tokens` | Snapshot kube bootstrap token + endpoint from `worker.yaml`; returns copy-paste instructions |
| `GET/DELETE /api/clusters/{id}/join-tokens/{tid}` | Show instructions / soft-revoke snapshot (does not rotate kube Secret) |

UI: Cluster → Nodes → **Add node** modes — Create VM / Adopt / Join instructions.

Cloud providers (AWS/GCP/Azure) are **paused**; Proxmox + ESXi remain the supported hypervisors.
