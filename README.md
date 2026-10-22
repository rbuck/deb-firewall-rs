# fw

`fw` is a Rust-based project for setting up and managing Debian firewall
routers. It is intended to grow into a multi-crate workspace for the tools and
services involved in that system.

## Crates

- [`fw-if-discovery`](crates/fw-if-discovery/README.md) discovers Linux network
  interfaces and writes a versioned JSON facts document; its executable is
  `fw-if-discover`.
- [`fw-if-resolver`](crates/fw-if-resolver/README.md) combines discovered
  facts with network intent; its executable is `fw-if-resolve`.

## Workspace

Workspace package properties and shared dependencies are defined in the root
`Cargo.toml`.

## Versioning

This workspace follows [Semantic Versioning](https://semver.org/). Release
versions use `MAJOR.MINOR.PATCH`: incompatible changes increment MAJOR,
backward-compatible functionality increments MINOR, and backward-compatible
bug fixes increment PATCH. Workspace crate versions are kept aligned.

## Build and test

From this directory:

```sh
make help
make test
make build
make release
```

- `make build` builds all workspace crates in the unoptimized profile.
- `make release` builds the optimized `fw-if-discover` executable for the
  selected Rust target (the current host target by default).
- `make all` cleans build and package artifacts, runs workspace tests, then
  creates debug and optimized builds.
- `make precommit` checks formatting, runs Clippy with warnings denied, and
  runs workspace tests.

The integration tests run the built command against temporary mock sysfs
attributes and a mock `ip` executable. They do not need root access, Linux
network interfaces, or Docker.

## Cross-build and package

`make package` builds both release executables and creates separate tar.gz
archives under `dist/`:

- `fw-if-discover-<version>-<rust-target>.tar.gz`
- `fw-if-resolve-<version>-<rust-target>.tar.gz`

Each archive contains its executable, the corresponding crate README, and the
MIT `LICENSE`.

To build for another Rust target, install the target with `rustup` and pass it
to Make:

```sh
rustup target add x86_64-unknown-linux-musl
make package RUST_TARGET=x86_64-unknown-linux-musl
```

The Rust target must be supported by the installed toolchain and its
dependencies. Cross-compilation does not guarantee that the resulting binary
can run on every distribution; consider the target's C runtime and runtime
libraries when choosing a target.

### Build Linux packages with Docker

The Docker build uses the official Rust Bookworm image and GNU Make. It builds
inside a Linux container and writes the archive to the host's `dist/`
directory:

```sh
make docker-package
```

Choose a Docker platform explicitly when cross-building:

```sh
make docker-package DOCKER_PLATFORM=linux/amd64
make docker-package DOCKER_PLATFORM=linux/arm64
```

Build packages for both supported Linux architectures in sequence with:

```sh
make docker-packages
```

Docker may need emulation support enabled when its requested platform differs
from the host architecture. These package tasks build the executable; they do
not run it or execute the test suite inside the target container.

## Tagged releases

Pushing a version tag such as `v0.1.0` triggers the GitHub Actions release
workflow. It checks that the tag matches the workspace package version, runs
the precommit checks, builds both crate packages for each configured Rust
target, generates `SHA256SUMS.txt`, and attaches the packages to a GitHub
Release.

Tags should be treated as immutable. Fetch remote tags and push only the
specific new release tag:

```sh
git fetch --tags origin
git tag --list
```

For example, to release version `0.3.0`:

1. Select the next version according to Semantic Versioning, update `version`
   in the root `Cargo.toml`, update `Cargo.lock` as needed, then run
   `make precommit` and review the changes.
2. Commit and push the release changes.
3. Create and push an annotated tag:

   ```sh
   git tag -a v0.3.0 -m "Release v0.3.0"
   git push origin v0.3.0
   ```

4. Check the Actions run and confirm both crate archives for every configured
   target, along with the checksum file, are attached to the release.

Do not use `git push --tags` for a release, since it can publish other local
tags unintentionally. If a release build fails or the tag version is wrong,
fix the source and use a new version and tag rather than moving the existing
tag.

## Known issues

### Scaling CLI build and packaging tasks

The Makefile currently keeps a `CLI_TARGETS` registry mapping each Cargo
package to its executable name. GNU Make templates generate that CLI's
release-build and package targets, so adding a command requires one registry
entry rather than a copied set of recipes. Keep the package and binary names
in sync with the corresponding crate manifest.

If the build setup grows, consider these alternatives:

1. **Keep the Makefile registry and templates.** This is the current approach.
   It is simple and keeps Make as the interface, but the registry duplicates
   package/binary metadata from Cargo manifests.
2. **Use Cargo metadata as the registry.** Store or infer the CLI executable
   metadata in crate manifests and have a small helper read `cargo metadata`.
   This avoids duplicate lists but adds metadata conventions and parsing logic.
3. **Add a Rust `xtask` package.** Move release packaging and target selection
   into a workspace tool, with Make targets as thin wrappers. This offers more
   room for complex release workflows, but is more infrastructure than needed
   for the current build-and-archive tasks.

The current recommendation is to use the Makefile template until packaging
needs become substantially more complex; then reassess a metadata-driven
registry or `xtask`.
