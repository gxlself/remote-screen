use host_core::store::{
    DevicePlatform, DeviceStatus, PairedDevice, PairingPath, PathPermission, PathPermissions,
    Store, StoreError, TrustState,
};
use rsp_core::ids::id_from_fp;
use rsp_core::trust::Permission;

fn dir(name: &str) -> std::path::PathBuf {
    let root = std::env::var_os("TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("target"));
    let path = root.join(format!("rsp-host-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    path
}

fn device(fp: [u8; 32]) -> PairedDevice {
    PairedDevice {
        fp,
        device_id: id_from_fp(&fp),
        name: "phone".into(),
        cert_der: vec![1, 2, 3],
        permission: Permission::Control,
        paired_at_us: 42,
        require_confirm: false,
        platform: DevicePlatform::Ipados,
        paired_via: PairingPath::Intranet,
        whois_user: Some("user@example.com".into()),
        whois_node: Some("n-phone".into()),
        whois_node_name: Some("phone.hs.example.com".into()),
        path_permissions: PathPermissions {
            tailnet: PathPermission::Allow,
            intranet: PathPermission::ViewOnly,
            lan: PathPermission::Deny,
        },
    }
}

#[test]
fn paired_revoked_and_trust_state_round_trip() {
    let path = dir("roundtrip");
    let store = Store::open(&path).expect("open");
    let fp = [7; 32];
    store.upsert(device(fp)).expect("upsert");
    store
        .set_trust_state(TrustState {
            last_applied_seq: 4,
            last_trust_sync_at_us: 9,
        })
        .expect("state");
    assert!(matches!(
        store.authorize_device(&fp).unwrap(),
        DeviceStatus::Paired { .. }
    ));
    drop(store);
    let store = Store::open(&path).expect("reopen");
    assert_eq!(store.get(&fp).unwrap(), Some(device(fp)));
    assert_eq!(store.get(&fp).unwrap().unwrap().name, "phone");
    assert_eq!(store.trust_state().unwrap().last_applied_seq, 4);
    store.revoke(fp, 5).expect("revoke");
    assert_eq!(store.authorize_device(&fp).unwrap(), DeviceStatus::Revoked);
    assert!(store.get(&fp).unwrap().is_none());
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn corrupt_json_is_not_treated_as_empty() {
    let path = dir("corrupt");
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("paired.json"), b"not json").unwrap();
    let error = Store::open(&path).expect_err("corrupt");
    assert!(matches!(error, StoreError::Corrupt { .. }));
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn upsert_cannot_restore_a_revoked_fingerprint_even_after_restart() {
    let path = dir("revoked-upsert");
    let store = Store::open(&path).expect("open");
    let fp = [7; 32];
    store.upsert(device(fp)).expect("initial pairing");
    store.revoke(fp, 5).expect("revoke");
    let paired_before = std::fs::read(path.join("paired.json")).expect("paired file");
    let revoked_before = std::fs::read(path.join("revoked.json")).expect("revoked file");
    assert!(matches!(store.upsert(device(fp)), Err(StoreError::Revoked)));
    assert!(store.get(&fp).expect("get").is_none());
    assert_eq!(store.authorize_device(&fp).unwrap(), DeviceStatus::Revoked);
    drop(store);

    let store = Store::open(&path).expect("reopen");
    assert!(matches!(store.upsert(device(fp)), Err(StoreError::Revoked)));
    assert!(store.get(&fp).expect("get").is_none());
    assert_eq!(store.authorize_device(&fp).unwrap(), DeviceStatus::Revoked);
    assert_eq!(
        std::fs::read(path.join("paired.json")).unwrap(),
        paired_before
    );
    assert_eq!(
        std::fs::read(path.join("revoked.json")).unwrap(),
        revoked_before
    );
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn legacy_pairing_file_is_upgraded_and_round_trips() {
    let path = dir("legacy");
    std::fs::create_dir_all(&path).unwrap();
    let fp = [17; 32];
    // Original v1 schema, deliberately without optional pairing metadata.
    let legacy = serde_json::json!({
        "version": 1,
        "devices": [{
            "fp": rsp_core::ids::fp_b64url(&fp),
            "device_id": id_from_fp(&fp),
            "name": "legacy phone",
            "cert_der": "AQID",
            "permission": "control",
            "paired_at_us": 42,
            "require_confirm": true
        }]
    });
    std::fs::write(path.join("paired.json"), legacy.to_string()).unwrap();
    let store = Store::open(&path).expect("open legacy file");
    let mut expected = device(fp);
    expected.name = "legacy phone".into();
    expected.require_confirm = true;
    expected.platform = DevicePlatform::Unknown;
    expected.paired_via = PairingPath::Tailnet;
    expected.whois_user = None;
    expected.whois_node = None;
    expected.whois_node_name = None;
    expected.path_permissions = PathPermissions::default();
    assert_eq!(store.get(&fp).unwrap(), Some(expected.clone()));
    let upgraded: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path.join("paired.json")).unwrap()).unwrap();
    let record = &upgraded["devices"][0];
    assert_eq!(record["platform"], "unknown");
    assert_eq!(record["paired_via"], "tailnet");
    for key in ["whois_user", "whois_node", "whois_node_name"] {
        assert!(record.as_object().unwrap().contains_key(key));
        assert!(record[key].is_null());
    }
    assert_eq!(
        record["path_permissions"],
        serde_json::json!({"tailnet": "allow", "intranet": "allow", "lan": "allow"})
    );
    let first_write = std::fs::read(path.join("paired.json")).unwrap();
    drop(store);
    let store = Store::open(&path).expect("reopen upgraded file");
    assert_eq!(store.get(&fp).unwrap(), Some(expected));
    assert_eq!(
        std::fs::read(path.join("paired.json")).unwrap(),
        first_write
    );
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn desktop_platform_round_trips_and_unknown_values_are_ignored() {
    let path = dir("desktop-platform");
    std::fs::create_dir_all(&path).unwrap();
    let fp = [19; 32];
    let mut value = serde_json::json!({
        "version": 1,
        "devices": [{
            "fp": rsp_core::ids::fp_b64url(&fp),
            "device_id": id_from_fp(&fp),
            "name": "desktop",
            "cert_der": "AQID",
            "permission": "control",
            "paired_at_us": 42,
            "require_confirm": false,
            "platform": "macos"
        }]
    });
    std::fs::write(path.join("paired.json"), value.to_string()).unwrap();
    let store = Store::open(&path).expect("open desktop platform");
    assert_eq!(
        store.get(&fp).unwrap().unwrap().platform,
        DevicePlatform::Macos
    );
    drop(store);

    value["devices"][0]["platform"] = serde_json::json!("beos");
    std::fs::write(path.join("paired.json"), value.to_string()).unwrap();
    let store = Store::open(&path).expect("open unknown platform");
    assert_eq!(
        store.get(&fp).unwrap().unwrap().platform,
        DevicePlatform::Unknown
    );
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn permission_updates_persist_without_changing_pairing_metadata() {
    let path = dir("permissions");
    let fp = [18; 32];
    let store = Store::open(&path).unwrap();
    let mut expected = device(fp);
    store.upsert(expected.clone()).unwrap();
    for permission in [
        Permission::ViewOnly,
        Permission::Unattended,
        Permission::Control,
    ] {
        assert!(store.set_permission(&fp, permission).unwrap());
        expected.permission = permission;
        assert_eq!(store.get(&fp).unwrap(), Some(expected.clone()));
    }
    for route in [
        PairingPath::Tailnet,
        PairingPath::Intranet,
        PairingPath::Lan,
    ] {
        for permission in [
            PathPermission::Deny,
            PathPermission::ViewOnly,
            PathPermission::Allow,
        ] {
            assert!(store.set_path_permission(&fp, route, permission).unwrap());
            match route {
                PairingPath::Tailnet => expected.path_permissions.tailnet = permission,
                PairingPath::Intranet => expected.path_permissions.intranet = permission,
                PairingPath::Lan => expected.path_permissions.lan = permission,
            }
            assert_eq!(store.get(&fp).unwrap(), Some(expected.clone()));
            let reopened = Store::open(&path).unwrap();
            assert_eq!(reopened.get(&fp).unwrap(), Some(expected.clone()));
        }
    }
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.get(&fp).unwrap(), Some(expected));
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn permission_updates_reject_revoked_and_ignore_unknown_fingerprints() {
    let path = dir("permissions-revoked");
    let store = Store::open(&path).unwrap();
    let fp = [19; 32];
    store.upsert(device(fp)).unwrap();
    store.revoke(fp, 7).unwrap();
    for store in [store, Store::open(&path).unwrap()] {
        let paired_before = std::fs::read(path.join("paired.json")).unwrap();
        let revoked_before = std::fs::read(path.join("revoked.json")).unwrap();
        assert!(matches!(
            store.set_permission(&fp, Permission::Unattended),
            Err(StoreError::Revoked)
        ));
        for route in [
            PairingPath::Tailnet,
            PairingPath::Intranet,
            PairingPath::Lan,
        ] {
            assert!(matches!(
                store.set_path_permission(&fp, route, PathPermission::Allow),
                Err(StoreError::Revoked)
            ));
            assert!(!store
                .set_path_permission(&[20; 32], route, PathPermission::Deny)
                .unwrap());
        }
        assert!(!store
            .set_permission(&[20; 32], Permission::Control)
            .unwrap());
        assert_eq!(
            std::fs::read(path.join("paired.json")).unwrap(),
            paired_before
        );
        assert_eq!(
            std::fs::read(path.join("revoked.json")).unwrap(),
            revoked_before
        );
    }
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn failed_permission_write_keeps_the_previous_authorization_state() {
    let path = dir("permissions-write-error");
    let fp = [21; 32];
    let store = Store::open(&path).unwrap();
    let expected = device(fp);
    store.upsert(expected.clone()).unwrap();
    let blocked_tmp = path.join(".paired.json.tmp");
    std::fs::create_dir(&blocked_tmp).unwrap();
    assert!(matches!(
        store.set_permission(&fp, Permission::Unattended),
        Err(StoreError::Io(_))
    ));
    assert!(matches!(
        store.set_path_permission(&fp, PairingPath::Intranet, PathPermission::Allow),
        Err(StoreError::Io(_))
    ));
    assert_eq!(store.get(&fp).unwrap(), Some(expected.clone()));
    std::fs::remove_dir(blocked_tmp).unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.get(&fp).unwrap(), Some(expected));
    let _ = std::fs::remove_dir_all(path);
}
