mod support;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use host_fake::harness::{TestHost, TestHostOptions};
use rs_mobile::rscore::{
    api_pairing::{forget, paired, parse_qr, start},
    api_sessions::{self as sessions, record_host_seen},
    dto_core::{HubDisplay, OpenIntent, OpenOpts, PairOutcome, PairStartArg, SessionPhase},
    runtime::Core,
    store::{HostSeenRow, PairedHostRow},
    tables::SessionTable,
};
use rsp_core::{pb, trust::Permission};
use tokio::time::{sleep, timeout};

async fn until(predicate: impl Fn() -> bool) {
    timeout(Duration::from_secs(3), async {
        while !predicate() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition before deadline");
}

fn pair_row(core: &Arc<Core>, host_id: &str, fp: [u8; 32], address: std::net::SocketAddr) {
    core.store()
        .upsert_paired_host(&PairedHostRow {
            host_id: host_id.into(),
            fp,
            name: "Seen test host".into(),
            addresses: vec![address],
            paired_at_us: core.now_us(),
            permission: Some("control".into()),
        })
        .unwrap();
}

struct Fixture {
    core: Arc<Core>,
    host: TestHost,
}

impl Fixture {
    async fn new() -> Self {
        let core = support::test_core_with(|_, options| options.worker_threads = 2);
        support::with_identity(&core, "Host seen test device");
        let host = TestHost::start_with(TestHostOptions {
            tcp: false,
            ..Default::default()
        })
        .await;
        host.pair(&core.local_identity().unwrap(), Permission::Control);
        pair_row(&core, &host.host_id, host.fp, host.quic_addr);
        Self { core, host }
    }

    async fn stream_once(&self) {
        let id = sessions::open(
            &self.core,
            &self.host.host_id,
            OpenOpts {
                intent: OpenIntent::Control,
                display_id: None,
                quality: None,
                mode: None,
                audio: None,
                max_fps: None,
                max_width: None,
                codec: None,
            },
        )
        .unwrap();
        let entry = self.core.state::<SessionTable>().get(id).unwrap();
        until(|| entry.summary().phase == SessionPhase::Streaming).await;
        sessions::close(&self.core, id);
        until(|| self.core.state::<SessionTable>().is_empty()).await;
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for entry in self.core.state::<SessionTable>().list() {
            sessions::close(&self.core, entry.id);
        }
        self.core.shutdown();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_hello_fills_paired_host_metadata_until_forget() {
    let f = Fixture::new().await;
    let before = f.core.now_us();
    f.stream_once().await;
    let after = f.core.now_us();

    let hosts = paired(&f.core).unwrap();
    assert_eq!(hosts.len(), 1);
    let host = &hosts[0];
    assert_eq!(host.os.as_deref(), Some("linux"));
    assert_eq!(host.os_version.as_deref(), Some(std::env::consts::OS));
    assert_eq!(
        host.host_version.as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert!(host.features.is_some_and(|features| features != 0));
    assert_eq!(host.displays.len(), 1);
    assert_eq!(host.displays[0].name, "Synthetic display");
    assert_eq!(
        (host.displays[0].width_px, host.displays[0].height_px),
        (1920, 1080)
    );
    assert!(host.displays[0].primary);
    assert!(host
        .last_seen_at
        .is_some_and(|at| (before..=after).contains(&at)));
    // hosts_paired() serializes the same rows for the App.
    let json = serde_json::to_value(&hosts).unwrap();
    assert_eq!(json[0]["os"], "linux");
    assert_eq!(json[0]["hostVersion"], env!("CARGO_PKG_VERSION"));
    assert_eq!(json[0]["features"], host.features.unwrap());
    assert_eq!(json[0]["displays"][0]["widthPx"], 1920);
    assert_eq!(json[0]["lastSeenAt"], host.last_seen_at.unwrap());

    forget(&f.core, &f.host.host_id).unwrap();
    assert!(paired(&f.core).unwrap().is_empty());
    assert_eq!(f.core.store().host_seen(&f.host.host_id).unwrap(), None);
    pair_row(&f.core, &f.host.host_id, f.host.fp, f.host.quic_addr);
    let again = &paired(&f.core).unwrap()[0];
    assert_eq!(again.os, None);
    assert_eq!(again.host_version, None);
    assert_eq!(again.features, None);
    assert!(again.displays.is_empty());
    assert_eq!(again.last_seen_at, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_metadata_write_does_not_affect_the_session() {
    let f = Fixture::new().await;
    let db = rusqlite::Connection::open(f.core.data_dir().join("rs.db")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER host_seen_fails BEFORE INSERT ON host_seen
         BEGIN SELECT RAISE(ABORT, 'injected host_seen failure'); END;",
    )
    .unwrap();
    drop(db);

    f.stream_once().await;
    assert_eq!(f.core.store().host_seen(&f.host.host_id).unwrap(), None);
    assert_eq!(paired(&f.core).unwrap()[0].os, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_records_what_the_pairing_hello_discloses() {
    let core = support::test_core_with(|_, options| {
        options.timeouts.pairing = Duration::from_secs(6);
    });
    support::with_identity(&core, "Host seen pairing phone");
    let host = TestHost::start_with(TestHostOptions {
        tcp: false,
        ..Default::default()
    })
    .await;
    let target = parse_qr(&host.new_pairing_code().qr_url).unwrap();
    let before = core.now_us();
    let outcome = timeout(
        Duration::from_secs(9),
        start(&core, PairStartArg::Target(target)),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(outcome, PairOutcome::Ok { .. }), "{outcome:?}");

    let hosts = paired(&core).unwrap();
    assert_eq!(hosts.len(), 1);
    assert_eq!(
        hosts[0].host_version.as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert!(hosts[0].features.is_some_and(|features| features != 0));
    assert!(hosts[0].last_seen_at.is_some_and(|at| at >= before));
    // Pairing callers do not learn the OS or display inventory.
    assert_eq!(hosts[0].os, None);
    assert_eq!(hosts[0].os_version, None);
    assert!(hosts[0].displays.is_empty());
    host.stop().await;
    core.shutdown();
}

fn full_hello(os: pb::Platform) -> pb::ServerHello {
    pb::ServerHello {
        host_version: "1.0.0".into(),
        features: 0b111,
        os: os as i32,
        os_version: "macOS 26.3".into(),
        displays: vec![pb::Display {
            id: 1,
            name: "Built-in".into(),
            width_px: 3024,
            height_px: 1964,
            primary: true,
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn partial_hellos_keep_session_inventory_and_unpaired_hosts_are_skipped() {
    let clock = Arc::new(AtomicU64::new(100));
    let now = clock.clone();
    let core = support::test_core_with(move |_, options| {
        options.clock = Some(Arc::new(move || now.load(Ordering::Relaxed)));
    });
    for host_id in ["h1", "h2"] {
        pair_row(&core, host_id, [1; 32], "100.64.0.3:7400".parse().unwrap());
    }

    record_host_seen(&core, "h1", &full_hello(pb::Platform::Macos), true);
    let first = HostSeenRow {
        host_id: "h1".into(),
        os: Some("macos".into()),
        os_version: Some("macOS 26.3".into()),
        host_version: Some("1.0.0".into()),
        features: Some(0b111),
        displays: vec![HubDisplay {
            id: 1,
            name: "Built-in".into(),
            width_px: 3024,
            height_px: 1964,
            primary: true,
        }],
        seen_at_us: 100,
    };
    assert_eq!(core.store().host_seen("h1").unwrap().as_ref(), Some(&first));

    clock.store(200, Ordering::Relaxed);
    let minimal = pb::ServerHello {
        host_version: "1.1.0".into(),
        features: 0b1,
        ..Default::default()
    };
    record_host_seen(&core, "h1", &minimal, false);
    assert_eq!(
        core.store().host_seen("h1").unwrap(),
        Some(HostSeenRow {
            host_version: Some("1.1.0".into()),
            seen_at_us: 200,
            ..first.clone()
        })
    );
    record_host_seen(&core, "h2", &minimal, false);
    assert_eq!(
        core.store().host_seen("h2").unwrap(),
        Some(HostSeenRow {
            host_id: "h2".into(),
            os: None,
            os_version: None,
            host_version: Some("1.1.0".into()),
            features: Some(0b1),
            displays: Vec::new(),
            seen_at_us: 200,
        })
    );

    for (platform, os) in [
        (pb::Platform::Windows, Some("windows")),
        (pb::Platform::Linux, Some("linux")),
        (pb::Platform::Ios, None),
        (pb::Platform::Unspecified, None),
    ] {
        record_host_seen(&core, "h1", &full_hello(platform), true);
        let row = core.store().host_seen("h1").unwrap().unwrap();
        assert_eq!(row.os.as_deref(), os, "{platform:?}");
    }

    record_host_seen(&core, "stranger", &full_hello(pb::Platform::Macos), true);
    assert_eq!(core.store().host_seen("stranger").unwrap(), None);
    core.shutdown();
}
