use host_core::store::{DevicePlatform, PairedDevice, PairingPath, PathPermissions, Store};
use rsp_transport::identity::{CertKind, LocalIdentity};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

pub fn device() -> &'static Arc<LocalIdentity> {
    static DEVICE: OnceLock<Arc<LocalIdentity>> = OnceLock::new();
    DEVICE.get_or_init(|| {
        Arc::new(LocalIdentity::generate_software(CertKind::App).expect("test device"))
    })
}

pub fn state_dir() -> std::path::PathBuf {
    static ID: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "h3b-{}-{}-{}",
        std::process::id(),
        rsp_core::clock::unix_micros(),
        ID.fetch_add(1, Ordering::Relaxed)
    ))
}

pub fn paired_store() -> Arc<Store> {
    let store = Arc::new(Store::open(state_dir()).expect("store"));
    pair_device(&store, device());
    store
}

pub fn pair_device(store: &Store, device: &LocalIdentity) {
    store
        .upsert(PairedDevice {
            fp: device.fingerprint(),
            device_id: rsp_core::ids::id_from_fp(&device.fingerprint()),
            name: "test device".into(),
            cert_der: device.cert_der.as_ref().to_vec(),
            permission: rsp_core::trust::Permission::Control,
            paired_at_us: rsp_core::clock::unix_micros(),
            require_confirm: false,
            platform: DevicePlatform::Ios,
            paired_via: PairingPath::Lan,
            whois_user: None,
            whois_node: None,
            whois_node_name: None,
            path_permissions: PathPermissions::default(),
        })
        .expect("paired test identity");
}

#[allow(dead_code)] // Shared fixture: only tests which pin a runtime need this.
pub fn host_fingerprint(platform: &host_fake::FakePlatform) -> [u8; 32] {
    use host_core::platform::Platform;
    host_core::identity::HostIdentity::load_or_create(
        platform.keystore(),
        rsp_core::clock::unix_micros(),
    )
    .expect("host identity")
    .fp()
}
