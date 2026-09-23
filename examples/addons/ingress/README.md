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
| Image tag | `0.1.95` | Multi-arch tag (`0.1.95` / `v0.1.95` both exist). Install pins a platform digest when the registry answers |
| Registry user / password | empty | Optional. Leave blank for a **public** registry (no login). Set only for a private project |
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
  --set image.tag=0.1.95 \
  --set nodeSelector.kubernetes\.io/arch=amd64 \
  --set image.pullPolicy=Always

# ARM example: --set image.tag=0.1.95-arm64 --set nodeSelector.kubernetes\.io/arch=arm64

# Pull (public registry — no login):
#   docker pull registry.tools.thaidevops.co/pertisk-proxy/ingress:0.1.95
# Private registry: docker login, or set imagePullSecrets / MGMT_IMAGE_REGISTRY_*

kubectl -n pertisk-proxy get deploy,svc pertisk-proxy-ingress
kubectl get ingressclass pertisk-proxy
```

IPv4-only labs should also set:

```bash
  --set service.ipFamilyPolicy=SingleStack \
  --set service.ipFamilies[0]=IPv4
```
