# Pertisk KOS — top-level build helpers
#
#   make build                         # initramfs, default VERSION + ARCH=amd64
#   make build VERSION=0.2.0 ARCH=arm64
#   make build PROFILE=debug                 # BusyBox ash recovery image
#   make build EMBED_BOOT=1 EMBED_RUNTIME=1
#   make build-all VERSION=0.2.0       # amd64 + arm64
#   make build-host VERSION=0.2.0      # host cargo release bins
#   make mgmt                               # management UI+API → out/bin/pertisk-mgmt
#   make mgmt-pkg / make release           # DEB+RPM amd64/arm64 → out/pkg/
#   make mgmt-rpm / make rpm               # linux/amd64 DEB+RPM (lab)
#   make cloud VERSION=0.2.0 ARCH=amd64
#   make os-trust                          # Ed25519 keys → out/secrets/os-trust.{sk,pk}
#   make os-bundle VERSION=0.2.0 ARCH=amd64  # signed A/B OS zip for Upgrade tab
#
# VERSION embeds into binaries via PERTISK_BUILD_VERSION.
# ARCH is amd64 | arm64 (aliases: x86_64 → amd64, aarch64 → arm64).

SHELL := /usr/bin/env bash
.SHELLFLAGS := -euo pipefail -c

ROOT := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))

# Default version from workspace Cargo.toml.
VERSION ?= $(shell sed -n 's/^version = "\(.*\)"/\1/p' $(ROOT)/Cargo.toml | head -1)
ARCH ?= amd64

# Normalize ARCH aliases (command-line ARCH= cannot be reassigned).
ifeq ($(filter $(ARCH),amd64 x86_64),$(ARCH))
  BUILD_ARCH := amd64
else ifeq ($(filter $(ARCH),arm64 aarch64),$(ARCH))
  BUILD_ARCH := arm64
else
  $(error unsupported ARCH=$(ARCH); use amd64 or arm64)
endif

PLATFORM := linux/$(BUILD_ARCH)

EMBED_BOOT ?= 0
EMBED_RUNTIME ?= 0
# production (default): no interactive shell. debug: BusyBox ash.
PROFILE ?= production
ifeq ($(filter $(PROFILE),production debug),)
  $(error unsupported PROFILE=$(PROFILE); use production or debug)
endif

.PHONY: help build build-host build-all initramfs pertiskctl cloud uki enroll-ovmf \
	fetch-runtime fetch-kernel test fmt clippy check check-hardening clean version lab-up \
	mgmt mgmt-ui mgmt-pkg mgmt-rpm rpm release os-trust os-bundle \
	create-tag delete-tag retag clean-tag

help:
	@echo "Pertisk KOS make targets"
	@echo ""
	@echo "  make build [VERSION=...] [ARCH=amd64|arm64] [PROFILE=production|debug] [EMBED_BOOT=1] [EMBED_RUNTIME=1]"
	@echo "  make build-all [VERSION=...] [PROFILE=...] [EMBED_BOOT=1] [EMBED_RUNTIME=1]"
	@echo "  make build-host [VERSION=...]          # cargo release (host)"
	@echo "  make pertiskctl [VERSION=...]          # host CLI → out/bin/pertiskctl"
	@echo "  make mgmt [VERSION=...]                # management API+UI → out/bin/pertisk-mgmt"
	@echo "  make mgmt-ui                           # build React UI into crates/pertisk-mgmt/static"
	@echo "  make mgmt-pkg [VERSION=...]            # DEB+RPM amd64+arm64 → out/pkg/"
	@echo "  make mgmt-rpm [VERSION=...]            # linux/amd64 DEB+RPM (lab) → out/pkg/"
	@echo "  make rpm                               # alias for mgmt-rpm"
	@echo "  make release [VERSION=...]             # DEB+RPM amd64/arm64 for GitHub Release"
	@echo "  make create-tag TAG=0.1.10             # push tag (triggers .github/workflows/release.yml)"
	@echo "  make cloud [VERSION=...] [ARCH=...]"
	@echo "  make os-trust                         # Ed25519 os-trust.{sk,pk} → out/secrets/"
	@echo "  make os-bundle [VERSION=...] [ARCH=...]  # signed A/B OS zip (not Kubernetes)"
	@echo "  make stage-images [DEST=out]           # cloud + *-50g/*-75g qcow2 for RPM mgmt"
	@echo "  make deploy-lab MGMT=user@host PVE=ip  # build→RPM→images→mgmt (see script)"
	@echo "  make uki [VERSION=...] [ARCH=...]     # Unified Kernel Image"
	@echo "  make enroll-ovmf [ARCH=...]           # enroll SB keys into OVMF vars (lab)"
	@echo "  make lab-up [ARCH=...]                # build→VMs→IPs→cluster→CNI (see script)"
	@echo "  make test | check-hardening | fmt | clippy | clean"
	@echo ""
	@echo "Current: VERSION=$(VERSION) ARCH=$(BUILD_ARCH) PLATFORM=$(PLATFORM) PROFILE=$(PROFILE)"

