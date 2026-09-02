# Pertisk Management UI

Single-port control plane: **Rust API** (`pertisk-mgmt`) + **React UI** (Adminator-inspired shell).

Production install (RPM / DEB, providers, **SSH matrix**): [DEPLOY.md](./DEPLOY.md).

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
| `MGMT_PUBLIC_URL` | Public base URL for OIDC callback + guest serial (`machine.dashboard.mgmt_url`) + password-reset links. Set in `/etc/pertisk-mgmt/pertisk-mgmt.env`. `deploy-mgmt-lab.sh` preserves an existing value unless you export `MGMT_PUBLIC_URL=…` for that run. |
| `MGMT_SMTP_HOST` / `MGMT_SMTP_FROM` | Enable outbound email (see SMTP section) |
| `MGMT_ADMIN_EMAILS` | Comma-separated admin inboxes for Auth0 first-login notices |
| `MGMT_METRICS_TOKEN` | Optional Bearer when scraping guest `:50001/metrics` |
| `MGMT_METRICS_TLS_CA` / `MGMT_METRICS_TLS_CERT` / `MGMT_METRICS_TLS_KEY` | Optional client mTLS for `https://{ip}:50001/metrics` (all three required together) |
| `MGMT_PERTISKCTL` | Path to `pertiskctl` (default `./out/bin/pertiskctl`) |

Auth0 role claim: `https://pertisk.io/role` or `role` → `admin` \| `operator` \| `viewer`. On first Auth0 sign-in the identity is auto-provisioned (default role `viewer` when the claim is absent). Pertisk does **not** gate Auth0 access with a local approval queue — configure allowlists / Actions / claim mapping in Auth0. When SMTP and `MGMT_ADMIN_EMAILS` are set, mgmt sends a non-blocking notice to those addresses after the first Auth0 identity is created.

### Local user management

Admins can manage accounts on the **Users** page (`/#/users`):

- Create local users with an email, role, and either a temporary password or a reset email
- Change roles, disable/enable accounts (the last enabled admin cannot be disabled or demoted)
- Resend password-reset email for local users (Auth0-only identities have no local password)

Public local reset (enumeration-safe): `POST /api/auth/password-reset/request` and `POST /api/auth/password-reset/confirm`. UI: **Forgot password?** on the login page and `/#/reset-password?token=…`.

### SMTP (password reset + Auth0 notices)

Email features are disabled cleanly when SMTP is not configured. Set both host and from address:

| Env | Description |
|-----|-------------|
| `MGMT_SMTP_HOST` | SMTP relay hostname |
| `MGMT_SMTP_PORT` | Port (default `587`) |
| `MGMT_SMTP_FROM` / `MGMT_SMTP_SENDER` | From address (Mailbox format, e.g. `Pertisk <noreply@example.com>`) |
| `MGMT_SMTP_USER` / `MGMT_SMTP_USERNAME` / `MGMT_SMTP_PASSWORD` | Optional AUTH credentials (both user and password required together; `USERNAME` is an alias for `USER`) |
| `MGMT_SMTP_TLS` | `none` \| `starttls` (default) \| `tls` (implicit TLS, typical port 465) |
| `MGMT_ADMIN_EMAILS` | Comma-separated recipients for Auth0 first-login notices |

Reset links use `MGMT_PUBLIC_URL` (`{public}/#/reset-password?token=…`). Send failures are logged and audited; they never block an Auth0 login.

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

Sign out for Auth0 users hits `GET /api/auth/logout`, which redirects to Auth0 `/v2/logout` (Auth0 app session only — not federated IdP logout) and then back to **Allowed Logout URLs**. Without that allowlist entry, Auth0 logout fails and the next SSO login may reuse the previous Auth0 session. OIDC start also sends `prompt=login` so Auth0 shows the login / account UI.

If you also use a local or IP URL, list every callback on separate lines (or comma-separated), e.g.:

