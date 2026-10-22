# fw-if-discover

A Rust workspace for discovering Linux network-interface facts and writing a
versioned JSON inventory. The workspace currently contains the
`fw-if-discover` command-line application.

The application reads interface attributes from `/sys/class/net`, uses
`iproute2` to collect IP addresses, and optionally uses `ethtool` as a link
speed fallback. It observes the system; it does not configure networking.

## Workspace

- `crates/fw-if-discover`: discovers network interfaces and writes the
  `network_interfaces` facts document.

Workspace package properties and shared dependencies are defined in the root
`Cargo.toml`.

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

## Cross-build and package

`make package` builds a release executable and creates a tar.gz archive under
`dist/`. The archive is named
`fw-if-discover-<version>-<rust-target>.tar.gz` and contains a
`fw-if-discover/` directory with the executable, this README, and the MIT
`LICENSE`.

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

## Run the application

The application requires Linux network sysfs and `iproute2` at runtime. To
write to the default path (`/var/lib/spider/facts.d/interfaces.json`), use
suitable permissions:

```sh
sudo ./target/release/fw-if-discover
```

Use `--output` to select an exact output path, or pass a positional output
path:

```sh
./target/release/fw-if-discover --output /tmp/interfaces.json
./target/release/fw-if-discover /tmp/interfaces.json
```

`--prefix` redirects the default system output path under a directory. This
is useful for staging or testing without writing under `/var/lib`:

```sh
./target/release/fw-if-discover --prefix /tmp/fw-root
```

The command creates the output directory as needed and atomically replaces
the output file. The generated file is restricted to owner read/write
permissions.

## Facts document

The output includes a schema version, a UTC generation timestamp, and
interfaces sorted by name. Optional sysfs attributes are represented as JSON
`null` when unavailable. A document has this general shape:

```json
{
  "schema_version": 1,
  "facts_type": "network_interfaces",
  "generated_at": "2026-10-22T12:00:00Z",
  "interfaces": [
    {
      "name": "enp1s0",
      "mac_address": "00:11:22:33:44:55",
      "ifindex": 2,
      "type": "ether",
      "arphrd_type": 1,
      "admin_state": "up",
      "operstate": "up",
      "carrier": true,
      "speed_mbps": 1000,
      "mtu": 1500,
      "addresses": [
        {
          "family": "inet",
          "local": "192.168.1.10",
          "prefixlen": 24,
          "scope": "global"
        }
      ],
      "device": {
        "bus": "pci",
        "path": "0000:01:00.0",
        "vendor_id": "8086",
        "device_id": "1539",
        "driver": "igb"
      }
    }
  ]
}
```

Hardware-specific device fields and IP addresses depend on the interface and
the information available on the system.

## Tagged releases

Pushing a version tag such as `v0.1.0` triggers the GitHub Actions release
workflow. It checks that the tag matches the workspace package version, runs
the precommit checks, builds packages for 64-bit Linux x86_64 and ARM64 and
macOS x86_64 and ARM64, generates `SHA256SUMS.txt`, and attaches the packages
to a GitHub Release.

Tags should be treated as immutable. Fetch remote tags and push only the
specific new release tag:

```sh
git fetch --tags origin
git tag --list
```

For example, to release version `0.1.1`:

1. Update `version` in the root `Cargo.toml`, update `Cargo.lock` as needed,
   then run `make precommit` and review the changes.
2. Commit and push the release changes.
3. Create and push an annotated tag:

   ```sh
   git tag -a v0.1.1 -m "Release v0.1.1"
   git push origin v0.1.1
   ```

4. Check the Actions run and confirm the four archives and checksum file are
   attached to the release.

Do not use `git push --tags` for a release, since it can publish other local
tags unintentionally. If a release build fails or the tag version is wrong,
fix the source and use a new version and tag rather than moving the existing
tag.

## Known limitations

- The application discovers interfaces using Linux-specific sysfs and
  `iproute2`; macOS packages can be cross-built, but the application itself
  requires Linux to run.
- Docker-based packages use the Docker image's default Rust target for the
  selected platform.
- The workspace currently has no automated unit tests for discovery edge
  cases.
