# Pertisk Management UI

Single-port control plane: **Rust API** (`pertisk-mgmt`) + **React UI** (Adminator-inspired shell).

## Quick start

```bash
# Seed admin (optional; defaults admin/admin)
export MGMT_ADMIN_USER=admin
export MGMT_ADMIN_PASSWORD=admin
export MGMT_SECRET_KEY=$(openssl rand -hex 32)

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

Auth0 role claim: `https://pertisk.io/role` or `role` → `admin` \| `operator` \| `viewer`.

## Proxmox provider

UI → Providers → add URL, API token, node, storage (same fields as [PROXMOX.md](./PROXMOX.md)).

Secrets are encrypted at rest with `MGMT_SECRET_KEY`. For lab self-signed TLS, set **Insecure TLS = Yes** on the provider (same as `PROXMOX_INSECURE=1`).

## Clusters (HA)

Create with **M control planes** + **N workers**. When `M > 1`, **VIP** is required (kube-vip), matching:

```bash
./scripts/proxmox-lab-up.sh --controlplanes 3 --vip 10.1.1.200 --workers 2 --cni cilium
```

Jobs shell `MGMT_LAB_UP` (default `./scripts/proxmox-lab-up.sh`) with provider credentials.

## Docker

```bash
docker build -f crates/pertisk-mgmt/Dockerfile -t pertisk-mgmt .
docker run -p 8080:8080 -e MGMT_ADMIN_PASSWORD=admin -e MGMT_SECRET_KEY=dev \
  -v pertisk-mgmt-data:/data pertisk-mgmt
```

## RBAC

| Role | Capabilities |
|------|----------------|
| `viewer` | Read clusters/providers |
| `operator` | Create/update/delete clusters, providers, nodes, upgrades |
| `admin` | Operator + delete providers |