```text
https://mgmt.example.com/api/auth/oidc/callback
http://127.0.0.1:8080/api/auth/oidc/callback
```

Mismatch (`Callback URL mismatch`) means Auth0 received a `redirect_uri` that is not in **Allowed Callback URLs** — usually `MGMT_PUBLIC_URL` was updated but Auth0 was not, or a typo (`http` vs `https`, host, or path).

## Dashboard

Home (`/`) shows cluster counts plus **Providers** and **Clusters** resource cards: CPU, memory, and disk donut charts with **used / available / total**.

Provider cards and **Providers → Dashboard** (or click the provider name) open `/providers/{id}` with the same gauges for that hypervisor.

| Surface | Metric | Source |
|--------|--------|--------|
| Cluster cards | CPU / memory usage | `kubectl top nodes` vs provisioned cores / memory |
| Cluster cards | Disk | kubelet stats summary when reachable; else provisioned `disk_gb` |
| Provider cards / dashboard | CPU / memory | Proxmox node status; Nutanix AHV hosts; ESXi host quickStats; pertisk-vms cluster members |
| Provider cards / dashboard | Disk | Selected storage / container / datastore capacity |

Polls `GET /api/dashboard/resources` and `GET /api/dashboard/providers` about every 15s. Cluster list / job status updates push via **SSE** (`GET /api/events?token=…`) with a slow poll fallback. Click a cluster card to open the cluster; click a provider card for the hypervisor dashboard (`GET /api/providers/{id}/dashboard`).

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

## Cluster Add-ons tab

When a cluster is **ready**, **Add-ons** uses group tabs (**Autoscaling**, **Certificates**, **Ingress**, **Storage & network**, **Dashboard**). It checks live install state and applies optional cluster apps via `kubectl` (and Helm for Helm-based add-ons) on the management host. CoreDNS and metrics-server stay bootstrap basics (not this tab).

| Add-on | Config | What install applies |
|--------|--------|----------------------|
| NFS storage | server IP/hostname + export path | `pertisk-nfs-modules` DaemonSet + nfs-subdir-external-provisioner (`StorageClass` `nfs-client`) |
| cert-manager | DNS provider (`cloudflare`), ACME email, API token, production/staging, optional wildcard domain | cert-manager `v1.21.1` + webhook `hostNetwork` (port 10260) + Cloudflare token Secret + `ClusterIssuer` `letsencrypt-cloudflare` + kubernetes-reflector + wildcard `Certificate` (apex + `*.domain`, Secret copied to all namespaces) |
| Cilium LoadBalancer | ELB IPv4; IPv6 when dual-stack | `CiliumLoadBalancerIPPool` + `CiliumL2AnnouncementPolicy` (listed only when cluster CNI is `cilium`) |
| Pertisk Ingress | image tag (default `v0.1.83`); optional admin host; TLS secret list (or HTTP only); optional admin password | Helm `pertisk-ingress` from `--repo https://chart.tools.pertisk.com` into `pertisk-proxy`, public Harbor image pinned to cluster arch (`linux/arm64` or `linux/amd64`) |
| Kubernetes Dashboard | namespace (default `pertisk-dashboard`), image tag, Dashboard user/password, optional hostname and TLS Secret (or HTTP only) | Helm `pertisk-kube` from `--repo https://chart.tools.pertisk.com`; creates the selected namespace, configures the Dashboard image/login, and enables a `pertisk-proxy` Ingress for the configured host |

