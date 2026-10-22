use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

// Configuration
const DEFAULT_OUTPUT: &str = "/var/lib/spider/facts.d/interfaces.json";
const SCHEMA_VERSION: u32 = 1;

// System path resolution
struct Paths {
    prefix: Option<PathBuf>,
}

impl Paths {
    fn resolve(&self, absolute_path: &str) -> Result<PathBuf> {
        let path = Path::new(absolute_path);

        if !path.is_absolute() {
            bail!("Expected an absolute system path: {absolute_path}");
        }

        match &self.prefix {
            Some(prefix) => {
                let relative = path
                    .strip_prefix("/")
                    .context("Could not remove root from system path")?;

                Ok(prefix.join(relative))
            }
            None => Ok(path.to_path_buf()),
        }
    }
}

// Facts document schema
#[derive(Debug, Serialize)]
struct FactsDocument {
    schema_version: u32,
    facts_type: &'static str,
    generated_at: String,
    interfaces: Vec<NetworkInterface>,
}

#[derive(Debug, Serialize)]
struct NetworkInterface {
    name: String,
    mac_address: Option<String>,
    ifindex: Option<u32>,
    #[serde(rename = "type")]
    kind: String,
    arphrd_type: Option<u32>,
    admin_state: String,
    operstate: Option<String>,
    carrier: Option<bool>,
    speed_mbps: Option<u64>,
    mtu: Option<u32>,
    addresses: Vec<AddressInfo>,
    device: Option<Device>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AddressInfo {
    family: String,
    local: String,
    prefixlen: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    flags: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Device {
    bus: Option<String>,
    path: Option<String>,
    vendor_id: Option<String>,
    device_id: Option<String>,
    driver: Option<String>,
}

// iproute2 address response schema
#[derive(Debug, Deserialize)]
struct IpInterface {
    ifname: String,
    #[serde(default)]
    addr_info: Vec<IpAddress>,
}

#[derive(Debug, Deserialize)]
struct IpAddress {
    family: String,
    local: Option<String>,
    prefixlen: u32,
    scope: Option<String>,
    #[serde(default)]
    flags: Vec<String>,
}

// Sysfs readers and interface metadata
fn read_attr(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn read_number<T: std::str::FromStr>(path: &Path) -> Option<T> {
    read_attr(path)?.parse().ok()
}

fn sys_path(iface: &str) -> PathBuf {
    sysfs_net_root().join(iface)
}

fn sysfs_net_root() -> PathBuf {
    std::env::var_os("FW_IF_DISCOVER_SYSFS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/sys"))
        .join("class/net")
}

fn get_admin_state(iface: &str) -> String {
    // Linux IFF_UP is bit 0 in /sys/class/net/<iface>/flags.
    let Some(flags) = read_attr(&sys_path(iface).join("flags")) else {
        return "unknown".to_string();
    };

    let Some(flags) = flags
        .strip_prefix("0x")
        .and_then(|s| u32::from_str_radix(s, 16).ok())
    else {
        return "unknown".to_string();
    };

    if flags & 0x1 != 0 {
        "up".to_string()
    } else {
        "down".to_string()
    }
}

fn get_carrier(iface: &str) -> Option<bool> {
    let value: u32 = read_number(&sys_path(iface).join("carrier"))?;
    match value {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

fn normalize_id(value: Option<String>) -> Option<String> {
    let value = value?;
    let value = value.trim();
    let value = value.strip_prefix("0x").unwrap_or(value);
    if value.is_empty() {
        None
    } else {
        Some(value.to_ascii_lowercase())
    }
}

fn get_device(iface: &str) -> Option<Device> {
    let device_link = sys_path(iface).join("device");
    let resolved = fs::canonicalize(&device_link).ok()?;

    let path = resolved
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::to_string);

    let subsystem_path = device_link.join("subsystem");
    let bus = fs::canonicalize(subsystem_path)
        .ok()
        .and_then(|p| p.file_name().and_then(|s| s.to_str()).map(str::to_string));

    // PCI uses vendor/device; USB commonly uses idVendor/idProduct.
    let vendor_id = normalize_id(
        read_attr(&resolved.join("vendor")).or_else(|| read_attr(&resolved.join("idVendor"))),
    );

    let device_id = normalize_id(
        read_attr(&resolved.join("device")).or_else(|| read_attr(&resolved.join("idProduct"))),
    );

    let driver = fs::canonicalize(device_link.join("driver"))
        .ok()
        .and_then(|p| p.file_name().and_then(|s| s.to_str()).map(str::to_string));

    Some(Device {
        bus,
        path,
        vendor_id,
        device_id,
        driver,
    })
}

fn interface_type(iface: &str, arphrd_type: Option<u32>) -> String {
    let path = sys_path(iface);

    // Check specific interface kinds before generic device-backed Ethernet.
    if iface == "lo" {
        "loopback"
    } else if path.join("wireless").is_dir() {
        "wireless"
    } else if path.join("bridge").is_dir() {
        "bridge"
    } else if path.join("bonding").is_dir() {
        "bond"
    } else if path.join("device").exists() {
        if arphrd_type == Some(1) {
            "ether"
        } else {
            "device-backed"
        }
    } else if arphrd_type == Some(1) {
        "ether-virtual"
    } else {
        "other"
    }
    .to_string()
}

fn parse_speed(value: &str) -> Option<u64> {
    let value = value.trim();

    // Common ethtool formats: 1000Mb/s, 10000Mb/s, 10Gb/s.
    if let Some(number) = value.strip_suffix("Mb/s") {
        return number.trim().parse().ok();
    }

    if let Some(number) = value
        .strip_suffix("Gb/s")
        .or_else(|| value.strip_suffix("Gbps"))
    {
        return number
            .trim()
            .parse::<u64>()
            .ok()
            .and_then(|n| n.checked_mul(1000));
    }

    None
}

fn get_speed(iface: &str) -> Option<u64> {
    let speed_path = sys_path(iface).join("speed");

    if let Some(speed) = read_number::<i64>(&speed_path)
        && speed > 0
    {
        return Some(speed as u64);
    }

    // ethtool is an optional fallback.
    let output = Command::new("ethtool").arg(iface).output().ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);

    text.lines().find_map(|line| {
        let (key, value) = line.trim().split_once(':')?;
        if key.trim() == "Speed" {
            parse_speed(value.trim())
        } else {
            None
        }
    })
}

// iproute2 discovery
fn get_addresses() -> Result<HashMap<String, Vec<AddressInfo>>> {
    let output = Command::new("ip")
        .args(["-j", "address", "show"])
        .output()
        .context("Could not execute `ip -j address show`; install iproute2")?;

    if !output.status.success() {
        bail!(
            "`ip -j address show` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let links: Vec<IpInterface> = serde_json::from_slice(&output.stdout)
        .context("Could not parse JSON from `ip -j address show`")?;

    let mut result = HashMap::new();

    for link in links {
        let addresses = link
            .addr_info
            .into_iter()
            .filter_map(|address| {
                Some(AddressInfo {
                    family: address.family,
                    local: address.local?,
                    prefixlen: address.prefixlen,
                    scope: address.scope,
                    flags: address.flags,
                })
            })
            .collect();

        result.insert(link.ifname, addresses);
    }

    Ok(result)
}

// Inventory assembly
fn collect_interfaces() -> Result<Vec<NetworkInterface>> {
    let root = sysfs_net_root();

    let entries =
        fs::read_dir(&root).with_context(|| format!("Could not read {}", root.display()))?;

    let mut names = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect::<Vec<_>>();

    names.sort();

    let addresses = get_addresses()?;
    let mut interfaces = Vec::with_capacity(names.len());

    for name in names {
        let path = sys_path(&name);

        // An interface can disappear while the snapshot is being collected.
        if !path.exists() {
            continue;
        }

        let arphrd_type = read_number(&path.join("type"));
        let mac_address = read_attr(&path.join("address"));
        let ifindex = read_number(&path.join("ifindex"));
        let mtu = read_number(&path.join("mtu"));
        let operstate = read_attr(&path.join("operstate"));

        interfaces.push(NetworkInterface {
            kind: interface_type(&name, arphrd_type),
            name: name.clone(),
            mac_address,
            ifindex,
            arphrd_type,
            admin_state: get_admin_state(&name),
            operstate,
            carrier: get_carrier(&name),
            speed_mbps: get_speed(&name),
            mtu,
            addresses: addresses.get(&name).cloned().unwrap_or_default(),
            device: get_device(&name),
        });
    }

    if interfaces.is_empty() {
        bail!("No network interfaces were discovered");
    }

    Ok(interfaces)
}

// Atomic output
fn write_atomically(output: &Path, document: &FactsDocument) -> Result<()> {
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    fs::create_dir_all(parent).with_context(|| format!("Could not create {}", parent.display()))?;

    // Keep the temporary file on the destination filesystem so persist()
    // can replace the destination with a rename.
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("Could not create temporary file in {}", parent.display()))?;

    temp.as_file_mut()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .context("Could not set temporary file permissions")?;

    serde_json::to_writer_pretty(temp.as_file_mut(), document)
        .context("Could not serialize interface facts to JSON")?;

    temp.write_all(b"\n")
        .context("Could not finish writing JSON")?;

    temp.as_file_mut()
        .sync_all()
        .context("Could not sync temporary file")?;

    temp.persist(output)
        .map_err(|e| e.error)
        .with_context(|| format!("Could not atomically replace {}", output.display()))?;

    Ok(())
}

// Command-line arguments
struct Cli {
    prefix: Option<PathBuf>,
    output: Option<PathBuf>,
}

fn parse_args() -> Result<Cli> {
    let mut args = std::env::args().skip(1);
    let mut prefix = None;
    let mut output = None;
    let mut positional_output = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--prefix" => {
                if prefix.is_some() {
                    bail!("--prefix may only be specified once");
                }

                let value = args
                    .next()
                    .context("--prefix requires a directory argument")?;
                let path = PathBuf::from(value);

                if !path.is_absolute() {
                    bail!("--prefix must be an absolute path");
                }

                prefix = Some(path);
            }
            "--output" => {
                if output.is_some() {
                    bail!("--output may only be specified once");
                }

                let value = args.next().context("--output requires a file path")?;

                output = Some(PathBuf::from(value));
            }
            "--help" | "-h" => {
                println!(
                    "Usage: gather-interface-facts [--prefix DIR] \
                     [--output FILE | OUTPUT_FILE]\n\
                     \n\
                     --prefix DIR  Redirect the default system output path \
                     under DIR\n\
                     --output FILE Write to this exact path\n\
                     OUTPUT_FILE   Legacy positional output path"
                );
                std::process::exit(0);
            }
            value if value.starts_with('-') => {
                bail!("Unknown option: {value}");
            }
            value => {
                if positional_output.is_some() {
                    bail!("Only one positional output path is allowed");
                }

                positional_output = Some(PathBuf::from(value));
            }
        }
    }

    if output.is_some() && positional_output.is_some() {
        bail!("Use either --output or a positional output path, not both");
    }

    Ok(Cli {
        prefix,
        output: output.or(positional_output),
    })
}

// Application entry point
fn main() -> Result<()> {
    let cli = parse_args()?;

    let paths = Paths { prefix: cli.prefix };

    // An explicit output path is used exactly as supplied.
    // Otherwise, resolve the standard system path through the prefix.
    let output = match cli.output {
        Some(path) => path,
        None => paths.resolve(DEFAULT_OUTPUT)?,
    };

    eprintln!("Gathering network interface facts...");

    let interfaces = collect_interfaces()?;
    let count = interfaces.len();

    let document = FactsDocument {
        schema_version: SCHEMA_VERSION,
        facts_type: "network_interfaces",
        generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        interfaces,
    };

    write_atomically(&output, &document)?;

    eprintln!("Discovered {count} interfaces.");
    eprintln!("Wrote facts to {}.", output.display());

    Ok(())
}
