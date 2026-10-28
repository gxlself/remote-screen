use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use host_core::platform::ConsoleState;
use host_linux::sys_model::{
    adapter_records, console_state_from, detect_session, host_info, model_name, nics,
    os_version_string, parse_hex_u32, parse_mac, parse_os_release, prefix_from_netmask,
    read_os_release, render_nodes, DesktopSession, IfAddrRecord, LogindSession, OsRelease,
    VENDOR_INTEL, VENDOR_NVIDIA,
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "remote-host-linux-{label}-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn session_detection() {
    assert_eq!(
        detect_session(Some("wayland"), None, None),
        DesktopSession::Wayland
    );
    assert_eq!(
        detect_session(Some("X11 "), Some("wayland-0"), Some(":0")),
        DesktopSession::X11
    );
    assert_eq!(
        detect_session(Some("tty"), Some("wayland-0"), None),
        DesktopSession::Wayland
    );
    assert_eq!(detect_session(None, None, Some(":1")), DesktopSession::X11);
    assert_eq!(
        detect_session(None, Some(""), Some("")),
        DesktopSession::None
    );
}

#[test]
fn os_release_parsing() {
    let os = parse_os_release(
        r#"
# comment
NAME="Ubuntu"
PRETTY_NAME="Ubuntu 24.04.1 LTS"
VERSION_ID="24.04"
ID=ubuntu
"#,
    );
    assert_eq!(os.name, "Ubuntu");
    assert_eq!(os.pretty_name.as_deref(), Some("Ubuntu 24.04.1 LTS"));
    assert_eq!(os.version_id.as_deref(), Some("24.04"));
    assert_eq!(os.id.as_deref(), Some("ubuntu"));
    assert_eq!(
        parse_os_release(r##"PRETTY_NAME="A \"B\" \\ C""##).pretty_name,
        Some("A \"B\" \\ C".to_owned())
    );
    assert_eq!(parse_os_release("NAME='Fedora Linux'").name, "Fedora Linux");
    assert_eq!(parse_os_release("VERSION_ID=41").name, "Linux");
}

#[test]
fn os_version_strings() {
    let ubuntu = OsRelease {
        name: "Ubuntu".into(),
        pretty_name: Some("Ubuntu 24.04.1 LTS".into()),
        version_id: Some("24.04".into()),
        id: Some("ubuntu".into()),
    };
    assert_eq!(
        os_version_string(&ubuntu, "6.8.0"),
        "Ubuntu 24.04.1 LTS (Linux 6.8.0)"
    );
    let fedora = OsRelease {
        name: "Fedora Linux".into(),
        pretty_name: None,
        version_id: Some("41".into()),
        id: Some("fedora".into()),
    };
    assert_eq!(
        os_version_string(&fedora, "6.11.4"),
        "Fedora Linux 41 (Linux 6.11.4)"
    );
    assert_eq!(os_version_string(&fedora, ""), "Fedora Linux 41");
}

#[test]
fn read_os_release_fallback() {
    let root = TempDir::new("os-release");
    fs::create_dir_all(root.path().join("usr/lib")).expect("create os-release directory");
    fs::write(
        root.path().join("usr/lib/os-release"),
        "NAME='Fallback Linux'\nVERSION_ID=1\n",
    )
    .expect("write fallback os-release");
    assert_eq!(read_os_release(root.path()).name, "Fallback Linux");
    fs::remove_file(root.path().join("usr/lib/os-release")).expect("remove fallback file");
    assert_eq!(read_os_release(root.path()).name, "Linux");
}

#[test]
fn model_names() {
    assert_eq!(
        model_name(Some("LENOVO"), Some("ThinkPad X1 Carbon")),
        "LENOVO ThinkPad X1 Carbon"
    );
    assert_eq!(
        model_name(Some("Dell Inc."), Some("Dell Inc. XPS 13")),
        "Dell Inc. XPS 13"
    );
    assert_eq!(
        model_name(Some("System manufacturer"), Some("System Product Name")),
        "Linux PC"
    );
    assert_eq!(model_name(None, Some("NUC12")), "NUC12");
}

#[test]
fn host_info_fields() {
    let os = parse_os_release("NAME=Ubuntu\nVERSION_ID=24.04");
    let info = host_info("  box \n", &os, "6.8", Some("Dell"), Some("XPS"), "x86_64");
    assert_eq!(info.os, rsp_core::pb::Platform::Linux);
    assert_eq!(info.name, "box");
    assert_eq!(
        host_info("  \n", &os, "", None, None, "aarch64").name,
        "linux"
    );
}

#[test]
fn render_node_scan() {
    let root = TempDir::new("drm");
    for minor in [129, 128] {
        let device = root.path().join(format!("class/drm/renderD{minor}/device"));
        fs::create_dir_all(&device).expect("create render node");
        fs::write(
            device.join("vendor"),
            if minor == 129 { "0x10de\n" } else { "0x8086" },
        )
        .expect("write vendor");
        if minor == 128 {
            #[cfg(unix)]
            std::os::unix::fs::symlink("/sys/bus/pci/drivers/i915", device.join("driver"))
                .expect("create driver link");
        }
    }
    fs::create_dir_all(root.path().join("class/drm/renderD130")).expect("create missing vendor");
    fs::create_dir_all(root.path().join("class/drm/card0")).expect("create card node");
    let nodes = render_nodes(root.path());
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].minor, 128);
    assert_eq!(nodes[0].vendor, VENDOR_INTEL);
    assert_eq!(nodes[0].dev_path, PathBuf::from("/dev/dri/renderD128"));
    assert_eq!(nodes[1].minor, 129);
    assert_eq!(nodes[1].vendor, VENDOR_NVIDIA);
    #[cfg(unix)]
    assert_eq!(nodes[0].driver.as_deref(), Some("i915"));
}