**Check config** validates the form (and for NFS, TCP 2049 from the mgmt host) and reports live resources. **Install** / **Update** enqueues job `install_addon` (cluster stays ready on failure). Add-on configs (including encrypted tokens) are saved by **cluster name**. Delete keeps that preset; creating the same name again restores the forms and reinstalls after the cluster is ready (wizard **Reuse add-on config**, on by default). You can also copy presets from another cluster name. Add-on installs run **in parallel** with other clusters’ jobs (and with other add-ons on the same cluster); they wait only if *this* cluster already has a create/upgrade/node job running. Cloudflare tokens, Dashboard passwords, and the Dashboard JWT secret are encrypted at rest (`MGMT_SECRET_KEY`) and never returned by the API. The Dashboard JWT secret is generated automatically and preserved on updates. Ingress install needs `helm` on the mgmt host PATH (same as the Shell tab). `harbor.tools.pertisk.com/pertisk-proxy` is public (no pull secret unless you set Harbor user/password).

The Dashboard namespace is selected before its first install. To move an existing release from `default` (or another namespace), first uninstall the old Helm release, then install the add-on with the desired namespace; the chart owns cluster-scoped RBAC resources that cannot be adopted by a second release automatically.

API (Bearer JWT; install needs **operator/admin**):

- `GET /api/clusters/{id}/addons` — catalog + stored config + live status
- `GET /api/clusters/{id}/addons/{name}` — one add-on (`nfs` \| `cert-manager` \| `cilium-lb` \| `ingress` \| `kos-scaler` \| `kubernetes-dashboard`)
- `POST /api/clusters/{id}/addons/{name}/check` — validate submitted config + live probe
- `POST /api/clusters/{id}/addons/{name}/install` — persist config and enqueue `install_addon`
- `GET /api/addon-presets` — saved add-on configs by cluster name (for recreate / copy)
- `POST /api/clusters` — optional `reuse_addons` (default true) and `addon_preset` (source cluster name)

Guest NFS client modules: [image/extensions/nfs-client](../image/extensions/nfs-client/). Manifests: [examples/addons](../examples/addons/).

## Cluster Shell tab

**Shell** opens an interactive OS shell **on the management host** (not a guest pod). `KUBECONFIG` is set to this cluster’s admin.conf so you can install apps with:

```bash
kubectl get ns
helm install …
```

Requires `kubectl` and (optionally) `helm` on the mgmt host PATH. Operator/admin only.

API (Bearer JWT; shell needs **operator/admin**):

- `GET /api/clusters/{id}/kubeconfig` — admin kubeconfig YAML download
- `GET /api/clusters/{id}/versions` — component/package versions for overview (also included on `GET /api/clusters/{id}` as `versions`)
- `GET /api/clusters/{id}/config-bundle` — ZIP of `{data_dir}/kubeconfigs/{name}/` (`admin.conf`, `worker.yaml`, role MachineConfigs)
- `GET /api/clusters/{id}/k8s/namespaces`
- `GET /api/clusters/{id}/k8s/workloads/{kind}?namespace=`
- `POST /api/clusters/{id}/k8s/deployments/{ns}/{name}/scale`
- `POST /api/clusters/{id}/k8s/deployments/{ns}/{name}/restart`
- `DELETE /api/clusters/{id}/k8s/workloads/{kind}/{ns}/{name}`
- `GET /api/clusters/{id}/k8s/shell?token=` (WebSocket host PTY; `token` = JWT)

Requires `kubectl` on the mgmt host PATH (same as node sync / `kubectl top`).

Cluster **Overview** lists component/package versions: Kubernetes (kubelet), OS (Machine API `version` — same as the guest dashboard; not kubelet `osImage`), kernel and containerd (kubelet `nodeInfo`), CNI (cluster spec), and image pins for etcd / pause / kube-vip. OS **Target** is the latest catalog package for the cluster arch.

## Cluster Upgrade tab

Two independent rolling jobs. Kubernetes version and node OS are **not** the same upgrade.

| Action | API | What it changes |
|--------|-----|-----------------|
| Kubernetes rolling upgrade | `POST /api/clusters/{id}/upgrade` `{ "version": "v1.36.3" }` | kubelet + control-plane static pods |
| OS A/B upgrade | `POST /api/clusters/{id}/os-upgrade` (multipart) or `POST /api/clusters/{id}/os-upgrade/package` `{ "package_id" }` | kernel + initramfs (`pertiskd`); STATE/etcd stay |

