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

## DNS-01 / `no such host` on Cloudflare nameservers

If the Certificate stays **Issuing** and challenges log:

```text
Waiting for DNS-01 challenge propagation: dial udp: lookup sid.ns.cloudflare.com. on 10.96.0.10:53: no such host
```

cluster DNS (`kube-dns` **10.96.0.10** / CoreDNS) cannot reach a working upstream resolver. cert-manager checks TXT records by querying public DNS; that path goes **pod → CoreDNS → upstream**.

**Check:**

```bash
kubectl -n kube-system get pods -l k8s-app=kube-dns
kubectl -n kube-system logs deploy/coredns --tail=30
kubectl run dns-test --rm -it --restart=Never --image=busybox:1.36 -- \
  nslookup sid.ns.cloudflare.com
```

**Fix (re-apply CoreDNS with public forwarders):**

```bash
kubectl apply -f examples/dns/coredns.yaml
kubectl -n kube-system rollout restart deploy/coredns
kubectl -n kube-system rollout status deploy/coredns --timeout=120s
```

The bundled Corefile forwards to `1.1.1.1`, `8.8.8.8`, then node `resolv.conf`. Ensure pods can egress **UDP/TCP 53** (and generally reach the internet) — Cilium/network policy must not block CoreDNS or cert-manager egress.

After DNS works, delete stuck challenges or re-trigger issuance:

```bash
kubectl -n cert-manager describe certificate <name>
kubectl -n cert-manager delete challenge --all   # cert-manager recreates them
```
