//! These tests depend on nextest running each case in its own process: the Go
//! tailnet node and its ABI loader are process-level singletons.

mod support;

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use rs_mobile::rscore::api_tailnet::{
    available, net_dialer, parse_start_options, start, status_json, stop, TailnetStartOptions,
};
use rs_mobile::rscore::error::CoreError;
use rs_tailnet::testkit::{TestNet, TestNetOptions};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use support::{test_core, EventCollector};

fn start_options(
    testnet: &TestNet,
    hostname: &str,
    auth_key: Option<String>,
) -> TailnetStartOptions {
    TailnetStartOptions {
        control_url: testnet.control_url.clone(),
        hostname: hostname.to_owned(),
        auth_key,
        disable_netns: true,
    }
}

fn tailnet_events(events: &EventCollector) -> Vec<Value> {
    events.of_type("tailnet.state")
}

fn running_event(events: &EventCollector) -> Option<Value> {
    tailnet_events(events)
        .into_iter()
        .find(|event| event["state"] == "running")
}

fn wait_running(events: &EventCollector) -> Value {
    assert!(events.wait_for(
        |events| events
            .iter()
            .any(|event| event["type"] == "tailnet.state" && event["state"] == "running"),
        Duration::from_secs(30),
    ));
    running_event(events).expect("running event")
}

