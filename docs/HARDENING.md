# Pertisk KOS — Hardening checklist (CIS-ish)

Baseline: [CIS Kubernetes Benchmark](https://www.cisecurity.org/benchmark/kubernetes) worker-node controls, plus Pertisk OS posture from [DESIGN.md](../DESIGN.md) §8.

Status: **pass** · **partial** · **gap** · **n/a** (control-plane / not applicable)

## OS / node control plane

| Control | Status | Notes |
|---------|--------|-------|
| No SSH / interactive shell in production image | pass | Default `IMAGE_PROFILE=production`: no `/bin/sh`; DHCPv4 via in-process client (BusyBox `udhcpc` fallback). `mount`/`umount` BusyBox applets for kubelet volumes. Debug profile adds ash (`PROFILE=debug`) |
| Immutable root FS | partial | Initramfs root; STATE/EPHEMERAL writable; full SquashFS/EROFS root still Phase 4/5 |
| Management API mTLS | pass | `PERTISK_TLS_*` + `scripts/gen-mtls-certs.sh` |
| Signed A/B OS upgrades | pass | Ed25519 trust key on STATE; unsigned rejected |
| Metrics endpoint auth | pass | mTLS when `PERTISK_TLS_*` set (same PEMs as API); optional bearer: `--metrics-token` / `PERTISK_METRICS_TOKEN` / STATE `secrets/metrics.token` |
| STATE `secrets/` mode `0700` | pass | Set in `StateVolume::ensure_layout` |
| Kernel sysctls before kubelet | pass | `pertiskd` `sysctl::apply_hardening_sysctls` |
| Secure Boot / UKI measured boot | partial | UKI + OVMF enroll + sysfs PCR Attest lab; no TPM2 Quote / remote verify yet ([SECURE_BOOT.md](./SECURE_BOOT.md)) |
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
| 4.2.12 | Strong TLS ciphers | pass | `tlsCipherSuites` pinned (ECDHE + AES-GCM / ChaCha20) |
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
5. Prefer metrics mTLS (`PERTISK_TLS_*` also covers `:50001`) and/or a bearer token (`PERTISK_METRICS_TOKEN` / STATE `secrets/metrics.token`); otherwise bind `--metrics-listen 127.0.0.1:50001`.
6. Approve kubelet serving certificate CSRs after first join (`serverTLSBootstrap`).
7. Keep SBOM (`scripts/generate-sbom.sh`) and CI green on release tags.

## Automated checks (CI)

```bash
./scripts/check-hardening.sh
# or: make check-hardening
```

CI runs this gate on every PR (static CIS 4.2.x source checks + unit tests). It is **not** a full [kube-bench](https://github.com/aquasecurity/kube-bench) scan against a live node.

## Running kube-bench (manual)

Against a running Pertisk worker (after join), use a privileged Pod:

```bash
kubectl apply -f - <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: kube-bench
  namespace: kube-system
spec:
  hostPID: true
  hostNetwork: true
  containers:
    - name: kube-bench
      image: docker.io/aquasec/kube-bench:v0.10.0
      command: ["kube-bench", "run", "--targets", "node"]
      volumeMounts:
        - name: var-lib-kubelet
          mountPath: /var/lib/kubelet
          readOnly: true
        - name: etc-kubernetes
          mountPath: /etc/kubernetes
          readOnly: true
  restartPolicy: Never
  volumes:
    - name: var-lib-kubelet
      hostPath: { path: /var/lib/kubelet }
    - name: etc-kubernetes
      hostPath: { path: /etc/kubernetes }
  tolerations:
    - operator: Exists
EOF
kubectl logs -n kube-system kube-bench
```

Expect worker-node controls to largely match this checklist. Control-plane static-pod hardening is **partial** (Phase A bootstrap); treat CIS CP controls as follow-up.

## Secure Boot / UKI (stretch roadmap)

Pertisk goals (DESIGN §8): measured boot where feasible. Not required for v0.1.

| Step | Work | Status |
|------|------|--------|
| 1 | Keep signed A/B bundles (Ed25519) on STATE | done |
| 2 | systemd-boot ESP entries for A/B | done |
| 3 | Optional UKI (`*.efi` with kernel+initrd+cmdline) per slot | done — `./image/build-uki.sh`, ESP `EFI/Linux/` |
| 4 | Enroll PK/KEK/db in OVMF / firmware; reject unsigned UKI | done — keygen + signed UKI + `scripts/enroll-ovmf-vars.sh` |
| 5 | TPM PCR attestation of boot chain (optional) | done (lab) — sysfs PCR read; `Attest` RPC / `pertiskctl attest`; QEMU `PERTISK_TPM=1` |

See [SECURE_BOOT.md](./SECURE_BOOT.md) for build/sign/enroll steps.

## Image profiles

| Profile | Flag | Shell | Use |
|---------|------|-------|-----|
| `production` (default) | `make build` / `PERTISK_IMAGE_PROFILE=production` | none (`/bin/busybox` absent) | Releases, cloud images |
| `debug` | `make build PROFILE=debug` | BusyBox `ash` at `/bin/sh` | Lab recovery only; ship/sign separately |

Marker file in the image: `/etc/pertisk/image-profile`.

## Gaps tracked for later

- TPM2 Quote / AK enrollment + remote attestation verifier
- BusyBox `mount` / `umount` / `ip` applets (DHCP lease path is already in-process)
