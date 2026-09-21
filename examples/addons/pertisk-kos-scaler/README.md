# Pertisk KOS scaler

Worker-node autoscaler from Helm chart `pertisk/pertisk-kos-scaler`
(`https://charts.tools.thaidevops.co`).

```bash
helm repo add pertisk https://charts.tools.thaidevops.co
helm repo update
helm upgrade --install pertisk-kos-scaler pertisk/pertisk-kos-scaler \
  --namespace pertisk-kos-scaler --create-namespace \
  --set image.repository=registry.tools.thaidevops.co/pertisksoft/pertisk-kos-scaler \
  --set mgmt.endpoint=https://ptkos.example \
  --set mgmt.clusterId=<cluster-uuid> \
  --set mgmt.username=admin \
  --set mgmt.password=…
```

From the management UI: cluster → **Add-ons** → **Autoscaling** → Pertisk KOS scaler.

Install injects this cluster’s UUID and the management public URL (or **Mgmt URL override**).
Images come from `MGMT_IMAGE_REGISTRY` (default `registry.tools.thaidevops.co`).
The account must be **admin** or **operator**. Requires `helm` on the management host.
State PVC defaults to StorageClass `nfs-client` (install NFS first, or set StorageClass to `none`).
