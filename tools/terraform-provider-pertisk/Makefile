VERSION ?= 0.1.0
OS_ARCH := $(shell go env GOOS)_$(shell go env GOARCH)
BIN := terraform-provider-pertisk
PLUGIN_DIR := $(HOME)/.terraform.d/plugins/registry.terraform.io/pertisk-tech/pertisk/$(VERSION)/$(OS_ARCH)

.PHONY: build install tidy test testacc fmt docs snapshot check trivy

tidy:
	go mod tidy

build: tidy
	go build -ldflags="-X main.version=$(VERSION)" -o bin/$(BIN) .

install: build
	mkdir -p "$(PLUGIN_DIR)"
	cp bin/$(BIN) "$(PLUGIN_DIR)/$(BIN)_v$(VERSION)"
	@echo "installed → $(PLUGIN_DIR)/$(BIN)_v$(VERSION)"

fmt:
	gofmt -w .

# Unit tests (always). Acceptance tests are skipped unless TF_ACC=1.
test:
	go test ./...

# Live Proxmox + pertisk-mgmt acceptance (3-node cluster). See README.
testacc:
	TF_ACC=1 go test ./internal/provider/ -run TestAcc -v -timeout 60m -count=1

# Scan from this directory so ./.trivyignore is picked up.
trivy:
	trivy fs . --scanners vuln

docs:
	go run github.com/hashicorp/terraform-plugin-docs/cmd/tfplugindocs generate -provider-name pertisk

check:
	goreleaser check

snapshot:
	goreleaser release --snapshot --clean --skip=sign,publish