**OS packages** (UI → **OS packages**): catalog of signed bundles by version + arch.

| API | Notes |
|-----|--------|
| `GET /api/os-packages` | List `{ id, version, arch, size_bytes, has_trust_pk, … }` |
| `POST /api/os-packages` | Multipart upload (same files as cluster OS upgrade). Upserts on `(version, arch)`. Max 512 MiB |
| `DELETE /api/os-packages/{id}` | Remove from catalog (blocked while a job uses it) |
| `POST /api/os-packages/{id}/apply` | `{ "cluster_ids": ["…"], "reboot": true }` — rolling `upgrade_os` per cluster |

OS upgrade upload: the four signed files (`kernel`, `initramfs`, `manifest.json`, `manifest.sig`) or a `.zip` of them. Max 512 MiB. Cluster uploads are also saved into the catalog.

```bash
make os-trust                              # once → out/secrets/os-trust.{sk,pk}
make os-bundle VERSION=0.2.86 ARCH=amd64   # → out/os-bundle-amd64-v0.2.86.zip (includes os-trust.pk)
make os-bundle VERSION=0.2.86 ARCH=arm64
```

Job `upgrade_os` stages the bundle onto each guest via a privileged hostPath pod (`/var/lib/pertisk-os-upgrade`), installs `os-trust.pk` on STATE if missing, then `pertiskctl upgrade --bundle … --reboot`, waits for the Machine API to go **down** then **up** (rediscovering DHCP/IPAM), `mark-boot-good`, uncordon. kubectl uses the kubeconfig VIP when it answers; if kube-vip is down it uses a control-plane `:6443`. If no API is reachable it runs `etcd recover --force-new-cluster` on `*-cp-1` and retries. On Nutanix, extra netcfg disks can steal UEFI (`Unable to find valid boot device`); the job re-pins boot to the OS disk and power-cycles if the guest stays dark. Order: workers first, then control planes. Requires `kubectl` + `pertiskctl` on the mgmt host.

Mgmt runs **one exclusive job per cluster** (create, delete, upgrade, add/remove/resize node). Jobs for **different clusters run in parallel**. **Add-on installs** can overlap with other clusters and with other add-ons; they wait only if *this* cluster already has a create/upgrade/node job. Delete cancels that cluster’s queued work, **aborts** a running create/upgrade/add-on, and removes job rows when the cluster is gone — so a new create is not stuck `queued` behind a leftover delete.

**Guests before 0.3.59** often stay **NotReady** after VM power-off/on: kubelet can miss the CRI race (`kubelet=absent`), HA etcd can lose quorum even when IPs did not change (`:6443` listens but `/readyz` fails, kube-vip never announces), and kube-vip can bind the wrong NIC or use an empty `spec.nodeName`. **0.3.59+** waits for IPv4, rebases advertise IPs without a default route, skips the kube-vip `/32` for `--node-ip`, refreshes kube-vip onto the live NIC + hostname, and on `*-cp-1` runs `--force-new-cluster` if a 3-member etcd still has no leader after ~3 minutes (2-member join is not recovered). Ship a new OS bundle. Lab: `./scripts/recover-not-ready-nodes.sh ~/.kube/ptkos/<cluster>.yaml` (uses `/readyz`, not TCP). Extra CPs after etcd recover must be reset + re-joined.

Recreating VMs from a new qcow2 is a reinstall, not this path.

## Images (install disks)

**Images** is the qcow2 catalog for **cluster create**. Mgmt does **not** compile the OS — build on a Docker host, then upload (or `scp` into `PERTISK_IMAGES_DIR`, default `/var/lib/pertisk-mgmt/images/`).

