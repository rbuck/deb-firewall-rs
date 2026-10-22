use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

const DEFAULT_FACTS: &str = "/var/lib/spider/facts.d/interfaces.json";
const DEFAULT_CONFIG: &str = "/etc/spider/network.yaml";
const DEFAULT_OUTPUT: &str = "/var/lib/spider/resolved/network.json";
const FACTS_TYPE: &str = "network_interfaces";

#[derive(Debug)]
struct Cli {
    prefix: Option<PathBuf>,
    facts: Option<PathBuf>,
    config: Option<PathBuf>,
    output: Option<PathBuf>,
    role_overrides: Vec<(String, String)>,
}

#[derive(Debug, Deserialize)]
struct FactsDocument {
    schema_version: u32,
    facts_type: String,
    #[serde(default)]
    generated_at: String,
    #[serde(default)]
    interfaces: Vec<DiscoveredInterface>,
}

#[derive(Debug, Clone, Deserialize)]
struct DiscoveredInterface {
    name: String,
    #[serde(default)]
    mac_address: Option<String>,
    #[serde(default)]
    ifindex: Option<u32>,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    arphrd_type: Option<u32>,
    #[serde(default)]
    admin_state: String,
    #[serde(default)]
    operstate: Option<String>,
    #[serde(default)]
    carrier: Option<bool>,
    #[serde(default)]
    speed_mbps: Option<u64>,
    #[serde(default)]
    mtu: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct UserConfig {
    #[serde(default)]
    schema_version: Option<u32>,
    #[serde(default)]
    interfaces: Vec<InterfaceIntent>,
}

#[derive(Debug, Deserialize)]
struct InterfaceIntent {
    #[serde(rename = "match")]
    matcher: InterfaceMatch,
    role: Option<String>,
    #[serde(default)]
    ipv4: Option<Value>,
    #[serde(default)]
    ipv6: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct InterfaceMatch {
    #[serde(default)]
    mac_address: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Serialize)]
struct ResolvedDocument {
    schema_version: u32,
    facts_generated_at: String,
    source_config_schema_version: Option<u32>,
    interfaces: Vec<ResolvedInterface>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Serialize)]
struct ResolvedInterface {
    name: String,
    mac_address: Option<String>,
    ifindex: Option<u32>,
    role: Option<String>,
    role_source: Option<String>,
    configured: bool,
    ipv4: Option<Value>,
    ipv6: Option<Value>,
    observed: ObservedState,
}

#[derive(Debug, Serialize)]
struct ObservedState {
    interface_type: String,
    arphrd_type: Option<u32>,
    admin_state: String,
    operstate: Option<String>,
    carrier: Option<bool>,
    speed_mbps: Option<u64>,
    mtu: Option<u32>,
}

#[derive(Debug, Serialize)]
struct Diagnostic {
    level: String,
    code: String,
    message: String,
}

fn main() -> Result<()> {
    let cli = parse_args()?;
    let facts_path = resolve_path(cli.facts.as_deref(), DEFAULT_FACTS, cli.prefix.as_deref());
    let config_path = resolve_path(cli.config.as_deref(), DEFAULT_CONFIG, cli.prefix.as_deref());
    let output_path = resolve_path(cli.output.as_deref(), DEFAULT_OUTPUT, cli.prefix.as_deref());

    let facts_text = fs::read_to_string(&facts_path)
        .with_context(|| format!("reading facts file {}", facts_path.display()))?;
    let facts: FactsDocument = serde_json::from_str(&facts_text)
        .with_context(|| format!("parsing facts JSON {}", facts_path.display()))?;

    if facts.schema_version != 1 {
        bail!(
            "unsupported facts schema_version {}; this resolver supports version 1",
            facts.schema_version
        );
    }
    if facts.facts_type != FACTS_TYPE {
        bail!(
            "unexpected facts_type {:?}; expected {:?}",
            facts.facts_type,
            FACTS_TYPE
        );
    }

    let config_text = fs::read_to_string(&config_path)
        .with_context(|| format!("reading configuration file {}", config_path.display()))?;
    let config: UserConfig = serde_yaml::from_str(&config_text)
        .with_context(|| format!("parsing YAML configuration {}", config_path.display()))?;

    let resolved = resolve(facts, config, &cli.role_overrides)?;
    write_json_atomically(&output_path, &resolved)?;
    eprintln!("wrote resolved network plan to {}", output_path.display());

    let errors = resolved
        .diagnostics
        .iter()
        .filter(|d| d.level == "error")
        .count();
    if errors > 0 {
        bail!(
            "resolution produced {errors} error diagnostic(s); output was written for inspection"
        );
    }

    Ok(())
}