version:
	@echo "$(VERSION)"

## Build initramfs for ARCH with embedded VERSION.
build: initramfs

initramfs:
	@echo "==> make initramfs VERSION=$(VERSION) ARCH=$(BUILD_ARCH) PROFILE=$(PROFILE)"
	PERTISK_VERSION="$(VERSION)" \
	PERTISK_PLATFORM="$(PLATFORM)" \
	PERTISK_ARCH="$(BUILD_ARCH)" \
	PERTISK_IMAGE_PROFILE="$(PROFILE)" \
	PERTISK_EMBED_BOOT="$(EMBED_BOOT)" \
	PERTISK_EMBED_RUNTIME="$(EMBED_RUNTIME)" \
	  "$(ROOT)/image/build-initramfs.sh"

## Build both architectures.
build-all:
	$(MAKE) build VERSION="$(VERSION)" ARCH=amd64 PROFILE="$(PROFILE)" EMBED_BOOT="$(EMBED_BOOT)" EMBED_RUNTIME="$(EMBED_RUNTIME)"
	$(MAKE) build VERSION="$(VERSION)" ARCH=arm64 PROFILE="$(PROFILE)" EMBED_BOOT="$(EMBED_BOOT)" EMBED_RUNTIME="$(EMBED_RUNTIME)"
	@echo "==> multi-arch artifacts"
	@ls -lh "$(ROOT)/out"/initramfs-*.cpio.gz

## Host cargo release binaries with VERSION override.
build-host:
	@echo "==> cargo release VERSION=$(VERSION)"
	PERTISK_BUILD_VERSION="$(VERSION)" cargo build --release --workspace
	@mkdir -p "$(ROOT)/out/bin"
	@cp -f "$(ROOT)/target/release/pertiskd" "$(ROOT)/out/bin/pertiskd" 2>/dev/null || true
	@cp -f "$(ROOT)/target/release/pertiskctl" "$(ROOT)/out/bin/pertiskctl" 2>/dev/null || true
	@cp -f "$(ROOT)/target/release/pertisk-mgmt" "$(ROOT)/out/bin/pertisk-mgmt" 2>/dev/null || true
	@echo "==> host bins in out/bin/ (if built)"

## Host CLI only (management client).
pertiskctl:
	@echo "==> build pertiskctl VERSION=$(VERSION)"
	PERTISK_BUILD_VERSION="$(VERSION)" cargo build --release -p pertiskctl
	@mkdir -p "$(ROOT)/out/bin"
	@cp -f "$(ROOT)/target/release/pertiskctl" "$(ROOT)/out/bin/pertiskctl"
	@ls -lh "$(ROOT)/out/bin/pertiskctl"
	@echo "==> $(ROOT)/out/bin/pertiskctl"

## React management UI → crates/pertisk-mgmt/static (embedded by pertisk-mgmt).
mgmt-ui:
	@echo "==> build mgmt-ui VERSION=$(VERSION)"
	cd "$(ROOT)/web/mgmt-ui" && npm install && VITE_APP_VERSION="$(VERSION)" npm run build
	@rm -rf "$(ROOT)/crates/pertisk-mgmt/static"
	@mkdir -p "$(ROOT)/crates/pertisk-mgmt/static"
	@cp -R "$(ROOT)/web/mgmt-ui/dist/." "$(ROOT)/crates/pertisk-mgmt/static/"
	@echo "==> UI assets in crates/pertisk-mgmt/static"

## Management API + embedded UI (single port).
mgmt: mgmt-ui
	@echo "==> build pertisk-mgmt VERSION=$(VERSION)"
	PERTISK_BUILD_VERSION="$(VERSION)" cargo build --release -p pertisk-mgmt
	@mkdir -p "$(ROOT)/out/bin"
	@cp -f "$(ROOT)/target/release/pertisk-mgmt" "$(ROOT)/out/bin/pertisk-mgmt"
	@ls -lh "$(ROOT)/out/bin/pertisk-mgmt"
	@echo "==> $(ROOT)/out/bin/pertisk-mgmt"

