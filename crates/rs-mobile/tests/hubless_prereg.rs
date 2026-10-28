mod support;

use rs_mobile::rscore::api_pairing::{paired, pairing_error, start};
use rs_mobile::rscore::dto_core::HubDisplay;
use rs_mobile::rscore::dto_core::PairStartArg;
use rs_mobile::rscore::store::{HostSeenRow, PairedHostRow};
use support::{test_core, with_identity};

#[test]
fn paired_hosts_include_seen_metadata_and_default_for_hosts_never_seen() {
    let core = test_core();
    for (host_id, paired_at_us) in [("seen", 1), ("unseen", 2)] {
        core.store()
            .upsert_paired_host(&PairedHostRow {
                host_id: host_id.into(),
                fp: [1; 32],
                name: host_id.into(),
                addresses: vec!["100.64.0.3:7400".parse().expect("test address")],
                paired_at_us,
                permission: None,
            })
            .expect("paired host");
    }
    let seen = HostSeenRow {
        host_id: "seen".into(),
        os: Some("linux".into()),
        os_version: Some("6.12".into()),
        host_version: Some("0.11.0".into()),
        features: Some(1 << 22),
        displays: vec![HubDisplay {
            id: 1,
            name: "Main".into(),
            width_px: 1920,
            height_px: 1080,
            primary: true,
        }],
        seen_at_us: 123,
    };
    core.store().upsert_host_seen(&seen).expect("host metadata");

    let hosts = paired(&core).expect("paired hosts with metadata");
    assert_eq!(hosts.len(), 2);
    assert_eq!(hosts[0].os, seen.os);
    assert_eq!(hosts[0].os_version, seen.os_version);
    assert_eq!(hosts[0].host_version, seen.host_version);
    assert_eq!(hosts[0].features, seen.features);
    assert_eq!(hosts[0].displays, seen.displays);
    assert_eq!(hosts[0].last_seen_at, Some(seen.seen_at_us));
    assert_eq!(hosts[1].os, None);
    assert_eq!(hosts[1].os_version, None);
    assert_eq!(hosts[1].host_version, None);
    assert_eq!(hosts[1].features, None);
    assert!(hosts[1].displays.is_empty());
    assert_eq!(hosts[1].last_seen_at, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn address_pairing_no_longer_returns_the_preregistered_stub_outcome() {
    let core = test_core();
    with_identity(&core, "Prereg phone");
    let outcome = start(
        &core,
        PairStartArg::CodeAddr {
            addr: "a b".into(),
            port: None,
            code: "123456".into(),
        },
    )
    .await
    .expect("address pairing outcome");
    assert_ne!(outcome, pairing_error("pairing by address not implemented"));
    assert_eq!(outcome, pairing_error("not a remote host"));
    assert!(paired(&core).expect("paired hosts").is_empty());
}