fn resolve_path(explicit: Option<&Path>, default: &str, prefix: Option<&Path>) -> PathBuf {
    if let Some(path) = explicit {
        // Explicit paths are honored exactly as supplied.
        return path.to_path_buf();
    }
    let default_path = Path::new(default);
    match prefix {
        Some(prefix) => prefix.join(default_path.strip_prefix("/").unwrap_or(default_path)),
        None => default_path.to_path_buf(),
    }
}

fn resolve(
    facts: FactsDocument,
    config: UserConfig,
    raw_overrides: &[(String, String)],
) -> Result<ResolvedDocument> {
    let mut diagnostics = Vec::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();
    let mut by_mac: HashMap<String, Vec<usize>> = HashMap::new();

    for (index, iface) in facts.interfaces.iter().enumerate() {
        if by_name.insert(iface.name.clone(), index).is_some() {
            bail!("facts contain duplicate interface name {:?}", iface.name);
        }
        if let Some(mac) = iface.mac_address.as_deref() {
            by_mac.entry(normalize_mac(mac)).or_default().push(index);
        }
    }

    let mut intents_by_index: HashMap<usize, &InterfaceIntent> = HashMap::new();
    for intent in &config.interfaces {
        if intent
            .matcher
            .mac_address
            .as_deref()
            .is_some_and(|mac| mac.trim().is_empty())
        {
            bail!("interface match.mac_address cannot be empty");
        }
        if intent.matcher.name.as_deref().is_some_and(str::is_empty) {
            bail!("interface match.name cannot be empty");
        }

        let index = match_interface(&intent.matcher, &by_name, &by_mac)?;
        if intents_by_index.insert(index, intent).is_some() {
            bail!(
                "multiple configuration entries resolve to interface {:?}",
                facts.interfaces[index].name
            );
        }
        if intent.role.as_deref().is_some_and(|r| r.trim().is_empty()) {
            bail!(
                "role cannot be empty for interface {:?}",
                facts.interfaces[index].name
            );
        }
    }

    let mut overrides: HashMap<usize, String> = HashMap::new();
    for (identifier, role) in raw_overrides {
        if role.trim().is_empty() {
            bail!("runtime role override for {identifier:?} has an empty role");
        }
        let index = match_identifier(identifier, &facts.interfaces, &by_name, &by_mac)?;
        if overrides.insert(index, role.clone()).is_some() {
            bail!(
                "more than one runtime role override targets interface {:?}",
                facts.interfaces[index].name
            );
        }
    }

    let mut resolved_interfaces = Vec::new();

    for (index, iface) in facts.interfaces.iter().enumerate() {
        let intent = intents_by_index.get(&index).copied();
        let override_role = overrides.get(&index);
        let configured_role = intent.and_then(|i| i.role.as_deref());

        let (role, role_source) = if let Some(role) = override_role {
            (Some(role.clone()), Some("runtime_override".to_string()))
        } else if let Some(role) = configured_role {
            (Some(role.to_string()), Some("user_config".to_string()))
        } else {
            (None, None)
        };

        if intent.is_none() && override_role.is_none() {
            diagnostics.push(Diagnostic {
                level: "info".into(),
                code: "interface_unconfigured".into(),
                message: format!(
                    "Interface {:?} was discovered but has no configured role; it is not assigned a role by inference.",
                    iface.name
                ),
            });
        }

        if let (Some(configured), Some(overridden)) = (configured_role, override_role)
            && configured != overridden
        {
            diagnostics.push(Diagnostic {
                level: "info".into(),
                code: "role_overridden".into(),
                message: format!(
                    "Runtime override changes the role of {:?} from {:?} to {:?} for this run only.",
                    iface.name, configured, overridden
                ),
            });
        }

        if iface.carrier == Some(false) && role.as_deref() == Some("wan") {
            diagnostics.push(Diagnostic {
                level: "warning".into(),
                code: "wan_without_carrier".into(),
                message: format!(
                    "Interface {:?} is assigned to WAN but discovery reports no carrier.",
                    iface.name
                ),
            });
        }

        resolved_interfaces.push(ResolvedInterface {
            name: iface.name.clone(),
            mac_address: iface.mac_address.clone(),
            ifindex: iface.ifindex,
            role,
            role_source,
            configured: intent.is_some(),
            ipv4: intent.and_then(|i| i.ipv4.clone()),
            ipv6: intent.and_then(|i| i.ipv6.clone()),
            observed: ObservedState {
                interface_type: iface.kind.clone(),
                arphrd_type: iface.arphrd_type,
                admin_state: iface.admin_state.clone(),
                operstate: iface.operstate.clone(),
                carrier: iface.carrier,
                speed_mbps: iface.speed_mbps,
                mtu: iface.mtu,
            },
        });
    }

    // A role override may assign a role to a discovered interface without a
    // persistent config entry. Its address-family configuration remains absent.
    // Duplicate roles are intentionally allowed (e.g. multiple LAN ports).

    Ok(ResolvedDocument {
        schema_version: 1,
        facts_generated_at: facts.generated_at,
        source_config_schema_version: config.schema_version,
        interfaces: resolved_interfaces,
        diagnostics,
    })
}

