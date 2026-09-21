# Pertisk CD

Continuous deployment control plane from Helm chart `pertisk/pertisk-cd`
(`https://charts.tools.thaidevops.co`).

Requires an **external Postgres** `DATABASE_URL`. The chart does not install a database.

```bash
helm repo add pertisk https://charts.tools.thaidevops.co
helm repo update
helm upgrade --install pertisk-cd pertisk/pertisk-cd \
  --namespace pertisk-cd --create-namespace \
  --set image.repository=registry.tools.thaidevops.co/pertisk-cd/pertisk-cd \
  --set secrets.databaseUrl='postgres://…' \
  --set secrets.adminEmail=admin@local \
  --set secrets.adminPassword=…
```

From the management UI: cluster → **Add-ons** → **Continuous deployment** → Pertisk CD.

Install uses `MGMT_HELM_CHART_REPO` / `MGMT_IMAGE_REGISTRY` (defaults
`https://charts.tools.thaidevops.co` and `registry.tools.thaidevops.co`).
