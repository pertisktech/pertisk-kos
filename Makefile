# Pertisk KOS — top-level build helpers
#
#   make build                         # initramfs, default VERSION + ARCH=amd64
#   make build VERSION=0.2.0 ARCH=arm64
#   make build PROFILE=debug                 # BusyBox ash recovery image
#   make build EMBED_BOOT=1 EMBED_RUNTIME=1
#   make build-all VERSION=0.2.0       # amd64 + arm64
#   make build-host VERSION=0.2.0      # host cargo release bins
#   make mgmt                               # management UI+API → out/bin/pertisk-mgmt
#   make mgmt-rpm / make rpm                # linux/amd64 RPM → out/rpm/
#   make cloud VERSION=0.2.0 ARCH=amd64
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

.PHONY: help build build-host build-all initramfs pertiskctl cloud uki \
	fetch-runtime fetch-kernel test fmt clippy check check-hardening clean version lab-up \
	mgmt mgmt-ui mgmt-rpm rpm

help:
	@echo "Pertisk KOS make targets"
	@echo ""
	@echo "  make build [VERSION=...] [ARCH=amd64|arm64] [PROFILE=production|debug] [EMBED_BOOT=1] [EMBED_RUNTIME=1]"
	@echo "  make build-all [VERSION=...] [PROFILE=...] [EMBED_BOOT=1] [EMBED_RUNTIME=1]"
	@echo "  make build-host [VERSION=...]          # cargo release (host)"
	@echo "  make pertiskctl [VERSION=...]          # host CLI → out/bin/pertiskctl"
	@echo "  make mgmt [VERSION=...]                # management API+UI → out/bin/pertisk-mgmt"
	@echo "  make mgmt-ui                           # build React UI into crates/pertisk-mgmt/static"
	@echo "  make mgmt-rpm [VERSION=...]            # linux/amd64 RPM (API+UI) → out/rpm/"
	@echo "  make rpm                               # alias for mgmt-rpm (amd64 deploy)"
	@echo "  make cloud [VERSION=...] [ARCH=...]"
	@echo "  make uki [VERSION=...] [ARCH=...]     # Unified Kernel Image"
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

## linux/amd64 RPM of management API+UI (Docker buildx; for RHEL/Rocky/Alma deploy).
mgmt-rpm:
	VERSION="$(VERSION)" "$(ROOT)/scripts/build-mgmt-rpm.sh"

## Alias: package web/API for amd64 deploy.
rpm: mgmt-rpm

## Cloud golden disk (kernel + systemd-boot + containerd/kubelet in initramfs).
cloud:
	@echo "==> cloud image VERSION=$(VERSION) ARCH=$(BUILD_ARCH)"
	$(MAKE) fetch-runtime ARCH="$(BUILD_ARCH)"
	$(MAKE) build VERSION="$(VERSION)" ARCH="$(BUILD_ARCH)" EMBED_BOOT=1 EMBED_RUNTIME=1
	PERTISK_VERSION="$(VERSION)" PERTISK_ARCH="$(BUILD_ARCH)" "$(ROOT)/image/build-cloud-image.sh"

## Unified Kernel Image (requires kernel + initramfs artifacts).
uki:
	@echo "==> UKI VERSION=$(VERSION) ARCH=$(BUILD_ARCH)"
	PERTISK_VERSION="$(VERSION)" PERTISK_ARCH="$(BUILD_ARCH)" "$(ROOT)/image/build-uki.sh"

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