fn match_interface(
    matcher: &InterfaceMatch,
    by_name: &HashMap<String, usize>,
    by_mac: &HashMap<String, Vec<usize>>,
) -> Result<usize> {
    if matcher.mac_address.is_none() && matcher.name.is_none() {
        bail!("each interface configuration needs match.mac_address or match.name");
    }

    let mut candidates: Option<HashSet<usize>> = None;

    if let Some(mac) = matcher.mac_address.as_deref() {
        let key = normalize_mac(mac);
        let matches = by_mac.get(&key).cloned().unwrap_or_default();
        candidates = Some(matches.into_iter().collect());
    }

    if let Some(name) = matcher.name.as_deref() {
        let name_matches = by_name
            .get(name)
            .copied()
            .into_iter()
            .collect::<HashSet<_>>();
        candidates = Some(match candidates {
            Some(current) => current.intersection(&name_matches).copied().collect(),
            None => name_matches,
        });
    }

    let candidates = candidates.unwrap_or_default();
    match candidates.len() {
        1 => Ok(*candidates.iter().next().expect("one candidate exists")),
        0 => bail!(
            "configuration interface match did not match discovered hardware (name={:?}, mac_address={:?})",
            matcher.name,
            matcher.mac_address
        ),
        _ => bail!(
            "configuration interface match is ambiguous (name={:?}, mac_address={:?})",
            matcher.name,
            matcher.mac_address
        ),
    }
}

fn match_identifier(
    identifier: &str,
    interfaces: &[DiscoveredInterface],
    by_name: &HashMap<String, usize>,
    by_mac: &HashMap<String, Vec<usize>>,
) -> Result<usize> {
    let id = identifier.trim();
    if id.is_empty() {
        bail!("runtime role override has an empty interface identifier");
    }

    let mac_key = normalize_mac(id);
    let mut candidates = HashSet::new();
    if let Some(index) = by_name.get(id) {
        candidates.insert(*index);
    }
    if let Some(matches) = by_mac.get(&mac_key) {
        candidates.extend(matches.iter().copied());
    }

    match candidates.len() {
        1 => return Ok(*candidates.iter().next().expect("one candidate exists")),
        _ if !candidates.is_empty() => {
            bail!(
                "runtime role override identifier {id:?} is ambiguous; it matches multiple discovered interfaces"
            );
        }
        _ => {}
    }

    let known_names = interfaces
        .iter()
        .map(|i| i.name.as_str())
        .collect::<Vec<_>>();
    bail!(
        "runtime role override identifier {id:?} did not match a discovered interface (known names: {})",
        known_names.join(", ")
    )
}

