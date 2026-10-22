# fw-if-discover

A command-line application for discovering Linux network-interface facts and
writing a versioned JSON inventory.

The application reads interface attributes from `/sys/class/net`, uses
`iproute2` to collect IP addresses, and optionally uses `ethtool` as a link
speed fallback. It observes the system; it does not configure networking.

See the [workspace README](../../README.md) for workspace-wide build, test,
cross-build, packaging, and release instructions.

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

## Known limitations

- The application discovers interfaces using Linux-specific sysfs and
  `iproute2`; macOS packages can be cross-built, but the application itself
  requires Linux to run.
- Docker-based packages use the Docker image's default Rust target for the
  selected platform.
