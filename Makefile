CARGO?=cargo
TAR?=tar
DOCKER?=docker
PACKAGE=fw-if-discovery
BINARY=fw-if-discover
DOCKER_IMAGE=fw-rust-bookworm
DOCKER_PLATFORM_FLAG=$(if $(DOCKER_PLATFORM),--platform $(DOCKER_PLATFORM),)
VERSION=$(shell $(CARGO) metadata --no-deps --format-version 1 | sed -n 's/.*"name":"$(PACKAGE)","version":"\([^"]*\)".*/\1/p')
RUST_TARGET?=$(shell rustc -vV | sed -n 's/^host: //p')
TARGET_DIR=$(shell $(CARGO) metadata --no-deps --format-version 1 | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
RELEASE_DIR=$(TARGET_DIR)/$(RUST_TARGET)/release
PACKAGE_NAME=$(BINARY)-$(VERSION)-$(RUST_TARGET)

#:help: help          | Displays the Makefile help
.PHONY: help
help: ; @sed -n 's/^#:help://p' Makefile

#:help: all           | Cleans, tests, then builds debug and optimized release binaries
.PHONY: all
all: clean
	$(MAKE) test && $(MAKE) build && $(MAKE) release

#:help: clean         | Removes Cargo build artifacts and release packages
.PHONY: clean
clean:
	$(CARGO) clean
	rm -rf dist

#:help: precommit     | Runs formatting, Clippy, and workspace tests
.PHONY: precommit
precommit:
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --workspace --all-targets --locked -- -D warnings
	$(CARGO) test --workspace --locked

#:help: test          | Runs all workspace tests
.PHONY: test
test:
	$(CARGO) test --workspace --locked

#:help: build         | Builds unoptimized workspace binaries and libraries
.PHONY: build
build:
	$(CARGO) build --workspace --locked

#:help: run           | Runs fw-if-discover; pass arguments with ARGS='...'
.PHONY: run
run:
	$(CARGO) run --locked -p $(PACKAGE) -- $(ARGS)

#:help: release       | Builds the optimized executable for RUST_TARGET
.PHONY: release
release:
	$(CARGO) build --release --locked --target $(RUST_TARGET) -p $(PACKAGE)

#:help: package       | Bundles the release executable, crate README, and license in dist/*.tar.gz
.PHONY: package
package: release
	mkdir -p dist
	set -eu; \
		stage=$$(mktemp -d); \
		trap 'rm -rf "$$stage"' EXIT; \
		mkdir -p "$$stage/$(BINARY)"; \
		cp "$(RELEASE_DIR)/$(BINARY)" "$$stage/$(BINARY)/"; \
		cp "crates/$(PACKAGE)/README.md" LICENSE "$$stage/$(BINARY)/"; \
		$(TAR) -czf "dist/$(PACKAGE_NAME).tar.gz" -C "$$stage" "$(BINARY)"

#:help: docker-package | Builds a Linux package in dist/ using Docker
.PHONY: docker-package
docker-package:
	$(DOCKER) build $(DOCKER_PLATFORM_FLAG) --tag $(DOCKER_IMAGE) .
	$(DOCKER) run --rm \
		$(DOCKER_PLATFORM_FLAG) \
		--user "$$(id -u):$$(id -g)" \
		--env CARGO_HOME=/tmp/cargo \
		--volume "$(CURDIR):/workspace" \
		--workdir /workspace \
		$(DOCKER_IMAGE) make package

#:help: docker-packages | Builds Linux packages for amd64 and arm64
.PHONY: docker-packages
docker-packages:
	$(MAKE) docker-package DOCKER_PLATFORM=linux/amd64 && \
		$(MAKE) docker-package DOCKER_PLATFORM=linux/arm64