fn normalize_mac(mac: &str) -> String {
    mac.trim().to_ascii_lowercase()
}

fn write_json_atomically(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("creating output directory {}", parent.display()))?;

    let mut file = NamedTempFile::new_in(parent)
        .with_context(|| format!("creating temporary file in {}", parent.display()))?;
    serde_json::to_writer_pretty(&mut file, value).context("serializing resolved network plan")?;
    file.write_all(b"\n").context("writing JSON newline")?;
    file.as_file()
        .sync_all()
        .context("syncing temporary output file")?;

    file.persist(path)
        .map_err(|e| e.error)
        .with_context(|| format!("atomically replacing {}", path.display()))?;
    Ok(())
}

fn parse_args() -> Result<Cli> {
    let mut cli = Cli {
        prefix: None,
        facts: None,
        config: None,
        output: None,
        role_overrides: Vec::new(),
    };

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--prefix" => set_once_path(
                &mut cli.prefix,
                next_value(&mut args, "--prefix")?,
                "--prefix",
            )?,
            "--facts" => {
                set_once_path(&mut cli.facts, next_value(&mut args, "--facts")?, "--facts")?
            }
            "--config" => set_once_path(
                &mut cli.config,
                next_value(&mut args, "--config")?,
                "--config",
            )?,
            "--output" => set_once_path(
                &mut cli.output,
                next_value(&mut args, "--output")?,
                "--output",
            )?,
            "--role-override" => {
                let value = next_value(&mut args, "--role-override")?;
                let (identifier, role) = value.split_once('=').with_context(|| {
                    format!("invalid --role-override {value:?}; expected IDENTIFIER=ROLE")
                })?;
                if identifier.trim().is_empty() || role.trim().is_empty() {
                    bail!(
                        "invalid --role-override {value:?}; identifier and role must be non-empty"
                    );
                }
                cli.role_overrides
                    .push((identifier.trim().to_string(), role.trim().to_string()));
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => bail!("unknown argument {other:?}; use --help for usage"),
        }
    }

    if let Some(prefix) = cli.prefix.as_deref()
        && !prefix.is_absolute()
    {
        bail!("--prefix must be an absolute path");
    }

    Ok(cli)
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    args.next()
        .with_context(|| format!("{flag} requires a value"))
}

fn set_once_path(slot: &mut Option<PathBuf>, value: String, flag: &str) -> Result<()> {
    if slot.is_some() {
        bail!("{flag} may be specified only once");
    }
    *slot = Some(PathBuf::from(value));
    Ok(())
}

fn print_help() {
    println!(
        "fw network resolver\n\
         \n\
         Usage: fw-if-resolve [OPTIONS]\n\
         \n\
         Options:\n\
           --facts PATH                 Discovery facts JSON\n\
           --config PATH                User network YAML\n\
           --output PATH                Resolved plan JSON output\n\
           --prefix DIR                 Prefix default paths for isolated testing\n\
           --role-override ID=ROLE      Runtime role override; repeatable\n\
           -h, --help                   Show this help\n\
         \n\
         Defaults:\n\
           facts:  {DEFAULT_FACTS}\n\
           config: {DEFAULT_CONFIG}\n\
           output: {DEFAULT_OUTPUT}\n\
         \n\
         Runtime role overrides accept a discovered interface name or MAC address.\n\
         Explicit --facts/--config/--output paths are used as supplied; --prefix\n\
         affects only default paths."
    );
}
