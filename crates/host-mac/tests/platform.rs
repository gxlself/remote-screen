#![cfg(target_os = "macos")]

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use host_core::platform::{ClipboardItem, KeyStore, Platform};
use host_mac::{
    activity, console, cursor, displays, keystore::FileKeyStore, nics, wol, MacPlatform,
};
use rsp_core::pb::clipboard_header::Kind;
use sha2::{Digest, Sha256};

fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "remote-host-{label}-{}-{stamp}",
        std::process::id()
    ))
}

#[test]
fn file_keystore_round_trip_delete_and_permissions() {
    let dir = temp_dir("keystore");
    let store = FileKeyStore::new(&dir);
    store.store("host-key-v1", b"secret").expect("store key");
    assert_eq!(
        store.load("host-key-v1").expect("load key"),
        Some(b"secret".to_vec())
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&dir)
                .expect("directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(dir.join("host-key-v1"))
                .expect("key metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    store
        .store("host-key-v1", b"replacement")
        .expect("replace key atomically");
    assert_eq!(
        store.load("host-key-v1").expect("load replacement"),
        Some(b"replacement".to_vec())
    );
    store.delete("host-key-v1").expect("delete key");
    assert_eq!(store.load("host-key-v1").expect("load deleted key"), None);
    fs::remove_dir_all(dir).expect("remove test directory");
}

#[test]
fn cursor_shape_decision_deduplicates_by_png_hash() {
    let known = std::collections::HashSet::new();
    let first = cursor::shape_decision(b"cursor-png", 8, 9, 1.0, 2.0, 2.0, &known);
    let id = match first {
        cursor::ShapeDecision::Shape(shape) => {
            assert_eq!(shape.width, 8);
            assert_eq!(shape.height, 9);
            shape.id
        }
        cursor::ShapeDecision::Use(_) => panic!("first shape must carry image"),
    };
    let mut known = std::collections::HashSet::new();
    known.insert(id);
    assert_eq!(
        cursor::shape_decision(b"cursor-png", 8, 9, 1.0, 2.0, 2.0, &known),
        cursor::ShapeDecision::Use(id)
    );
}

#[test]
fn cidr_and_tailnet_helpers() {
    let address = "100.100.1.42".parse().expect("valid IPv4");
    let mask = "255.255.0.0".parse().expect("valid netmask");
    assert_eq!(
        nics::cidr_network(address, mask),
        Some("100.100.0.0/16".into())
    );
    assert!(nics::is_tailnet_address(
        "100.100.1.42".parse().expect("valid IPv4")
    ));
}

#[test]
fn clipboard_round_trips_when_pasteboard_is_available() {
    let platform = MacPlatform::new();
    for (kind, data) in [
        (Kind::Text, b"remote screen text".to_vec()),
        (
            Kind::Png,
            vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0],
        ),
    ] {
        let item = ClipboardItem {
            kind,
            data: data.clone(),
            sha256: [0; 32],
        };
        if let Err(error) = platform.clipboard_set(item.clone()) {
            eprintln!("skipping clipboard {kind:?} round trip: {error}");
            continue;
        }
        let Some(received) = platform.clipboard_get() else {
            eprintln!("skipping clipboard {kind:?} round trip: pasteboard unavailable");
            continue;
        };
        assert_eq!(received.kind, kind);
        assert_eq!(received.data, data);
        let mut expected_sha256 = [0u8; 32];
        expected_sha256.copy_from_slice(Sha256::digest(&data).as_slice());
        assert_eq!(received.sha256, expected_sha256);
    }
}

#[test]
fn console_state_classifies_every_input_combination() {
    use host_core::platform::ConsoleState;

    assert_eq!(
        console::classify_console_state(None),
        ConsoleState::LoginWindow
    );
    for locked in [None, Some(false), Some(true)] {
        assert_eq!(
            console::classify_console_state(Some((None, locked))),
            ConsoleState::Unknown
        );
        assert_eq!(
            console::classify_console_state(Some((Some(false), locked))),
            ConsoleState::OtherUser
        );
    }
    // CGSessionCopyCurrentDictionary omits CGSSessionScreenIsLocked while
    // unlocked; the native shim encodes that missing key as false.
    assert_eq!(
        console::classify_console_state(Some((Some(true), Some(false)))),
        ConsoleState::LoggedIn
    );
    // None here is an unrecognized lock value, not an omitted lock key.
    assert_eq!(
        console::classify_console_state(Some((Some(true), None))),
        ConsoleState::Unknown
    );
    assert_eq!(
        console::classify_console_state(Some((Some(true), Some(true)))),
        ConsoleState::Locked
    );
}

#[test]
fn wol_magic_packet_has_the_wire_shape() {
    let mac = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab];
    let packet = wol::magic_packet(mac);
    assert_eq!(packet.len(), 102);
    assert_eq!(&packet[..6], &[0xff; 6]);
    for repeat in 0..16 {
        assert_eq!(&packet[6 + repeat * 6..12 + repeat * 6], &mac);
    }
}

