# Pertisk Management UI

Single-port control plane: **Rust API** (`pertisk-mgmt`) + **React UI** (Adminator-inspired shell).

Production install (RPM, providers, **SSH matrix**): [DEPLOY.md](./DEPLOY.md).

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
| `MGMT_PUBLIC_URL` | Public base URL for OIDC callback + guest serial (`machine.dashboard.mgmt_url`). Set in `/etc/pertisk-mgmt/pertisk-mgmt.env`. `deploy-mgmt-lab.sh` preserves an existing value unless you export `MGMT_PUBLIC_URL=…` for that run. |
| `MGMT_METRICS_TOKEN` | Optional Bearer when scraping guest `:50001/metrics` |
| `MGMT_METRICS_TLS_CA` / `MGMT_METRICS_TLS_CERT` / `MGMT_METRICS_TLS_KEY` | Optional client mTLS for `https://{ip}:50001/metrics` (all three required together) |
| `MGMT_PERTISKCTL` | Path to `pertiskctl` (default `./out/bin/pertiskctl`) |

Auth0 role claim: `https://pertisk.io/role` or `role` → `admin` \| `operator` \| `viewer`.

### Auth0 Application Settings

`pertisk-mgmt` builds the OIDC `redirect_uri` from `MGMT_PUBLIC_URL`:

```text
{MGMT_PUBLIC_URL}/api/auth/oidc/callback
```

In the Auth0 dashboard → **Applications** → your Regular Web Application → **Settings**, add exact matches (no trailing slash on the callback path unless `MGMT_PUBLIC_URL` itself ends with one — it should not):

| Field | Value (example) |
|-------|-----------------|
| **Allowed Callback URLs** | `https://mgmt.example.com/api/auth/oidc/callback` |
| **Allowed Logout URLs** | `https://mgmt.example.com/` |
| **Allowed Web Origins** | `https://mgmt.example.com` |

Sign out for Auth0 users hits `GET /api/auth/logout`, which redirects to Auth0 `/v2/logout?…&federated` and then back to **Allowed Logout URLs**. Without that allowlist entry, Auth0 logout fails and the next SSO login reuses the previous account. OIDC start also sends `prompt=login` so Auth0 shows the login / account UI.

If you also use a local or IP URL, list every callback on separate lines (or comma-separated), e.g.:

```text
https://mgmt.example.com/api/auth/oidc/callback
http://127.0.0.1:8080/api/auth/oidc/callback
```

Mismatch (`Callback URL mismatch`) means Auth0 received a `redirect_uri` that is not in **Allowed Callback URLs** — usually `MGMT_PUBLIC_URL` was updated but Auth0 was not, or a typo (`http` vs `https`, host, or path).

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

The page shows inventory (VMID, IPs, K8s, OS, hardware), live Machine Health, and charts:

| Source | How mgmt collects it |
|--------|----------------------|
| Health | `pertiskctl -e {ip}:50000 health` |
| Gauges + API metrics | HTTP(S) scrape `{http\|https}://{ip}:50001/metrics` (HTTPS + client cert when `MGMT_METRICS_TLS_*` set) |
| CPU / memory % | `kubectl top node` via cluster kubeconfig (needs metrics-server) |

Charts poll every ~4s and keep ~60 samples **in the browser** only. Soft errors show under each section. For durable CPU / RAM / net / disk series, scrape `:50001` into Prometheus (or Alloy → Mimir) and import [examples/observability/grafana-node.json](../examples/observability/grafana-node.json).

A **Logs** panel tails `pertiskd` / `containerd` / `kubelet` / `dmesg` via `pertiskctl logs` (unary poll). For live follow on the node CLI: `pertiskctl logs -f` / `pertiskctl logs -f container:<id>`. For cluster-wide durable logs, set `machine.observability.lokiUrl` (or `PERTISK_LOKI_URL`) so `pertiskd` pushes to Loki / Alloy — see [examples/observability](../examples/observability/README.md).

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

## Cluster Upgrade tab

Two independent rolling jobs. Kubernetes version and node OS are **not** the same upgrade.

| Action | API | What it changes |
|--------|-----|-----------------|
| Kubernetes rolling upgrade | `POST /api/clusters/{id}/upgrade` `{ "version": "v1.36.3" }` | kubelet + control-plane static pods |
| OS A/B upgrade | `POST /api/clusters/{id}/os-upgrade` (multipart) | kernel + initramfs (`pertiskd`); STATE/etcd stay |

OS upgrade upload: the four signed files (`kernel`, `initramfs`, `manifest.json`, `manifest.sig`) or a `.zip` of them. Max 512 MiB.

