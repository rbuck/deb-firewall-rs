CARGO?=cargo
TAR?=tar
DOCKER?=docker
CLI_TARGETS=fw-if-discovery=fw-if-discover fw-if-resolver=fw-if-resolve
DOCKER_IMAGE=fw-rust-bookworm
DOCKER_PLATFORM_FLAG=$(if $(DOCKER_PLATFORM),--platform $(DOCKER_PLATFORM),)
cli_package=$(firstword $(subst =, ,$(1)))
cli_binary=$(word 2,$(subst =, ,$(1)))
CLI_PACKAGES=$(foreach cli,$(CLI_TARGETS),$(call cli_package,$(cli)))
VERSION=$(shell $(CARGO) metadata --no-deps --format-version 1 | sed -n 's/.*"name":"$(call cli_package,$(firstword $(CLI_TARGETS)))","version":"\([^"]*\)".*/\1/p')
RUST_TARGET?=$(shell rustc -vV | sed -n 's/^host: //p')
TARGET_DIR=$(shell $(CARGO) metadata --no-deps --format-version 1 | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
RELEASE_DIR=$(TARGET_DIR)/$(RUST_TARGET)/release

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
	$(CARGO) run --locked -p $(call cli_package,$(firstword $(CLI_TARGETS))) --bin $(call cli_binary,$(firstword $(CLI_TARGETS))) -- $(ARGS)

#:help: release       | Builds all optimized CLI executables for RUST_TARGET
.PHONY: release
release: $(addprefix release-,$(CLI_PACKAGES))

#:help: package       | Bundles all release executables, crate READMEs, and license in dist/*.tar.gz
.PHONY: package
package: $(addprefix package-,$(CLI_PACKAGES))

define CLI_RULES
.PHONY: release-$(call cli_package,$(1)) package-$(call cli_package,$(1))
release-$(call cli_package,$(1)):
	$$(CARGO) build --release --locked --target $$(RUST_TARGET) -p $(call cli_package,$(1))

package-$(call cli_package,$(1)): release-$(call cli_package,$(1))
	mkdir -p dist
	set -eu; \
		stage=$$$$(mktemp -d); \
		trap 'rm -rf "$$$$stage"' EXIT; \
		mkdir -p "$$$$stage/$(call cli_binary,$(1))"; \
		cp "$$(RELEASE_DIR)/$(call cli_binary,$(1))" "$$$$stage/$(call cli_binary,$(1))/"; \
		cp "crates/$(call cli_package,$(1))/README.md" LICENSE "$$$$stage/$(call cli_binary,$(1))/"; \
		$$(TAR) -czf "dist/$(call cli_binary,$(1))-$$(VERSION)-$$(RUST_TARGET).tar.gz" -C "$$$$stage" "$(call cli_binary,$(1))"
endef

$(foreach cli,$(CLI_TARGETS),$(eval $(call CLI_RULES,$(cli))))

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
