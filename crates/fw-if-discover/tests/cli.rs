use serde_json::Value;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const IP_SUCCESS: &str = r#"[{"ifname":"enp1s0","addr_info":[{"family":"inet","local":"192.0.2.10","prefixlen":24,"scope":"global","flags":["secondary"]}]}]"#;

struct Fixture {
    _temp: TempDir,
    sysfs_root: PathBuf,
    bin_dir: PathBuf,
}

impl Fixture {
    fn new(ip_script: &str) -> Self {
        let temp = tempfile::tempdir().expect("create test directory");
        let sysfs_root = temp.path().join("sys");
        let net_dir = sysfs_root.join("class/net/enp1s0");
        let bin_dir = temp.path().join("bin");
        let device_dir = temp.path().join("devices/0000:01:00.0");
        let pci_bus_dir = temp.path().join("bus/pci");
        let driver_dir = temp.path().join("drivers/igb");

        fs::create_dir_all(&net_dir).expect("create mock interface directory");
        fs::create_dir_all(&bin_dir).expect("create mock command directory");
        fs::create_dir_all(&device_dir).expect("create mock device directory");
        fs::create_dir_all(&pci_bus_dir).expect("create mock PCI bus directory");
        fs::create_dir_all(&driver_dir).expect("create mock driver directory");

        for (name, value) in [
            ("address", "00:11:22:33:44:55\n"),
            ("ifindex", "2\n"),
            ("type", "1\n"),
            ("flags", "0x1003\n"),
            ("carrier", "1\n"),
            ("speed", "1000\n"),
            ("mtu", "1500\n"),
            ("operstate", "up\n"),
        ] {
            fs::write(net_dir.join(name), value).expect("write mock sysfs attribute");
        }

        fs::write(device_dir.join("vendor"), "0x8086\n").expect("write mock vendor ID");
        fs::write(device_dir.join("device"), "0x1539\n").expect("write mock device ID");
        symlink(&device_dir, net_dir.join("device")).expect("link mock network device");
        symlink(&pci_bus_dir, device_dir.join("subsystem")).expect("link mock subsystem");
        symlink(&driver_dir, device_dir.join("driver")).expect("link mock driver");

        let ip_path = bin_dir.join("ip");
        fs::write(&ip_path, ip_script).expect("write mock ip command");
        let mut permissions = fs::metadata(&ip_path)
            .expect("read mock ip permissions")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(ip_path, permissions).expect("make mock ip executable");

        Self {
            _temp: temp,
            sysfs_root,
            bin_dir,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fw-if-discover"));
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut search_path = self.bin_dir.as_os_str().to_os_string();
        search_path.push(":");
        search_path.push(path);

        command
            .env("FW_IF_DISCOVER_SYSFS_ROOT", &self.sysfs_root)
            .env("PATH", search_path);
        command
    }

    fn run_to(&self, output_path: &Path) -> Output {
        self.command()
            .arg("--output")
            .arg(output_path)
            .output()
            .expect("run fw-if-discover")
    }
}

#[test]
fn writes_interface_inventory_from_mocked_system_data() {
    let fixture = Fixture::new(&format!("#!/bin/sh\nprintf '%s\\n' '{IP_SUCCESS}'\n"));
    let output_path = fixture._temp.path().join("facts/interfaces.json");

    let output = fixture.run_to(&output_path);
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let facts: Value =
        serde_json::from_slice(&fs::read(&output_path).expect("read generated facts"))
            .expect("parse generated facts");

    assert_eq!(facts["schema_version"], 1);
    assert_eq!(facts["facts_type"], "network_interfaces");
    assert!(facts["generated_at"].as_str().is_some());
    assert_eq!(facts["interfaces"].as_array().unwrap().len(), 1);

    let interface = &facts["interfaces"][0];
    assert_eq!(interface["name"], "enp1s0");
    assert_eq!(interface["mac_address"], "00:11:22:33:44:55");
    assert_eq!(interface["ifindex"], 2);
    assert_eq!(interface["type"], "ether");
    assert_eq!(interface["admin_state"], "up");
    assert_eq!(interface["operstate"], "up");
    assert_eq!(interface["carrier"], true);
    assert_eq!(interface["speed_mbps"], 1000);
    assert_eq!(interface["mtu"], 1500);
    assert_eq!(interface["addresses"][0]["family"], "inet");
    assert_eq!(interface["addresses"][0]["local"], "192.0.2.10");
    assert_eq!(interface["addresses"][0]["prefixlen"], 24);
    assert_eq!(interface["addresses"][0]["scope"], "global");
    assert_eq!(interface["addresses"][0]["flags"][0], "secondary");
    assert_eq!(interface["device"]["bus"], "pci");
    assert_eq!(interface["device"]["path"], "0000:01:00.0");
    assert_eq!(interface["device"]["vendor_id"], "8086");
    assert_eq!(interface["device"]["device_id"], "1539");
    assert_eq!(interface["device"]["driver"], "igb");

    let permissions = fs::metadata(output_path)
        .expect("read output permissions")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(permissions, 0o600);
}

#[test]
fn reports_ip_failure_without_replacing_existing_output() {
    let fixture = Fixture::new("#!/bin/sh\necho 'mock ip failure' >&2\nexit 2\n");
    let output_path = fixture._temp.path().join("facts/interfaces.json");
    fs::create_dir_all(output_path.parent().unwrap()).expect("create output directory");
    fs::write(&output_path, "existing data").expect("write existing output");

    let output = fixture.run_to(&output_path);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("mock ip failure"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(output_path).expect("read existing output"),
        "existing data"
    );
}

#[test]
fn displays_help_without_discovering_interfaces() {
    let fixture = Fixture::new("#!/bin/sh\nexit 99\n");

    let output = fixture
        .command()
        .arg("--help")
        .output()
        .expect("run fw-if-discover help");

    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("--output FILE"),
        "unexpected help: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}
