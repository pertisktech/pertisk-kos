# Pertisk Ingress

Installs the [pertisk-ingress](https://charts.tools.thaidevops.co) Helm chart
(`pertisk/pertisk-ingress`) into namespace `pertisk-proxy`, pinning the controller
image to the configured registry (`MGMT_IMAGE_REGISTRY`).

Requires `helm` on the management host PATH (same as the cluster Shell tab).
The Service is `type: LoadBalancer`; on Cilium clusters, install **Cilium LoadBalancer**
first so an ELB IP can be assigned.

## Management UI

Cluster → **Add-ons** → **Pertisk Ingress**:

| Field | Default | Notes |
|-------|---------|--------|
| Image tag | `v0.1.95` | Multi-arch tag. Install pins `linux/{cluster-arch}` (digest or `v0.1.95-arm64`) so ARM nodes do not pull amd64 |
| Registry user / password | empty | Optional. `pertisk-proxy` on the registry is **public** — leave blank. Set only for a private registry project |
| Admin host | empty | Optional hostname for admin Ingress (`pertisk-proxy-ingress-admin`, port 9080) |
| TLS secret | `none` | Shown when admin host is set. Pick a `kubernetes.io/tls` Secret (from cert-manager / reflector) or **none** for HTTP only |
| Admin password | chart default | Stored encrypted; leave blank to keep the current value |

Install matches cluster `network_mode` (`SingleStack` IPv4/IPv6, or `PreferDualStack`) and
cluster **arch** (`nodeSelector kubernetes.io/arch` plus a platform digest so kubelet
cannot pull the other architecture from a multi-arch tag).
Gateway API reconciliation is enabled only when Gateway API CRDs are already in the cluster.

## Manual install

```bash
# Prefer --repo so a local helm alias cannot resolve to Bitnami (or another public repo).
helm upgrade --install pertisk-ingress pertisk-ingress \
  --repo https://charts.tools.thaidevops.co \
  --version 0.1.95 \
  --namespace pertisk-proxy --create-namespace \
  --set image.registry=registry.tools.thaidevops.co \
  --set image.repository=pertisk-proxy/ingress \
  --set image.tag=v0.1.95-arm64 \
  --set nodeSelector.kubernetes\.io/arch=arm64 \
  --set image.pullPolicy=Always

# Equivalent with a named repo:
#   helm repo add pertisk https://charts.tools.thaidevops.co --force-update
#   helm upgrade --install pertisk-ingress pertisk/pertisk-ingress ...

# ARM nodes must pull the arm64 variant (not the amd64 layer of a multi-arch tag):
#   docker pull registry.tools.thaidevops.co/pertisk-proxy/ingress:v0.1.95
#   registry.tools.thaidevops.co/pertisk-proxy/ingress:v0.1.95-arm64
# or :v0.1.95@sha256:<arm64-manifest>

kubectl -n pertisk-proxy get deploy,svc pertisk-proxy-ingress
kubectl get ingressclass pertisk-proxy
```

IPv4-only labs should also set:

```bash
  --set service.ipFamilyPolicy=SingleStack \
  --set service.ipFamilies[0]=IPv4
```
