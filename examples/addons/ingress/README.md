# Pertisk Ingress

Installs the [pertisk-ingress](https://chart.tools.pertisk.com) Helm chart
(`pertisk/pertisk-ingress`) into namespace `pertisk-proxy`, pinning the controller
image to Harbor.

Requires `helm` on the management host PATH (same as the cluster Shell tab).
The Service is `type: LoadBalancer`; on Cilium clusters, install **Cilium LoadBalancer**
first so an ELB IP can be assigned.

## Management UI

Cluster → **Add-ons** → **Pertisk Ingress**:

| Field | Default | Notes |
|-------|---------|--------|
| Image tag | `v0.1.83` | Multi-arch tag. Install pins `linux/{cluster-arch}` (digest or `v0.1.83-arm64`) so ARM nodes do not pull amd64 |
| Harbor user / password | empty | Optional. `pertisk-proxy` on Harbor is **public** — leave blank. Set only for a private project |
| Admin host | empty | Optional hostname for the viewer admin Ingress; skip to disable it |
| Admin password | chart default | Stored encrypted; leave blank to keep the current value |

Install matches cluster `network_mode` (`SingleStack` IPv4/IPv6, or `PreferDualStack`) and
cluster **arch** (`nodeSelector kubernetes.io/arch` plus a platform digest so kubelet
cannot pull the other architecture from a multi-arch tag).
Gateway API reconciliation is enabled only when Gateway API CRDs are already in the cluster.

## Manual install

```bash
helm repo add pertisk https://chart.tools.pertisk.com
helm repo update
helm upgrade --install pertisk-ingress pertisk/pertisk-ingress \
  --namespace pertisk-proxy --create-namespace \
  --set image.registry=harbor.tools.pertisk.com \
  --set image.repository=pertisk-proxy/ingress \
  --set image.tag=v0.1.83-arm64 \
  --set nodeSelector.kubernetes\.io/arch=arm64 \
  --set image.pullPolicy=Always

# ARM nodes must pull the arm64 variant (not the amd64 layer of a multi-arch tag):
#   docker pull harbor.tools.pertisk.com/pertisk-proxy/ingress:v0.1.83
#   harbor.tools.pertisk.com/pertisk-proxy/ingress:v0.1.83-arm64
# or :v0.1.83@sha256:<arm64-manifest>

kubectl -n pertisk-proxy get deploy,svc pertisk-proxy-ingress
kubectl get ingressclass pertisk-proxy
```

IPv4-only labs should also set:

```bash
  --set service.ipFamilyPolicy=SingleStack \
  --set service.ipFamilies[0]=IPv4
```
