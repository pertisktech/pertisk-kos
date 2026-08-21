# cert-manager + Cloudflare DNS-01

Install [cert-manager](https://cert-manager.io/) and a Let’s Encrypt `ClusterIssuer`
that solves ACME challenges with a **Cloudflare API token**.

Pinned release applied by the management UI: **v1.21.1**.

## Management UI

Cluster → **Add-ons** → **cert-manager**:

| Field | Example |
|-------|---------|
| DNS provider | `cloudflare` |
| ACME email | `ops@example.com` |
| API token | Cloudflare token with **Zone:DNS:Edit** |
| ACME environment | `production` or `staging` |
| Wildcard domain | `vsphere.pertisk.com` or `*.vsphere.pertisk.com` (optional) |

**Check config** validates the form and reports whether the controller, webhook, token Secret, `ClusterIssuer`, reflector, and wildcard Certificate are present. **Install** applies the upstream manifest, patches the webhook onto the **host network** (port `10260`, so host-networked kube-apiserver / Cilium kubeProxyReplacement can reach it), waits for deployments + endpoints, then creates the Secret + issuer (`letsencrypt-cloudflare`). If a wildcard domain is set, it also installs [kubernetes-reflector](https://github.com/emberstack/kubernetes-reflector) and a `Certificate` for the apex + `*.domain`. The TLS Secret is reflected into **every namespace**.

The token is stored encrypted on the management host (`MGMT_SECRET_KEY`) and is not returned by the API. Leave the token field blank on a later update to keep the stored value.

If a previous install failed on `webhook.cert-manager.io` / `no route to host`, run **Install** again after upgrading mgmt — the webhook patch is applied on every install.

## Manual install

```bash
kubectl apply -f https://github.com/cert-manager/cert-manager/releases/download/v1.21.1/cert-manager.yaml
kubectl -n cert-manager wait --for=condition=Available \
  deploy/cert-manager deploy/cert-manager-cainjector deploy/cert-manager-webhook --timeout=180s

kubectl -n cert-manager create secret generic cloudflare-api-token-secret \
  --from-literal=api-token="$CLOUDFLARE_API_TOKEN" \
  --dry-run=client -o yaml | kubectl apply -f -

kubectl apply -f examples/addons/cert-manager/clusterissuer-cloudflare.yaml
```

Edit the email (and optionally `server` for staging) in the ClusterIssuer before applying.

Issue a wildcard (apex + `*.domain`) and reflect the Secret to all namespaces:

```bash
kubectl apply -f https://github.com/emberstack/kubernetes-reflector/releases/latest/download/reflector.yaml
# edit dnsNames / secretName in wildcard-certificate.yaml first
kubectl apply -f examples/addons/cert-manager/wildcard-certificate.yaml
```

Use `issuerRef.name: letsencrypt-cloudflare` and `issuerRef.kind: ClusterIssuer` on other Certificates.