#[test]
fn hex_parse() {
    assert_eq!(parse_hex_u32("0x8086\n"), Some(0x8086));
    assert_eq!(parse_hex_u32("10DE"), Some(0x10de));
    assert_eq!(parse_hex_u32(" 0X1002 "), Some(0x1002));
    assert_eq!(parse_hex_u32(""), None);
    assert_eq!(parse_hex_u32("0xZZ"), None);
}

fn session(id: &str, uid: u32, class: &str, active: bool, locked_hint: bool) -> LogindSession {
    LogindSession {
        id: id.into(),
        uid,
        class: class.into(),
        active,
        locked_hint,
    }
}

#[test]
fn console_states() {
    assert_eq!(console_state_from(None, None), ConsoleState::Unknown);
    let locked = session("c1", 1000, "user", false, true);
    assert_eq!(
        console_state_from(Some(&locked), None),
        ConsoleState::Locked
    );
    let active = session("c1", 1000, "user", true, false);
    assert_eq!(
        console_state_from(Some(&active), None),
        ConsoleState::LoggedIn
    );
    let own = session("c1", 1000, "user", false, false);
    assert_eq!(
        console_state_from(Some(&own), None),
        ConsoleState::LoginWindow
    );
    assert_eq!(
        console_state_from(Some(&own), Some(&session("g", 0, "greeter", true, false))),
        ConsoleState::LoginWindow
    );
    assert_eq!(
        console_state_from(
            Some(&own),
            Some(&session("l", 0, "lock-screen", true, false)),
        ),
        ConsoleState::Locked
    );
    assert_eq!(
        console_state_from(Some(&own), Some(&session("u", 1001, "user", true, false))),
        ConsoleState::OtherUser
    );
    assert_eq!(
        console_state_from(Some(&own), Some(&session("u2", 1000, "user", true, false))),
        ConsoleState::OtherUser
    );
    assert_eq!(
        console_state_from(Some(&own), Some(&session("c1", 1000, "user", true, false))),
        ConsoleState::LoggedIn
    );
}

#[test]
fn netmask_prefix() {
    assert_eq!(
        prefix_from_netmask(IpAddr::V4(Ipv4Addr::new(255, 255, 255, 0))),
        Some(24)
    );
    assert_eq!(
        prefix_from_netmask(IpAddr::V4(Ipv4Addr::new(255, 0, 255, 0))),
        None
    );
    assert_eq!(
        prefix_from_netmask(IpAddr::V6(Ipv6Addr::new(
            0xffff, 0xffff, 0xffff, 0xffff, 0, 0, 0, 0
        ))),
        Some(64)
    );
}

#[test]
fn nic_records() {
    let root = TempDir::new("nics");
    let net = root.path().join("class/net");
    fs::create_dir_all(net.join("eth0")).expect("create eth0");
    fs::write(net.join("eth0/operstate"), "up\n").expect("write eth0 state");
    fs::write(net.join("eth0/address"), "52:54:00:12:34:56\n").expect("write eth0 mac");
    fs::create_dir_all(net.join("tailscale0")).expect("create tailscale");
    fs::write(net.join("tailscale0/operstate"), "unknown\n").expect("write tailscale state");
    fs::create_dir_all(net.join("docker0")).expect("create docker");
    fs::write(net.join("docker0/operstate"), "down\n").expect("write docker state");
    let addrs = vec![
        IfAddrRecord {
            name: "eth0".into(),
            ip: "192.168.31.20".parse().expect("valid address"),
            prefix: 24,
            loopback: false,
        },
        IfAddrRecord {
            name: "tailscale0".into(),
            ip: "100.64.0.7".parse().expect("valid address"),
            prefix: 32,
            loopback: false,
        },
        IfAddrRecord {
            name: "lo".into(),
            ip: "127.0.0.1".parse().expect("valid address"),
            prefix: 8,
            loopback: true,
        },
        IfAddrRecord {
            name: "docker0".into(),
            ip: "172.17.0.1".parse().expect("valid address"),
            prefix: 16,
            loopback: false,
        },
        IfAddrRecord {
            name: "../x".into(),
            ip: "10.0.0.1".parse().expect("valid address"),
            prefix: 24,
            loopback: false,
        },
    ];
    let records = adapter_records(&addrs, root.path());
    assert_eq!(records.len(), 4);
    let values = nics(&addrs, root.path());
    assert_eq!(
        values
            .iter()
            .map(|nic| nic.name.as_str())
            .collect::<Vec<_>>(),
        ["eth0", "tailscale0"]
    );
    assert_eq!(values[0].networks, ["192.168.31.0/24"]);
    assert_eq!(values[0].mac, Some([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]));
    assert!(values[1].tailnet);
    assert_eq!(values[1].mac, None);
}

#[test]
fn mac_parse() {
    assert_eq!(
        parse_mac("52:54:00:12:34:56\n"),
        Some([0x52, 0x54, 0, 0x12, 0x34, 0x56])
    );
    assert_eq!(parse_mac("52:54:00:12:34"), None);
    assert_eq!(parse_mac("zz:54:00:12:34:56"), None);
}
