# Lightweight observability

Pertisk is a minimal OS (no SSH, immutable root). Host metrics are exported from **`pertiskd` itself** on `:50001/metrics` — no node_exporter, no extra guest daemon.

Logs stay on the Machine API (`pertiskctl logs`) and can also be **pushed** from `pertiskd` to Loki (or Alloy `loki.source.api`) when `lokiUrl` / `PERTISK_LOKI_URL` is set.

## Docker Compose (Grafana + Loki)

Lab install on the **mgmt / edge host** (not on Pertisk guests):

```bash
cd examples/observability
docker compose up -d
# Grafana     http://127.0.0.1:3000  (admin / admin)
# Loki        http://127.0.0.1:3100
# Alloy       :3500  (Loki push API)
# Pushgateway :9091  (Prometheus text PUT — node dashboard)
# Prometheus  http://127.0.0.1:9990
```

Dashboards **Pertisk node** and **Pertisk logs** are provisioned. Point nodes at the compose host LAN IP:

```yaml
machine:
  observability:
    lokiUrl: http://10.1.1.150:3500/loki/api/v1/push
    # prometheusPushUrl is optional — :3500 Loki implies :9091 Pushgateway
```

`pertiskd` pushes logs to `:3500` and (on a current image) host metrics to `:9091`. The **Pertisk node** dashboard reads Prometheus, not Loki — without the metrics push (or a `file_sd` pull of `:50001`) that dashboard stays empty while logs still work.

Direct Loki push also works: `http://10.1.1.150:3100/loki/api/v1/push` (no auto metrics push; set `prometheusPushUrl` or `PERTISK_PROM_PUSH_URL`).

Stack files: [docker-compose.yml](./docker-compose.yml), configs under [compose/](./compose/).

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