| API | Notes |
|-----|--------|
| `GET /api/images` | `{ dir, images: [{ name, arch, size_bytes, role, is_default }], ready: { amd64, arm64 } }` |
| `POST /api/images` | Multipart `image` + optional `arch`. Streams to disk. Max 8 GiB. Replaces same filename |
| `DELETE /api/images/{name}` | Remove file (blocked while create/add-node is running) |

Need `pertisk-cloud-amd64*.qcow2` and/or `pertisk-cloud-arm64*.qcow2`. Create Cluster fails fast if the matching arch is missing. **arm64** guests are Proxmox and Pertisk VMs; vSphere and Nutanix stay **amd64**.

Prefer GitHub Release assets (`pertisk-cloud-{arch}-v{VERSION}.qcow2`). Local build is optional:

```bash
gh release download 0.3.0 -p 'pertisk-cloud-amd64-*.qcow2' -p 'pertisk-cloud-arm64-*.qcow2'
# or: make cloud VERSION=0.3.0 ARCH=amd64
# UI → Images → Upload (versioned names like pertisk-cloud-amd64-v0.3.0.qcow2 are fine)
```

## Proxmox provider

UI → Providers → add URL, API token, node, storage (same fields as [PROXMOX.md](./PROXMOX.md)).

Secrets are encrypted at rest with `MGMT_SECRET_KEY`. For lab self-signed TLS, set **Insecure TLS = Yes**.

`GET /api/providers` (and cluster list/detail) include live **`availability`**: `online` if the hypervisor API accepts stored credentials, `offline` if unreachable or auth fails. Same badges as cluster/node reachability.

## vSphere (ESXi) provider

UI → Providers → Kind **vSphere (ESXi)** → URL, username/password, host, datastore, network ([VSPHERE.md](./VSPHERE.md)).

Uses SOAP `/sdk` (not REST). Cluster create runs `vsphere-lab-up.sh` / `vsphere-upload-vm.sh` (qcow2 → VMDK upload). Mgmt must share L2 with guests for MAC→IP (`LAB_SUBNET`).

## Nutanix (AHV) provider

UI → Providers → Kind **Nutanix (AHV)** → URL (`:9440`), username/password, cluster, storage container, network ([NUTANIX.md](./NUTANIX.md)).

Uses Prism Element REST v2.0 (+ v0.8 image upload). Cluster create runs `nutanix-lab-up.sh` / `nutanix-upload-vm.sh` (qcow2 → DISK_IMAGE → UEFI VM). Mgmt must share L2 with guests for MAC→IP (`LAB_SUBNET`). Prefer the **unmanaged** VLAN (`vlan.0`) plus LAN DHCP; managed IPAM needs the netcfg disk. HA join writes etcd membership first; if local `/readyz` is still pulling images, lab-up waits up to 12 minutes then continues (node labels come later). `pertiskctl` waits up to 30 minutes (`PERTISKCTL_LONG_RPC_SECS`). Lab VLAN is IPv4-only — leave the wizard on **IPv4** unless the LAN has RA/DHCPv6.

## Pertisk VMs provider

UI → Providers → Kind **Pertisk VMs** → URL (`:7443` HTTPS or `:7480` HTTP), username/password, node (`n1`), storage (`replica`), network (`vmbr0`) ([PERTISK_VMS.md](./PERTISK_VMS.md)).

Uses pertiskd REST `/v1`. Cluster create runs `pertisk-vms-lab-up.sh` / `pertisk-vms-upload-vm.sh` (stream qcow2 → template volume → clone → UEFI QEMU VM). **No SSH** to the hypervisor. Mgmt must share L2 with guests for MAC→IP (`LAB_SUBNET`). arm64 is allowed when the host arch is arm64.

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

<a id="rpm-deploy-linuxamd64"></a>

## Package deploy (mgmt only — no copy to Proxmox)

