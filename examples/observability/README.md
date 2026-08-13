# Lightweight observability

Pertisk is a minimal OS (no SSH, immutable root). Host metrics are exported from **`pertiskd` itself** on `:50001/metrics` — no node_exporter, no extra guest daemon.

Logs stay on the Machine API (`pertiskctl logs`) and can also be **pushed** from `pertiskd` to Loki (or Alloy `loki.source.api`) when `lokiUrl` / `PERTISK_LOKI_URL` is set.

## Deploy (mgmt / edge host)

Do **not** run this on Pertisk guests. Copy the stack to the host that already runs `pertisk-mgmt` (lab: `root@10.1.1.15`, `/opt/observability`).

Prometheus uses **host networking** so it can scrape guest `:50001`. A Docker bridge cannot reach those VMs — Loki still works (nodes **push**), **Pertisk node** stays empty until Prometheus scrapes on the host network.

### 1. Copy files

```bash
# from the pertisk-kos repo
rsync -a examples/observability/ root@10.1.1.15:/opt/observability/
```

### 2. Start the stack

```bash
ssh root@10.1.1.15
cd /opt/observability
chmod +x sync-file-sd.sh
docker compose up -d
docker compose ps
```

| Service     | URL (on the compose host)     | Notes |
|-------------|-------------------------------|--------|
| Grafana     | http://10.1.1.15:3000         | `admin` / `admin` |
| Prometheus  | http://10.1.1.15:9990         | host network, listen `:9990` |
| Loki        | http://10.1.1.15:3100         | |
| Alloy       | http://10.1.1.15:3500         | Loki push ingest |
| Pushgateway | http://10.1.1.15:9091         | optional metrics push |

### 3. Scrape targets (Pertisk node dashboard)

```bash
cd /opt/observability
./sync-file-sd.sh          # writes compose/file_sd/nodes.yml from mgmt.db
# file_sd reloads within ~30s; or: docker compose restart prometheus
```

Check: **Prometheus → Status → Targets** — `pertisk-nodes` should be **up** for each ready guest. Query `pertisk_load1`. Then open Grafana → **Pertisk node**.

Re-run `sync-file-sd.sh` after cluster create / scale (new IPs).

### 4. Point nodes at Loki (logs dashboard)

Use the compose host **LAN** IP (not `localhost`, not a container name):

```yaml
machine:
  observability:
    lokiUrl: http://10.1.1.15:3500/loki/api/v1/push
```

Apply with `pertiskctl apply` (or cluster apply). Grafana → **Pertisk logs** (`{job="pertisk"}`).

On a current image, `lokiUrl` on `:3500` also implies metrics push to `:9091`. Pull scrape (step 3) is enough for **Pertisk node** without that.

### 5. If Pertisk node is empty

1. Prometheus in Docker bridge cannot reach VMs — keep `network_mode: host` and `--web.listen-address=0.0.0.0:9990`.
2. Grafana datasource must be `http://host.docker.internal:9990` (not `prometheus:9090`).
3. `file_sd/nodes.yml` must list real guest IPs (`./sync-file-sd.sh`).
4. From the **host**: `curl -sS http://<guest>:50001/metrics | grep pertisk_load1`

Direct Loki (no Alloy): `http://10.1.1.15:3100/loki/api/v1/push` — no auto metrics push; set `prometheusPushUrl` or rely on step 3.

## Architecture

```
pertiskd :50001/metrics  ──scrape──►  Prometheus
                         ──or──►     Alloy (edge proxy) ──remote_write──► Mimir
pertiskd logs            ──push──►   Alloy :3500 / Loki  ──► Loki
Grafana  ◄── Prometheus / Mimir / Loki
mgmt UI  ◄── scrape :50001 + pertiskctl logs (in-product, short retention)
```

## Slice status

| Slice | Status |
|-------|--------|
| 1. Host CPU / RAM / net / disk I/O on `:50001` | **Done** |
| 2. Grafana starter dashboard | **Done** — [grafana-node.json](./grafana-node.json) |
| 3. Loki push from every node | **Done** — `pertiskd` POST `/loki/api/v1/push` |
| 4. Edge proxy `remote_write` to Mimir | **Example** — [alloy-edge.alloy](./alloy-edge.alloy) |

