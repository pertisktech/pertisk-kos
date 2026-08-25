# Pertisk KOS — Production Release Checklist

Use this checklist before cutting a production tag.

## 1. Security and hardening gates

- [ ] `make check-hardening` passes with zero failures.
- [ ] `make check` passes (`fmt`, `clippy`, `test`, `check-hardening`).
- [ ] Management API and metrics are configured for mTLS in production (`PERTISK_TLS_*`).
- [ ] `cluster.ca` is present in all machine configs (no insecure-skip fallback).

## 2. Dependency freshness

- [ ] Kernel/userspace baseline is not EOL.
- [ ] `docs/PACKAGE.md` reflects current pinned versions and any known CVE-related constraints.
- [ ] Runtime component versions are verified for target Kubernetes minor compatibility.

## 3. Artifact integrity and signing

- [ ] OS signing keypair strategy is confirmed (`make os-trust`, private key offline).
- [ ] Release bundles are signed and include trust public key.
- [ ] Build provenance and SBOM generation completed (`scripts/generate-sbom.sh`).

## 4. HA and upgrade validation

- [ ] Rolling Kubernetes upgrade validated on a 3-control-plane HA cluster.
- [ ] Rolling OS A/B upgrade validated (workers first, then control planes).
- [ ] Reboot/failover drill validated for VIP and etcd quorum behavior.
- [ ] Recovery path validated (`etcd snapshot/restore`, node reset workflow).

## 5. Platform scope and docs

- [ ] Supported platforms and limitations are clearly documented in `docs/COMPATIBILITY.md`.
- [ ] Any lab-only features are clearly marked and excluded from production claims.
- [ ] Deployment runbook is up to date (`docs/DEPLOY.md`, provider docs).

## 6. Release execution

- [ ] Build release artifacts (`make release` and `make guest-release`).
- [ ] Verify package and image artifacts are present in `out/pkg`.
- [ ] Tag only after all checklist items are complete.

## Suggested release gate command

```bash
make check && make release && make guest-release
```

If any item fails, do not ship as production.