```bash
make os-trust                              # once → out/secrets/os-trust.{sk,pk}
make os-bundle VERSION=0.2.86 ARCH=amd64   # → out/os-bundle-amd64-v0.2.86.zip (includes os-trust.pk)
```

Job `upgrade_os` stages the bundle onto each guest via a privileged hostPath pod (`/var/lib/pertisk-os-upgrade`), installs `os-trust.pk` on STATE if missing, then `pertiskctl upgrade --bundle … --reboot` and `mark-boot-good`. Order: workers first, then control planes. Requires `kubectl` + `pertiskctl` on the mgmt host.

Recreating VMs from a new qcow2 is a reinstall, not this path.

## Proxmox provider

UI → Providers → add URL, API token, node, storage (same fields as [PROXMOX.md](./PROXMOX.md)).

Secrets are encrypted at rest with `MGMT_SECRET_KEY`. For lab self-signed TLS, set **Insecure TLS = Yes**.

## vSphere (ESXi) provider

UI → Providers → Kind **vSphere (ESXi)** → URL, username/password, host, datastore, network ([VSPHERE.md](./VSPHERE.md)).

Uses SOAP `/sdk` (not REST). Cluster create runs `vsphere-lab-up.sh` / `vsphere-upload-vm.sh` (qcow2 → VMDK upload). Mgmt must share L2 with guests for MAC→IP (`LAB_SUBNET`).

## Nutanix (AHV) provider

UI → Providers → Kind **Nutanix (AHV)** → URL (`:9440`), username/password, cluster, storage container, network ([NUTANIX.md](./NUTANIX.md)).

Uses Prism Element REST v2.0 (+ v0.8 image upload). Cluster create runs `nutanix-lab-up.sh` / `nutanix-upload-vm.sh` (qcow2 → DISK_IMAGE → UEFI VM). Mgmt must share L2 with guests for MAC→IP (`LAB_SUBNET`).

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
./scripts/deploy-mgmt-lab.sh --mgmt user@mgmt.example.com --version 0.3.0
# sets PROXMOX_NO_SSH=1 PROXMOX_UPLOAD_STORAGE=local
```

### Manual

```bash
# 1) laptop
make stage-images VERSION=0.1.3
make rpm VERSION=0.1.3

# 2) mgmt — RPM
MGMT=user@mgmt.example.com
scp out/rpm/pertisk-mgmt-0.3.0-1.x86_64.rpm "$MGMT:/tmp/"
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

`PROXMOX_SSH` is a **user + “prefer SSH” flag**, not a single hypervisor. Lab-up rewrites the host to the **current provider URL** (`root@other-pve` on a cluster whose API is `https://this-pve:8006` becomes `root@this-pve`). Install the mgmt host key on **each** Proxmox; if SSH fails, import/resize fall back to the API.

```bash
PROXMOX_SSH=root@any-pve   # host is ignored; rewritten per provider
# unset PROXMOX_NO_SSH
```

Or `./scripts/deploy-mgmt-lab.sh --mgmt user@mgmt.example.com --with-ssh --pve pve.example.com`.

## RBAC

| Role | Capabilities |
|------|----------------|
| `viewer` | Read clusters/providers/machines/templates/audit |
| `operator` | Create/update/delete clusters, providers, nodes, upgrades, templates |
| `admin` | Operator + delete providers |

## Phase D — fleet views

| Page | API | Notes |
|------|-----|--------|
| **Machines** | `GET /api/machines` | Cross-cluster node inventory with live **online** / **offline** (Machine API `:50000`); opens node detail |
| **Templates** | `GET/POST /api/templates`, `GET/PUT/DELETE /api/templates/{id}` | Machine-config YAML blueprints; load into cluster Config tab |
| **Audit** | `GET /api/audit?limit=&offset=&action=&resource=` | Management action log (`audit_log` table) |

### D2 — adopt / join tokens

| API | Notes |
|-----|--------|
| `POST /api/clusters/{id}/nodes/adopt` | Join existing host by IP (`role`, `ip`, optional `name`, `source`=`adopted`\|`baremetal`). Job runs `scripts/adopt-node.sh` (no VM create; `nodes.vmid` null). |
| `GET/POST /api/clusters/{id}/join-tokens` | Snapshot kube bootstrap token + endpoint from `worker.yaml`; returns copy-paste instructions |
| `GET/DELETE /api/clusters/{id}/join-tokens/{tid}` | Show instructions / soft-revoke snapshot (does not rotate kube Secret) |

UI: Cluster → Nodes → **Add node** modes — Create VM / Adopt / Join instructions.

Cloud providers (AWS/GCP/Azure) are **paused**; Proxmox, ESXi, and Nutanix AHV are the supported hypervisors.