## Metrics (`GET :50001/metrics`)

Existing health / boot / API counters plus host series (Linux `/proc`):

| Series | Type | Source |
|--------|------|--------|
| `pertisk_cpu_seconds_total{cpu,mode}` | counter | `/proc/stat` (use `rate()`) |
| `pertisk_load1` / `load5` / `load15` | gauge | `/proc/loadavg` |
| `pertisk_memory_total_bytes` / `_available_bytes` / `_free_bytes` | gauge | `/proc/meminfo` |
| `pertisk_network_{receive,transmit}_{bytes,packets}_total{device}` | counter | `/proc/net/dev` (skips `lo`) |
| `pertisk_disk_{read,written}_bytes_total{device}` | counter | `/proc/diskstats` (skips loop/ram/fd/sr/dm/zram) |
| `pertisk_filesystem_{size,avail}_bytes{label,mountpoint}` | gauge | STATE / EPHEMERAL |
| `pertisk_uptime_seconds` | gauge | `/proc/uptime` |
| `pertisk_host_info{hostname}` | gauge | kernel hostname |

Auth is unchanged: optional mTLS (`PERTISK_TLS_*`) and/or bearer (`PERTISK_METRICS_TOKEN`).

```bash
curl -s http://<node>:50001/metrics | grep '^pertisk_cpu\|^pertisk_memory\|^pertisk_network\|^pertisk_disk'
```

CPU busy ratio (PromQL):

```promql
1 - (
  rate(pertisk_cpu_seconds_total{cpu="total",mode="idle"}[5m])
  /
  sum by (instance) (rate(pertisk_cpu_seconds_total{cpu="total"}[5m]))
)
```

## Prometheus scrape

Static file: [prometheus-pertisk.yml](./prometheus-pertisk.yml). Point `targets` at guest IPs (mgmt inventory / Terraform output). Prefer HTTPS + client certs in production — see [docs/HARDENING.md](../../docs/HARDENING.md).

Kubernetes ServiceMonitor is optional; Pertisk nodes are often VMs without a metrics Service. Scraping `:50001` from Prometheus (or Alloy) on the mgmt / edge host is the intended path.

## Grafana

Import [grafana-node.json](./grafana-node.json) (metrics) and [grafana-logs.json](./grafana-logs.json) (Loki). Datasource UIDs `prometheus` and `loki` — change if yours differ.

In-product charts on **Node detail** still poll `:50001` in the browser (~60 samples). Grafana is the durable view.

## Loki push

Off by default. Same sources as `pertiskctl logs`: `pertiskd`, `containerd`, `kubelet`, `dmesg`. Starts at EOF (no backlog dump). Labels: `job=pertisk`, `service`, `hostname`, `cluster`, plus `extraLabels`.

Machine config (survives apply/reload):

```yaml
machine:
  observability:
    lokiUrl: http://10.1.1.10:3500/loki/api/v1/push
    # lokiToken: optional-bearer
    extraLabels:
      env: lab
```

Or env / flags (win over YAML): `PERTISK_LOKI_URL`, `PERTISK_LOKI_TOKEN`.

Empty URL disables the pusher. `pertiskctl apply` reloads it without a reboot.

LogQL:

```logql
{job="pertisk"}
{job="pertisk", service="kubelet"}
{job="pertisk"} |= "ERROR"
```

## Edge proxy (Alloy → Mimir / Loki)

[alloy-edge.alloy](./alloy-edge.alloy) runs **off the guest** (mgmt host or a small edge VM): scrape `:50001`, `remote_write` to Mimir, and accept Loki push on `:3500`.

```bash
export MIMIR_URL=https://mimir.example/api/v1/push
export LOKI_URL=https://loki.example/loki/api/v1/push
alloy run examples/observability/alloy-edge.alloy
```

Do not ship Prometheus, Mimir, Loki, or Grafana inside the Pertisk image.

## vs metrics-server

`examples/addons/metrics-server.yaml` remains for `kubectl top` and HPA. Host `:50001` series are for node OS / infra dashboards, not the Metrics API.
