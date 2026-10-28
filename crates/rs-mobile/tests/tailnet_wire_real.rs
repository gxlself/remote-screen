//! Requires nextest's one-process-per-case isolation: Go node and ABI loader are
//! process-global. Missing test library/helper is SKIP unless RS_REQUIRE_TAILNET=1.
#![cfg(unix)]

mod support;

use std::{path::Path, sync::Arc, time::Duration};

use rs_mobile::rscore::{
    api_diag::set_setting,
    runtime::Core,
    tailnet_ctl::ControllerConfig,
    tailnet_wire::{install_with, real_node, WireOptions},
};
use rs_tailnet::testkit::{TestNet, TestNetOptions, TestPaths};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::{sleep, timeout},
};

use support::{test_core_with, with_identity, EventCollector};

struct RunningCore {
    core: Arc<Core>,
    events: Arc<EventCollector>,
    testnet: TestNet,
}

async fn wait_event(
    events: &EventCollector,
    after: u64,
    seconds: u64,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    timeout(Duration::from_secs(seconds), async {
        loop {
            if let Some((_, event)) = events
                .events()
                .into_iter()
                .find(|(seq, event)| *seq > after && predicate(event))
            {
                return event;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("expected event before deadline")
}

fn running(event: &Value) -> bool {
    event["type"] == "network.changed"
        && event["status"]["tailnet"]["active"] == "embedded"
        && event["status"]["tailnet"]["embedded"]["state"] == "running"
}

fn last_seq(events: &EventCollector) -> u64 {
    events.events().last().map_or(0, |(seq, _)| *seq)
}

async fn running_core(paths: &TestPaths) -> RunningCore {
    let testnet = TestNet::spawn(
        paths,
        &TestNetOptions {
            require_auth: true,
            ..TestNetOptions::default()
        },
    )
    .expect("start local testnet");
    let core = test_core_with(|_, _| {});
    with_identity(&core, "phone");
    let events = EventCollector::new();
    core.events().subscribe(events.clone());
    let result = install_with(
        &core,
        real_node(&core, true),
        WireOptions {
            disable_netns: true,
            controller: ControllerConfig {
                background_grace: Duration::from_secs(2),
                ..ControllerConfig::default()
            },
            ..WireOptions::default()
        },
    );
    result.unwrap();
    set_setting(&core, "tailnet.mode", "\"embedded\"").unwrap();
    set_setting(
        &core,
        "tailnet.controlUrl",
        &serde_json::to_string(&testnet.control_url).unwrap(),
    )
    .unwrap();
    let login = wait_event(&events, 0, 15, |event| {
        event["type"] == "tailnet.state"
            && event["state"] == "needsLogin"
            && event["authUrl"]
                .as_str()
                .is_some_and(|url| url.contains("/auth/"))
    })
    .await;
    testnet
        .complete(login["authUrl"].as_str().unwrap())
        .expect("complete interactive login");
    wait_event(&events, 0, 30, running).await;
    RunningCore {
        core,
        events,
        testnet,
    }
}

async fn echo(fixture: &RunningCore) {
    timeout(Duration::from_secs(15), async {
        let mut stream = fixture
            .core
            .net()
            .dial_tcp_host(&fixture.testnet.peer_hostname, fixture.testnet.tcp_echo)
            .await
            .expect("dial embedded peer by MagicDNS name");
        let sent: Vec<u8> = (0..4096).map(|index| (index % 251) as u8).collect();
        stream.write_all(&sent).await.unwrap();
        let mut received = vec![0; sent.len()];
        stream.read_exact(&mut received).await.unwrap();
        assert_eq!(received, sent);
    })
    .await
    .expect("embedded echo deadline");
}

fn assert_no_key(bytes: &[u8], secret: &[u8]) {
    // The interactive helper normally announces an empty authKey. Supplying a
    // key would require preauth instead of the browser login under test. Also
    // reject the key prefix, so an empty key cannot make this check vacuous.
    assert!(
        !bytes
            .windows(b"tskey-".len())
            .any(|window| window == b"tskey-"),
        "auth key prefix leaked"
    );
    if !secret.is_empty() {
        assert!(
            !bytes.windows(secret.len()).any(|window| window == secret),
            "auth key leaked"
        );
    }
}

fn check_logs(path: &Path, secret: &[u8]) -> usize {
    let mut files = 0;
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files += check_logs(&path, secret);
        } else {
            let bytes = std::fs::read(path).unwrap();
            assert_no_key(&bytes, secret);
            files += 1;
        }
    }
    files
}

fn finish(fixture: &RunningCore) {
    let node = rs_tailnet::Tailnet::global(rs_tailnet::load_default().unwrap());
    fixture.core.shutdown();
    assert_eq!(
        node.status().unwrap().state,
        rs_tailnet::BackendState::Stopped
    );
    assert_no_key(
        &serde_json::to_vec(&fixture.events.events()).unwrap(),
        fixture.testnet.auth_key.as_bytes(),
    );
    assert!(check_logs(&fixture.core.log_dir(), fixture.testnet.auth_key.as_bytes()) > 0);
    for event in fixture.events.of_type("network.changed") {
        assert!(event["status"]["tailnet"]["embedded"]
            .get("authUrl")
            .is_none());
    }
}

#[tokio::test]
async fn embedded_end_to_end() {
    let Some(paths) = rs_tailnet::testkit::paths_or_skip("embedded_end_to_end") else {
        return;
    };
    let fixture = running_core(&paths).await;
    echo(&fixture).await;
    finish(&fixture);
}

#[tokio::test]
async fn background_stops_node() {
    let Some(paths) = rs_tailnet::testkit::paths_or_skip("background_stops_node") else {
        return;
    };
    let fixture = running_core(&paths).await;
    echo(&fixture).await;
    let before_stop = last_seq(&fixture.events);
    set_setting(&fixture.core, "device.appActive", "false").unwrap();
    wait_event(&fixture.events, before_stop, 10, |event| {
        event["type"] == "tailnet.state" && event["state"] == "stopped"
    })
    .await;
    let before_resume = last_seq(&fixture.events);
    set_setting(&fixture.core, "device.appActive", "true").unwrap();
    wait_event(&fixture.events, before_resume, 30, running).await;
    assert!(!fixture.events.events().iter().any(|(seq, event)| {
        *seq > before_resume && event["type"] == "tailnet.state" && event["state"] == "needsLogin"
    }));
    echo(&fixture).await;
    finish(&fixture);
}
