# KOS scaler

Worker-node autoscaler from Helm chart `pertisk/kos-scaler`
(`https://chart.tools.pertisk.com`).

```bash
helm repo add pertisk https://chart.tools.pertisk.com
helm repo update
helm upgrade --install kos-scaler pertisk/kos-scaler \
  --namespace kos-scaler --create-namespace \
  --set mgmt.endpoint=https://ptkos.example \
  --set mgmt.clusterId=<cluster-uuid> \
  --set mgmt.username=admin \
  --set mgmt.password=…
```

From the management UI: cluster → **Add-ons** → **Autoscaling** → KOS scaler.

Install injects this cluster’s UUID and the management public URL (or **Mgmt URL override**).
The account must be **admin** or **operator**. Requires `helm` on the management host.
State PVC defaults to StorageClass `nfs-client` (install NFS first, or set StorageClass to `none`).