#[test]
fn wol_broadcast_handles_prefix_boundaries() {
    use std::net::Ipv4Addr;

    assert_eq!(
        wol::broadcast_address(Ipv4Addr::new(192, 168, 7, 42), 24),
        Some(Ipv4Addr::new(192, 168, 7, 255))
    );
    assert_eq!(
        wol::broadcast_address(Ipv4Addr::new(10, 23, 7, 42), 16),
        Some(Ipv4Addr::new(10, 23, 255, 255))
    );
    // RFC 3021 /31 links and /32 host routes do not have a broadcast host.
    assert_eq!(wol::broadcast_address(Ipv4Addr::new(10, 0, 0, 0), 31), None);
    assert_eq!(wol::broadcast_address(Ipv4Addr::new(10, 0, 0, 1), 32), None);
}

#[test]
fn wol_send_to_loopback_delivers_one_magic_packet() {
    use std::io::ErrorKind;
    use std::net::{Ipv4Addr, UdpSocket};
    use std::time::Duration;

    let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind loopback receiver");
    receiver
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set loopback read timeout");
    let port = receiver.local_addr().expect("read receiver address").port();
    let mac = [0xa4, 0x83, 0xe7, 0x12, 0x34, 0x56];

    wol::send_to(
        mac,
        Ipv4Addr::LOCALHOST,
        Ipv4Addr::LOCALHOST,
        port,
        Some("lo0"),
    )
    .expect("send one loopback magic packet");

    let mut packet = [0_u8; 103];
    let (length, _) = receiver
        .recv_from(&mut packet)
        .expect("receive loopback magic packet");
    assert_eq!(length, 102);
    assert_eq!(&packet[..length], &wol::magic_packet(mac));

    let error = receiver
        .recv_from(&mut packet)
        .expect_err("send_to must emit exactly one packet");
    assert_eq!(error.kind(), ErrorKind::WouldBlock);
}

#[test]
fn wol_send_to_unknown_interface_fails() {
    use std::net::Ipv4Addr;

    let error = wol::send_to(
        [0, 1, 2, 3, 4, 5],
        Ipv4Addr::LOCALHOST,
        Ipv4Addr::LOCALHOST,
        0,
        Some("rs-no-such-if0"),
    )
    .expect_err("unknown interface must fail");
    match error {
        host_core::platform::PlatformError::Platform(message) => {
            assert!(
                message.contains("not found"),
                "unexpected message: {message}"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn wol_send_to_rejects_nul_in_interface_name() {
    use std::net::Ipv4Addr;

    let error = wol::send_to(
        [0, 1, 2, 3, 4, 5],
        Ipv4Addr::LOCALHOST,
        Ipv4Addr::LOCALHOST,
        0,
        Some("en\0"),
    )
    .expect_err("NUL in interface name must fail");
    assert!(matches!(
        error,
        host_core::platform::PlatformError::Platform(message) if message == "invalid interface name"
    ));
}

#[test]
fn wol_send_without_ipv4_route_fails() {
    use std::net::{IpAddr, Ipv6Addr};

    let nic = host_core::platform::Nic {
        name: "lo0".into(),
        addresses: vec![IpAddr::V6(Ipv6Addr::LOCALHOST)],
        networks: vec!["::1/128".into()],
        mac: None,
        tailnet: false,
    };
    let error = wol::send([0, 1, 2, 3, 4, 5], &nic).expect_err("IPv6-only NIC must fail");
    assert!(matches!(
        error,
        host_core::platform::PlatformError::Platform(message)
            if message == "WOL requires an IPv4 network with a broadcast address"
    ));
}

#[test]
fn display_debounce_coalesces_until_300ms_quiet() {
    use std::time::Duration;

    assert_eq!(
        displays::debounced_send_times(&[
            Duration::from_millis(0),
            Duration::from_millis(100),
            Duration::from_millis(250),
            Duration::from_millis(700),
        ]),
        vec![Duration::from_millis(550), Duration::from_millis(1000)]
    );
}

#[test]
fn activity_state_throttles_and_emits_after_two_seconds_silence() {
    use std::time::Duration;

    let mut state = activity::ActivityState::default();
    assert_eq!(state.on_event(Duration::from_millis(0)), Some(true));
    assert_eq!(state.on_event(Duration::from_millis(100)), None);
    assert_eq!(state.on_event(Duration::from_millis(200)), Some(true));
    assert_eq!(state.on_tick(Duration::from_millis(2_199)), None);
    assert_eq!(state.on_tick(Duration::from_millis(2_200)), Some(false));
    assert_eq!(state.on_event(Duration::from_millis(2_300)), Some(true));
}
