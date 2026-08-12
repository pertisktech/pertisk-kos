VERSION ?= 0.1.0
OS_ARCH := $(shell go env GOOS)_$(shell go env GOARCH)
BIN := terraform-provider-pertisk
PLUGIN_DIR := $(HOME)/.terraform.d/plugins/registry.terraform.io/pertisk-tech/pertisk/$(VERSION)/$(OS_ARCH)

.PHONY: build install tidy test fmt snapshot check

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

test:
	go test ./...

# Requires goreleaser (https://goreleaser.com/install/)
check:
	goreleaser check

# Local unsigned multi-arch build (no GitHub release / no GPG)
snapshot:
	goreleaser release --snapshot --clean --skip=sign,publish