## linux packages (DEB + RPM). Override: make mgmt-pkg PKG_PLATFORMS=linux/amd64
PKG_PLATFORMS ?= linux/amd64,linux/arm64
mgmt-pkg:
	PKG_PLATFORMS="$(PKG_PLATFORMS)" VERSION="$(VERSION)" "$(ROOT)/scripts/build-mgmt-pkg.sh"

## linux/amd64 DEB+RPM only (lab deploy). Full matrix: make mgmt-pkg
mgmt-rpm:
	PKG_PLATFORMS=linux/amd64 VERSION="$(VERSION)" "$(ROOT)/scripts/build-mgmt-pkg.sh"

## Alias: package web/API for amd64 deploy.
rpm: mgmt-rpm

## Release artifacts: DEB+RPM for linux/amd64 and linux/arm64 (CI on tag X.Y.Z).
## Cut a release with: make create-tag TAG=0.1.10
release: mgmt-pkg
	@shopt -s nullglob; \
	  pkgs=( "$(ROOT)/out/pkg"/pertisk-mgmt*.rpm "$(ROOT)/out/pkg"/pertisk-mgmt*.deb ); \
	  [[ $${#pkgs[@]} -ge 4 ]] || { echo "ERROR: expected DEB+RPM for amd64 and arm64 in out/pkg/" >&2; ls -la "$(ROOT)/out/pkg" >&2; exit 1; }; \
	  echo "==> release VERSION=$(VERSION)"; \
	  ls -lh "$(ROOT)/out/pkg"/pertisk-mgmt*.rpm "$(ROOT)/out/pkg"/pertisk-mgmt*.deb

## Cloud golden disk (kernel + systemd-boot + containerd/kubelet in initramfs).
cloud:
	@echo "==> cloud image VERSION=$(VERSION) ARCH=$(BUILD_ARCH)"
	$(MAKE) fetch-runtime ARCH="$(BUILD_ARCH)"
	$(MAKE) build VERSION="$(VERSION)" ARCH="$(BUILD_ARCH)" EMBED_BOOT=1 EMBED_RUNTIME=1
	PERTISK_VERSION="$(VERSION)" PERTISK_ARCH="$(BUILD_ARCH)" "$(ROOT)/image/build-cloud-image.sh"

OS_TRUST_SK ?= $(ROOT)/out/secrets/os-trust.sk
OS_TRUST_PK ?= $(ROOT)/out/secrets/os-trust.pk

## Generate Ed25519 OS trust keys (once). Copy .pk to STATE/secrets/os-trust.pk on nodes.
## Does not overwrite existing keys unless FORCE=1.
os-trust:
	@mkdir -p "$(dir $(OS_TRUST_SK))"
	@if [[ -f "$(OS_TRUST_SK)" || -f "$(OS_TRUST_PK)" ]] && [[ "$(FORCE)" != "1" ]]; then \
	  echo "keys already exist: $(OS_TRUST_SK) $(OS_TRUST_PK)"; \
	  echo "re-run with FORCE=1 to replace (nodes still on the old .pk cannot verify new bundles)"; \
	  ls -lh "$(OS_TRUST_SK)" "$(OS_TRUST_PK)" 2>/dev/null || true; \
	else \
	  echo "==> pertisk-sign keygen → $(OS_TRUST_PK)"; \
	  PERTISK_BUILD_VERSION="$(VERSION)" cargo build --release -p pertisk-update --bin pertisk-sign; \
	  "$(ROOT)/target/release/pertisk-sign" keygen --secret "$(OS_TRUST_SK)" --public "$(OS_TRUST_PK)"; \
	  chmod 600 "$(OS_TRUST_SK)"; \
	  echo "==> keep $(OS_TRUST_SK) offline; install $(OS_TRUST_PK) as STATE/secrets/os-trust.pk"; \
	fi

## Signed A/B OS bundle: kernel, initramfs, manifest.json, manifest.sig (or a .zip of those files).
## Kubernetes is not changed.
## Workers first, then control planes. Trust key os-trust.pk must already be on STATE.
## Recreating VMs from a new qcow2 is a reinstall, not this path.
##   make os-bundle VERSION=0.2.86 ARCH=amd64
##   make os-bundle SKIP_BUILD=1   # re-sign existing out/ kernel + initramfs
os-bundle:
	@echo "==> OS A/B bundle VERSION=$(VERSION) ARCH=$(BUILD_ARCH) PROFILE=$(PROFILE)"
	@if [[ "$(SKIP_BUILD)" != "1" ]]; then \
	  $(MAKE) fetch-runtime ARCH="$(BUILD_ARCH)"; \
	  $(MAKE) build VERSION="$(VERSION)" ARCH="$(BUILD_ARCH)" PROFILE="$(PROFILE)" EMBED_BOOT=1 EMBED_RUNTIME=1; \
	fi
	PERTISK_VERSION="$(VERSION)" \
	PERTISK_ARCH="$(BUILD_ARCH)" \
	PERTISK_IMAGE_PROFILE="$(PROFILE)" \
	OS_TRUST_SK="$(OS_TRUST_SK)" \
	OS_TRUST_PK="$(OS_TRUST_PK)" \
	SKIP_BUILD="$(SKIP_BUILD)" \
	  "$(ROOT)/scripts/build-os-bundle.sh"

## Stage base + role-sized qcow2 for RPM mgmt (/var/lib/pertisk-mgmt/images).
## Optional: make stage-images DEST=/tmp/images
stage-images:
	ARCH="$(BUILD_ARCH)" DEST="$(DEST)" "$(ROOT)/scripts/stage-cloud-images.sh"

## Local build → RPM on mgmt → copy images to mgmt (create pushes to Proxmox).
##   make deploy-lab MGMT=user@mgmt.example.com PVE=pve.example.com VERSION=0.3.0
deploy-lab:
	@[[ -n "$(MGMT)" ]] || { echo "set MGMT=user@host" >&2; exit 1; }
	"$(ROOT)/scripts/deploy-mgmt-lab.sh" --mgmt "$(MGMT)" \
		$(if $(PVE),--pve "$(PVE)",) \
		$(if $(VERSION),--version "$(VERSION)",)

## Unified Kernel Image (requires kernel + initramfs artifacts).
uki:
	@echo "==> UKI VERSION=$(VERSION) ARCH=$(BUILD_ARCH)"
	PERTISK_VERSION="$(VERSION)" PERTISK_ARCH="$(BUILD_ARCH)" "$(ROOT)/image/build-uki.sh"

## Enroll lab Secure Boot keys into an OVMF/AAVMF vars template (needs virt-fw-vars).
enroll-ovmf:
	PERTISK_ARCH="$(BUILD_ARCH)" "$(ROOT)/scripts/enroll-ovmf-vars.sh"

fetch-runtime:
	PERTISK_ARCH="$(BUILD_ARCH)" "$(ROOT)/image/fetch-runtime.sh"

fetch-kernel:
	PERTISK_ARCH="$(BUILD_ARCH)" "$(ROOT)/image/fetch-kernel.sh"

## Full lab: cloud image → Proxmox VMs → DHCP IPs → bootstrap → join → CNI.
## Extra flags: make lab-up ARGS='--skip-build --cni cilium --workers 2'
lab-up:
	ARCH="$(BUILD_ARCH)" "$(ROOT)/scripts/proxmox-lab-up.sh" $(ARGS)

test:
	cargo test --workspace

check-hardening:
	"$(ROOT)/scripts/check-hardening.sh"

fmt:
	cargo fmt --all

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

check: fmt clippy test check-hardening

clean:
	cargo clean
	rm -rf "$(ROOT)/out"/.initramfs-tmp-*
	@echo "cleaned cargo target + initramfs temps (out/ artifacts kept; rm -rf out to wipe)"


# Delete a tag (local and remote).
delete-tag:
ifndef TAG
	$(error TAG is not set. Usage: make delete-tag TAG=0.1.10)
endif
	@echo "Deleting tag $(TAG)..."
	git tag -d $(TAG)
	git push origin -d $(TAG)

# Create a new tag.
create-tag:
ifndef TAG
	$(error TAG is not set. Usage: make create-tag TAG=0.1.10)
endif
	@echo "Creating tag $(TAG)..."
	git tag $(TAG)
	git push origin $(TAG)

# Delete and recreate a tag (force update). Use after amending a release commit.
# Usage: make retag TAG=0.1.10
retag:
ifndef TAG
	$(error TAG is not set. Usage: make retag TAG=0.1.10)
endif
	@echo "Recreating tag $(TAG)..."
	@echo "Deleting local tag (if exists)..."
	-git tag -d $(TAG) 2>/dev/null || true
	@echo "Deleting remote tag (if exists)..."
	-git push origin -d $(TAG) 2>/dev/null || true
	@echo "Creating new tag $(TAG)..."
	git tag $(TAG)
	@echo "Pushing tag $(TAG) to origin..."
	git push origin $(TAG)
	@echo "✓ Tag $(TAG) created and pushed successfully"

clean-tag: retag