#[tokio::test]
async fn preauth_start_running() {
    let Some(paths) = rs_tailnet::testkit::paths_or_skip("preauth_start_running") else {
        return;
    };
    let testnet = TestNet::spawn(
        &paths,
        &TestNetOptions {
            auth_key: Some("tskey-rs-mobile-preauth".to_owned()),
            ..Default::default()
        },
    )
    .expect("testnet");
    let auth_key = testnet.auth_key.clone();
    let core = test_core();
    let events = EventCollector::new();
    core.events().subscribe(events.clone());
    assert!(available());

    start(
        &core,
        start_options(&testnet, "rs-app-preauth", Some(auth_key.clone())),
    )
    .await
    .expect("start");
    let running = wait_running(&events);
    assert!(!running["ips"].as_array().unwrap().is_empty());
    assert_eq!(running["derpRegion"], "test");

    let status: Value =
        serde_json::from_str(&status_json(&core).await.expect("status")).expect("status JSON");
    assert_eq!(status["state"], "running");
    assert!(status["peers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|peer| peer["hostname"] == "rs-peer"));
    let data_dir = core.data_dir();
    let tailnet_dir = data_dir.join("tailnet");
    assert_eq!(
        std::fs::metadata(&tailnet_dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let log_path = data_dir.join("logs/tailnet.log");
    assert!(log_path.is_file());
    let event_json = serde_json::to_string(&events.events()).unwrap();
    assert!(!event_json.contains(&auth_key));

    stop(&core).await.expect("stop");
    let log = std::fs::read_to_string(log_path).unwrap_or_default();
    assert!(!log.contains(&auth_key));
    core.shutdown();
}

#[tokio::test]
async fn interactive_login_logout_login() {
    let Some(paths) = rs_tailnet::testkit::paths_or_skip("interactive_login_logout_login") else {
        return;
    };
    let testnet = TestNet::spawn(
        &paths,
        &TestNetOptions {
            require_auth: true,
            ..Default::default()
        },
    )
    .expect("testnet");
    let core = test_core();
    let events = EventCollector::new();
    core.events().subscribe(events.clone());
    start(&core, start_options(&testnet, "rs-app-login", None))
        .await
        .expect("start");
    assert!(events.wait_for(
        |events| events.iter().any(|event| {
            event["type"] == "tailnet.state"
                && event["state"] == "needsLogin"
                && event["authUrl"]
                    .as_str()
                    .is_some_and(|url| url.contains("/auth/"))
        }),
        Duration::from_secs(15),
    ));
    let first_url = tailnet_events(&events)
        .into_iter()
        .find_map(|event| {
            (event["state"] == "needsLogin")
                .then(|| event["authUrl"].as_str().map(str::to_owned))
                .flatten()
        })
        .expect("first auth URL");
    testnet.complete(&first_url).expect("complete login");
    wait_running(&events);

    rs_mobile::rscore::api_tailnet::logout(&core)
        .await
        .expect("logout");
    assert!(events.wait_for(
        |events| events
            .iter()
            .any(|event| { event["type"] == "tailnet.state" && event["state"] == "needsLogin" }),
        Duration::from_secs(15),
    ));
    let before_login = tailnet_events(&events).len();
    rs_mobile::rscore::api_tailnet::login(&core)
        .await
        .expect("login");
    assert!(events.wait_for(
        |events| {
            events[before_login..].iter().any(|event| {
                event["state"] == "needsLogin"
                    && event["authUrl"]
                        .as_str()
                        .is_some_and(|url| url.contains("/auth/"))
            })
        },
        Duration::from_secs(15),
    ));
    let second_url = tailnet_events(&events)[before_login..]
        .iter()
        .find_map(|event| {
            (event["state"] == "needsLogin")
                .then(|| event["authUrl"].as_str().map(str::to_owned))
                .flatten()
        })
        .expect("second auth URL");
    testnet
        .complete(&second_url)
        .expect("complete second login");
    wait_running(&events);
    stop(&core).await.expect("stop");
    core.shutdown();
}

#[tokio::test]
async fn net_dialer_follows_state() {
    let Some(paths) = rs_tailnet::testkit::paths_or_skip("net_dialer_follows_state") else {
        return;
    };
    let testnet = TestNet::spawn(
        &paths,
        &TestNetOptions {
            auth_key: Some("tskey-rs-mobile-dialer".to_owned()),
            ..Default::default()
        },
    )
    .expect("testnet");
    let core = test_core();
    let events = EventCollector::new();
    core.events().subscribe(events.clone());
    assert!(net_dialer(&core).is_none());
    start(
        &core,
        start_options(&testnet, "rs-app-dialer", Some(testnet.auth_key.clone())),
    )
    .await
    .expect("start");
    wait_running(&events);
    let dialer = net_dialer(&core).expect("running dialer");
    let mut stream = dialer
        .dial_tcp_host("rs-peer", testnet.tcp_echo)
        .await
        .expect("dial peer");
    let payload: Vec<u8> = (0..4096).map(|index| (index % 251) as u8).collect();
    stream.write_all(&payload).await.expect("write");
    let mut received = vec![0; payload.len()];
    stream.read_exact(&mut received).await.expect("read");
    assert_eq!(received, payload);

    stop(&core).await.expect("stop");
    assert!(net_dialer(&core).is_none());
    assert!(events.wait_for(
        |events| events
            .iter()
            .any(|event| event["type"] == "tailnet.state" && event["state"] == "stopped"),
        Duration::from_secs(5),
    ));
    let status: Value =
        serde_json::from_str(&status_json(&core).await.expect("status")).expect("status JSON");
    assert_eq!(status["state"], "stopped");
    core.shutdown();
}

#[tokio::test]
async fn restart_and_idempotency() {
    let Some(paths) = rs_tailnet::testkit::paths_or_skip("restart_and_idempotency") else {
        return;
    };
    let testnet = TestNet::spawn(
        &paths,
        &TestNetOptions {
            auth_key: Some("tskey-rs-mobile-restart".to_owned()),
            ..Default::default()
        },
    )
    .expect("testnet");
    let core = test_core();
    let events = EventCollector::new();
    core.events().subscribe(events.clone());
    let opts = start_options(&testnet, "rs-app-restart", Some(testnet.auth_key.clone()));
    start(&core, opts.clone()).await.expect("start");
    let first = wait_running(&events);
    assert!(matches!(
        start(&core, opts.clone()).await,
        Err(CoreError::Internal { message }) if message.contains("already started")
    ));
    stop(&core).await.expect("stop");
    stop(&core).await.expect("idempotent stop");
    // The test helper currently requires its auth key on every registration;
    // use it again here while still checking that the persisted node keeps its IP.
    start(&core, opts).await.expect("restart");
    let second = wait_running(&events);
    assert_eq!(first["ips"], second["ips"]);
    stop(&core).await.expect("stop");
    core.shutdown();
}

#[test]
fn bad_options() {
    for json in [
        r#"{"controlUrl":"https://hs.example.com","hostname":"a","foo":1}"#,
        r#"{"controlUrl":"ftp://x","hostname":"a"}"#,
        r#"{"controlUrl":"https://hs.example.com","hostname":"  "}"#,
        "not json",
    ] {
        assert!(matches!(
            parse_start_options(json),
            Err(CoreError::Internal { message }) if message.contains("invalid tailnet options")
        ));
    }
}

#[tokio::test]
async fn shutdown_hook_stops_node() {
    let Some(paths) = rs_tailnet::testkit::paths_or_skip("shutdown_hook_stops_node") else {
        return;
    };
    let testnet = TestNet::spawn(
        &paths,
        &TestNetOptions {
            auth_key: Some("tskey-rs-mobile-shutdown".to_owned()),
            ..Default::default()
        },
    )
    .expect("testnet");
    let core = test_core();
    let events = EventCollector::new();
    core.events().subscribe(events.clone());
    start(
        &core,
        start_options(&testnet, "rs-app-shutdown", Some(testnet.auth_key.clone())),
    )
    .await
    .expect("start");
    wait_running(&events);
    core.shutdown();
    let api = rs_tailnet::load_default().expect("loaded tailnet");
    let node = rs_tailnet::Tailnet::global(api);
    assert!(
        (0..50).any(|_| {
            let stopped = node
                .status()
                .map(|status| status.state == rs_tailnet::BackendState::Stopped)
                .unwrap_or(false);
            if !stopped {
                std::thread::sleep(Duration::from_millis(100));
            }
            stopped
        }),
        "shutdown hook did not stop tailnet",
    );
}

#[tokio::test]
async fn no_duplicate_events() {
    let Some(paths) = rs_tailnet::testkit::paths_or_skip("no_duplicate_events") else {
        return;
    };
    let testnet = TestNet::spawn(
        &paths,
        &TestNetOptions {
            auth_key: Some("tskey-rs-mobile-dedup".to_owned()),
            ..Default::default()
        },
    )
    .expect("testnet");
    let core = test_core();
    let events = EventCollector::new();
    core.events().subscribe(events.clone());
    start(
        &core,
        start_options(&testnet, "rs-app-dedup", Some(testnet.auth_key.clone())),
    )
    .await
    .expect("start");
    wait_running(&events);
    tokio::time::sleep(Duration::from_secs(3)).await;
    let values = tailnet_events(&events);
    assert!(values.windows(2).all(|pair| pair[0] != pair[1]));
    stop(&core).await.expect("stop");
    core.shutdown();
}
