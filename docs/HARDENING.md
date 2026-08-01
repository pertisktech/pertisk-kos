# Pertisk KOS — Hardening checklist (CIS-ish)

Baseline: [CIS Kubernetes Benchmark](https://www.cisecurity.org/benchmark/kubernetes) worker-node controls, plus Pertisk OS posture from [DESIGN.md](../DESIGN.md) §8.

Status: **pass** · **partial** · **gap** · **n/a** (control-plane / not applicable)

## OS / node control plane

| Control | Status | Notes |
|---------|--------|-------|
| No SSH / interactive shell in production image | pass | Default initramfs is API-only; BusyBox present for install/DHCP helpers only |
| Immutable root FS | partial | Initramfs root; STATE/EPHEMERAL writable; full SquashFS/EROFS root still Phase 4/5 |
| Management API mTLS | pass | `PERTISK_TLS_*` + `scripts/gen-mtls-certs.sh` |
| Signed A/B OS upgrades | pass | Ed25519 trust key on STATE; unsigned rejected |
| Metrics endpoint auth | gap | Prometheus `:50001` is plaintext HTTP — firewall or bind to loopback in prod |
| STATE `secrets/` mode `0700` | pass | Set in `StateVolume::ensure_layout` |
| Kernel sysctls before kubelet | pass | `pertiskd` `sysctl::apply_hardening_sysctls` |
| Secure Boot / UKI measured boot | gap | Stretch (DESIGN §8.6) |
| Minimal kernel modules | gap | Still using Alpine virt kernel for QEMU |

## Kubelet (CIS §4.2)

| ID | Control | Status | Implementation |
|----|---------|--------|----------------|
| 4.2.1 | Anonymous auth disabled | pass | `authentication.anonymous.enabled: false` |
| 4.2.2 | Authorization not AlwaysAllow | pass | `authorization.mode: Webhook` |
| 4.2.3 | Client CA file set | partial | CA embedded when `cluster.ca` present; otherwise insecure-skip (dev) |
| 4.2.4 | Read-only port 0 | pass | `readOnlyPort: 0` |
| 4.2.5 | streamingConnectionIdleTimeout ≠ 0 | pass | `5m` |
| 4.2.6 | protectKernelDefaults | pass | `true` + matching sysctls |
| 4.2.7 | makeIPTablesUtilChains | pass | `true` |
| 4.2.8 | hostnameOverride only if needed | pass | From `machine.network.hostname` |
| 4.2.9 | eventRecordQPS | pass | `5` |
| 4.2.10 | rotateCertificates | pass | `true` |
| 4.2.11 | Rotate kubelet server cert | partial | `serverTLSBootstrap: true` (needs CSR approval on CP) |
| 4.2.12 | Strong TLS ciphers | gap | Not yet pinned in KubeletConfiguration |
| — | kubeconfig / CA mode `0600` | pass | After write |

## Filesystem mounts

| Mount | Flags | Status |
|-------|-------|--------|
| `/proc` | nosuid,noexec,nodev | pass |
| `/sys` | nosuid,noexec,nodev | pass |
| `/dev` | nosuid | pass |
| `/run`, `/tmp`, `/var` | nosuid,nodev | pass |

## Operator checklist (production)

1. Generate mTLS material; never run management API without TLS on untrusted networks.
2. Place `os-trust.pk` under `STATE/secrets/` before first upgrade.
3. Always set `cluster.ca` for join configs (avoid insecure-skip).
4. Prefer `cluster.cni: none` + audited CNI (Cilium) for multi-tenant clusters.
5. Firewall or restrict scrape of `:50001` (or set `--metrics-listen 127.0.0.1:50001`).
6. Approve kubelet serving certificate CSRs after first join (`serverTLSBootstrap`).
7. Keep SBOM (`scripts/generate-sbom.sh`) and CI green on release tags.

## Gaps tracked for later

- Metrics mTLS or authn token
- Explicit kubelet `tlsCipherSuites`
- Production debug image signed separately (no BusyBox in default)
- CIS automated scan job in CI (kube-bench against QEMU worker)