Same idea as [Omni’s Proxmox infra provider](https://github.com/siderolabs/omni-infra-provider-proxmox): talk to Proxmox over the **API token only**. Images live on the **mgmt** host; create uploads them with `content=import` + `scsi0 … import-from=` (no `scp` / SSH to PVE).

GitHub Releases ship **DEB + RPM**, **guest qcow2**, and (when `OS_TRUST_*` secrets are set) **OS A/B zips** for **amd64** and **arm64**. Lab `make rpm` builds amd64 packages only.

```text
[laptop]  stage-images + packages
    │
    ├─ DEB/RPM + qcow2 ──► [mgmt]  /var/lib/pertisk-mgmt/images/
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
# 1) laptop — amd64 only: make rpm    full matrix: make mgmt-pkg
make stage-images VERSION=0.1.3
make rpm VERSION=0.1.3

# 2) mgmt — RPM (RHEL / Rocky / Alma)
MGMT=user@mgmt.example.com
scp out/pkg/pertisk-mgmt-0.3.0-1.x86_64.rpm "$MGMT:/tmp/"
ssh "$MGMT" 'sudo rpm -Uvh /tmp/pertisk-mgmt-*.rpm && sudo systemctl enable --now pertisk-mgmt'

# 2b) mgmt — DEB (Debian / Ubuntu)
# scp out/pkg/pertisk-mgmt_0.3.0-1_amd64.deb "$MGMT:/tmp/"
# ssh "$MGMT" 'sudo apt-get install -y /tmp/pertisk-mgmt_*.deb && sudo systemctl enable --now pertisk-mgmt'

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
# Parallel VM clones during create (default 4; 1 = serial):
# PERTISK_VM_JOBS=4
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
| `viewer` | Read clusters/providers/machines/templates/os-packages/images/audit/addons |
| `operator` | Create/update/delete clusters, providers, nodes, upgrades, templates, OS packages, images; install cluster add-ons |
| `admin` | Operator + delete providers |

## Phase D — fleet views

| Page | API | Notes |
|------|-----|--------|
| **Machines** | `GET /api/machines` | Cross-cluster node inventory with live **online** / **offline** (Machine API `:50000`); opens node detail |
| **Providers** | `GET /api/providers`, `GET /api/providers/{id}/dashboard`, `GET /api/dashboard/providers` | Hypervisor inventory with live **online** / **offline**; CPU / memory / disk used·available·total |
| **OS packages** | `GET/POST /api/os-packages`, `DELETE /api/os-packages/{id}`, `POST /api/os-packages/{id}/apply` | Signed A/B OS bundles by version + arch; apply to matching clusters |
| **Images** | `GET/POST /api/images`, `DELETE /api/images/{name}` | Cloud qcow2 catalog for cluster create (`pertisk-cloud-{arch}.qcow2`); mgmt does not build |
| **Templates** | `GET/POST /api/templates`, `GET/PUT/DELETE /api/templates/{id}` | Machine-config YAML blueprints; load into cluster Config tab |
| **Audit** | `GET /api/audit?limit=&offset=&action=&resource=` | Management action log (`audit_log` table) |

### D2 — adopt / join tokens

| API | Notes |
|-----|--------|
| `POST /api/clusters/{id}/nodes/adopt` | Join existing host by IP (`role`, `ip`, optional `name`, `source`=`adopted`\|`baremetal`). Job runs `scripts/adopt-node.sh` (no VM create; `nodes.vmid` null). |
| `GET/POST /api/clusters/{id}/join-tokens` | Snapshot kube bootstrap token + endpoint from `worker.yaml`; returns copy-paste instructions |
| `GET/DELETE /api/clusters/{id}/join-tokens/{tid}` | Show instructions / soft-revoke snapshot (does not rotate kube Secret) |

UI: Cluster → Nodes → **Add node** modes — Create VM / Adopt / Join instructions.

Cloud providers (AWS/GCP/Azure) are **paused**; Proxmox, ESXi, Nutanix AHV, and Pertisk VMs are the supported hypervisors.
