mod support;

use std::{net::SocketAddr, sync::Arc, time::Duration};

use host_fake::{
    harness::{TestHost, TestHostOptions},
    FakePlatform,
};
use rs_mobile::rscore::{
    api_diag::set_setting,
    api_sessions as sessions,
    dto_core::{OpenIntent, OpenOpts, SessionPhase},
    runtime::Core,
    settings::{Settings, SettingsState},
    store::PairedHostRow,
    tables::{SessionEntry, SessionTable},
};
use rsp_core::trust::Permission;
use tokio::time::{sleep, timeout};

const DEADLINE: Duration = Duration::from_secs(2);

#[test]
fn device_low_power_is_a_validated_boolean_setting() {
    let mut settings = Settings::default();
    assert!(!settings.low_power);
    assert_eq!(
        settings.set("device.lowPower", serde_json::json!(true)),
        Ok(true)
    );
    assert!(settings.low_power);
    assert_eq!(
        settings.set("device.lowPower", serde_json::json!(true)),
        Ok(false)
    );
    assert!(settings
        .set("device.lowPower", serde_json::json!("true"))
        .is_err());
    assert!(settings.low_power);
}

fn pair_row(core: &Arc<Core>, host_id: &str, fp: [u8; 32], address: SocketAddr) {
    core.store()
        .upsert_paired_host(&PairedHostRow {
            host_id: host_id.into(),
            fp,
            name: "Low power test host".into(),
            addresses: vec![address],
            paired_at_us: core.now_us(),
            permission: Some("control".into()),
        })
        .unwrap();
}

fn opts() -> OpenOpts {
    OpenOpts {
        intent: OpenIntent::View,
        display_id: None,
        quality: None,
        mode: None,
        audio: None,
        max_fps: None,
        max_width: None,
        codec: None,
    }
}

async fn until(predicate: impl Fn() -> bool) {
    timeout(DEADLINE, async {
        while !predicate() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition before deadline");
}

async fn open_streaming(core: &Arc<Core>, host_id: &str) -> Arc<SessionEntry> {
    let id = sessions::open(core, host_id, opts()).unwrap();
    let entry = core.state::<SessionTable>().get(id).unwrap();
    until(|| entry.summary().phase == SessionPhase::Streaming && entry.handle().is_some()).await;
    entry
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn device_low_power_updates_existing_and_new_sessions_and_rejects_non_booleans() {
    let core = support::test_core_with(|_, options| options.worker_threads = 2);
    support::with_identity(&core, "Low power test device");
    let host = TestHost::start_with_platform(
        FakePlatform::new(),
        TestHostOptions {
            tcp: false,
            ..Default::default()
        },
    )
    .await;
    host.pair(&core.local_identity().unwrap(), Permission::Control);
    pair_row(&core, &host.host_id, host.fp, host.quic_addr);

    let first = open_streaming(&core, &host.host_id).await;
    assert!(!first.handle().unwrap().low_power());
    set_setting(&core, "device.lowPower", "true").unwrap();
    assert!(core.state::<SettingsState>().0.lock().unwrap().low_power);
    assert!(first.handle().unwrap().low_power());

    let second = open_streaming(&core, &host.host_id).await;
    assert!(second.handle().unwrap().low_power());

    assert!(set_setting(&core, "device.lowPower", "\"yes\"").is_err());
    assert!(core.state::<SettingsState>().0.lock().unwrap().low_power);
    assert!(first.handle().unwrap().low_power());
    assert!(second.handle().unwrap().low_power());

    sessions::close(&core, first.id);
    sessions::close(&core, second.id);
    until(|| core.state::<SessionTable>().is_empty()).await;
    core.shutdown();
}
