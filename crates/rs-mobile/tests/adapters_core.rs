mod support;

use std::future::Future;
use std::pin::Pin;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rs_mobile::rscore::adapters::{
    driver::{
        AdapterError, Credentials, Dial, DriverHost, DriverInput, DriverParams, FrameSink,
        ProtocolDriver,
    },
    dto::{
        AdapterOpenOpts, AdapterPhase, AdapterProtocol, AdapterScreen, AdapterTarget, CredField,
    },
    rawframe::RawRegistry,
    secrets::{self, MemSecretStore},
    session::{self, colon_fp, open_with, open_with_timings, NetDial, SessionTimings},
    ADAPTER_ID_BASE,
};
use rs_mobile::rscore::error::CoreError;
use rs_mobile::rscore::runtime::Core;
use rsp_transport::net::{NetDialer, NetFuture, NetStream};
use serde_json::Value;
use tokio::sync::mpsc as tokio_mpsc;
use zeroize::Zeroizing;

type ScriptFuture = Pin<Box<dyn Future<Output = Result<(), AdapterError>> + Send + 'static>>;
type Script = dyn Fn(
        DriverParams,
        Arc<dyn DriverHost>,
        Arc<dyn FrameSink>,
        tokio_mpsc::Receiver<DriverInput>,
    ) -> ScriptFuture
    + Send
    + Sync;

struct ScriptDriver {
    protocol: AdapterProtocol,
    script: Arc<Script>,
}

impl ScriptDriver {
    fn new(
        script: impl Fn(
                DriverParams,
                Arc<dyn DriverHost>,
                Arc<dyn FrameSink>,
                tokio_mpsc::Receiver<DriverInput>,
            ) -> ScriptFuture
            + Send
            + Sync
            + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            protocol: AdapterProtocol::Vnc,
            script: Arc::new(script),
        })
    }
}

#[async_trait]
impl ProtocolDriver for ScriptDriver {
    fn protocol(&self) -> AdapterProtocol {
        self.protocol
    }

    async fn run(
        &self,
        params: DriverParams,
        host: Arc<dyn DriverHost>,
        sink: Arc<dyn FrameSink>,
        input: tokio_mpsc::Receiver<DriverInput>,
    ) -> Result<(), AdapterError> {
        (self.script)(params, host, sink, input).await
    }
}

struct NoDial;

#[async_trait]
impl Dial for NoDial {
    async fn dial(&self) -> Result<NetStream, AdapterError> {
        Err(AdapterError::Refused)
    }
}

fn target() -> AdapterTarget {
    AdapterTarget {
        protocol: AdapterProtocol::Vnc,
        host_id: Some("h1".to_owned()),
        addr: "100.64.0.7".to_owned(),
        port: None,
    }
}

fn opts() -> AdapterOpenOpts {
    AdapterOpenOpts::default()
}

fn open(core: &Arc<Core>, driver: Arc<ScriptDriver>) -> (u64, Arc<support::EventCollector>) {
    let collector = support::EventCollector::new();
    core.events().subscribe(collector.clone());
    let id = core
        .block_on(open_with(core, target(), opts(), driver, Arc::new(NoDial)))
        .expect("open adapter session");
    (id, collector)
}

fn wait_ended(collector: &support::EventCollector, id: u64) -> Value {
    let session_id = id.to_string();
    assert!(collector.wait_for(
        |events| events.iter().any(|event| {
            event["type"] == "adapter.state"
                && event["sessionId"].as_str() == Some(session_id.as_str())
                && event["phase"] == "ended"
        }),
        Duration::from_secs(5),
    ));
    collector
        .of_type("adapter.state")
        .into_iter()
        .find(|event| {
            event["sessionId"].as_str() == Some(session_id.as_str()) && event["phase"] == "ended"
        })
        .expect("ended event")
}

fn credentials(user: &str, password: &str) -> Credentials {
    Credentials {
        user: Some(user.to_owned()),
        password: Zeroizing::new(password.to_owned()),
        domain: None,
    }
}

#[test]
fn open_registers_and_closes() {
    let core = support::test_core();
    let driver = ScriptDriver::new(|_, _, _, mut input| {
        Box::pin(async move {
            let _ = input.recv().await;
            Ok(())
        })
    });
    let (id, collector) = open(&core, driver);
    let session_id = id.to_string();
    assert_eq!(id, ADAPTER_ID_BASE + 1);
    assert!(collector.wait_for(
        |events| events.iter().any(|event| {
            event["type"] == "adapter.state"
                && event["sessionId"].as_str() == Some(session_id.as_str())
                && event["phase"] == "connecting"
                && event["protocol"] == "vnc"
                && event["credError"] == false
        }),
        Duration::from_secs(5),
    ));
    assert_eq!(session::list(&core).len(), 1);
    assert!(core.state::<RawRegistry>().get(id).is_some());
    assert!(session::close(&core, id));
    let ended = wait_ended(&collector, id);
    assert!(ended.get("error").is_none());
    assert!(collector.wait_for(
        |_| session::list(&core).is_empty() && core.state::<RawRegistry>().get(id).is_none(),
        Duration::from_secs(5),
    ));
}

#[test]
fn streaming_and_frame_events() {
    let core = support::test_core();
    let driver = ScriptDriver::new(|_, host, sink, mut input| {
        Box::pin(async move {
            host.authenticating();
            host.streaming(800, 600, "mac");
            sink.resize(4, 3);
            sink.fill(0, 0, 4, 3, [1, 2, 3, 255]);
            sink.commit();
            host.clipboard("hi");
            host.screens(&[
                AdapterScreen {
                    x: 0,
                    y: 0,
                    width: 1024,
                    height: 768,
                    guess: None,
                },
                AdapterScreen {
                    x: 1024,
                    y: 0,
                    width: 1024,
                    height: 768,
                    guess: None,
                },
            ]);
            let _ = input.recv().await;
            Ok(())
        })
    });
    let (id, collector) = open(&core, driver);
    let session_id = id.to_string();
    assert!(collector.wait_for(
        |events| {
            let states: Vec<_> = events
                .iter()
                .filter(|event| {
                    event["type"] == "adapter.state"
                        && event["sessionId"].as_str() == Some(session_id.as_str())
                })
                .collect();
            states.len() >= 3
                && states[0]["phase"] == "connecting"
                && states[1]["phase"] == "authenticating"
                && states[2]["phase"] == "streaming"
                && states[2]["width"] == 800
                && states[2]["height"] == 600
        },
        Duration::from_secs(5),
    ));
    let listed = session::list(&core);
    assert_eq!(listed[0].phase, AdapterPhase::Streaming);
    assert_eq!((listed[0].width, listed[0].height), (Some(800), Some(600)));
    assert!(collector.wait_for(
        |events| events
            .iter()
            .any(|event| { event["type"] == "adapter.clipboard" && event["text"] == "hi" })
            && events.iter().any(|event| {
                event["type"] == "adapter.screens"
                    && event["screens"].as_array().unwrap().len() == 2
            }),
        Duration::from_secs(5),
    ));
    assert_eq!(core.state::<RawRegistry>().get(id).unwrap().seq(), 1);
    session::close(&core, id);
    let _ = wait_ended(&collector, id);
}

#[test]
fn driver_error_and_replacement() {
    let core = support::test_core();
    let error_driver =
        ScriptDriver::new(|_, _, _, _| Box::pin(async { Err(AdapterError::AuthFailed) }));
    let (first, first_events) = open(&core, error_driver);
    let ended = wait_ended(&first_events, first);
    assert_eq!(ended["error"]["code"], "authFailed");
    assert_eq!(ended["error"]["rsCode"], 201);
    assert_eq!(ended["error"]["message"], "authentication failed");

    let stuck = ScriptDriver::new(|_, _, _, _| Box::pin(std::future::pending()));
    let (old, old_events) = open(&core, stuck.clone());
    let (new, new_events) = open(&core, stuck);
    assert_eq!(new, old + 1);
    let ended = wait_ended(&old_events, old);
    assert!(ended.get("error").is_none());
    assert!(session::list(&core)
        .iter()
        .all(|summary| { summary.session_id == new.to_string() }));
    session::close(&core, new);
    let _ = wait_ended(&new_events, new);
}

#[test]
fn credentials_submit_cancel_and_send_input() {
    let core = support::test_core();
    let (cred_tx, cred_rx) = mpsc::channel();
    let driver = ScriptDriver::new(move |_, host, _, mut input| {
        let cred_tx = cred_tx.clone();
        Box::pin(async move {
            let got = host
                .credentials(&[CredField::User, CredField::Password], false)
                .await;
            cred_tx
                .send(got.map(|value| (value.user, value.password.to_string())))
                .unwrap();
            let _ = input.recv().await;
            Ok(())
        })
    });
    let (id, collector) = open(&core, driver);
    assert!(collector.wait_for(
        |events| events.iter().any(|event| {
            event["phase"] == "needsCredentials"
                && event["credFields"] == serde_json::json!(["user", "password"])
        }),
        Duration::from_secs(5),
    ));
    session::submit_credentials(&core, id, credentials("alice", "pw"), false).unwrap();
    assert_eq!(
        cred_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        Some((Some("alice".into()), "pw".into()))
    );
    assert_eq!(
        session::submit_credentials(&core, id, credentials("again", "pw"), false),
        Err(CoreError::internal("not waiting for credentials"))
    );
    session::send_input(&core, id, DriverInput::CtrlAltDel).unwrap();
    session::close(&core, id);
    let _ = wait_ended(&collector, id);

    let (cancel_tx, cancel_rx) = mpsc::channel();
    let cancel_driver = ScriptDriver::new(move |_, host, _, mut input| {
        let cancel_tx = cancel_tx.clone();
        Box::pin(async move {
            cancel_tx
                .send(
                    host.credentials(&[CredField::Password], false)
                        .await
                        .is_none(),
                )
                .unwrap();
            let _ = input.recv().await;
            Ok(())
        })
    });
    let (cancel_id, cancel_events) = open(&core, cancel_driver);
    let cancel_session_id = cancel_id.to_string();
    assert!(collector.wait_for(
        |events| events.iter().any(|event| {
            event["sessionId"].as_str() == Some(cancel_session_id.as_str())
                && event["phase"] == "needsCredentials"
        }),
        Duration::from_secs(5),
    ));
    session::cancel_credentials(&core, cancel_id);
    assert!(cancel_rx.recv_timeout(Duration::from_secs(5)).unwrap());
    session::close(&core, cancel_id);
    let _ = wait_ended(&cancel_events, cancel_id);

    let (close_tx, close_rx) = mpsc::channel();
    let close_driver = ScriptDriver::new(move |_, host, _, mut input| {
        let close_tx = close_tx.clone();
        Box::pin(async move {
            close_tx
                .send(
                    host.credentials(&[CredField::Password], false)
                        .await
                        .is_none(),
                )
                .unwrap();
            let _ = input.recv().await;
            Ok(())
        })
    });
    let (close_id, close_events) = open(&core, close_driver);
    let close_session_id = close_id.to_string();
    assert!(close_events.wait_for(
        |events| events.iter().any(|event| event["sessionId"].as_str()
            == Some(close_session_id.as_str())
            && event["phase"] == "needsCredentials"),
        Duration::from_secs(5),
    ));
    session::close(&core, close_id);
    assert!(close_rx.recv_timeout(Duration::from_secs(5)).unwrap());
    let _ = wait_ended(&close_events, close_id);
}

#[test]
fn saved_credentials_and_remember() {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap();
    secrets::set_registered(None);
    let core = support::test_core();
    let store = MemSecretStore::new();
    secrets::set_registered(Some(store.clone()));
    secrets::save(&core, "vnc:h1", &credentials("alice", "pw")).unwrap();
    let (saved_tx, saved_rx) = mpsc::channel();
    let driver = ScriptDriver::new(move |_, host, _, mut input| {
        let saved_tx = saved_tx.clone();
        Box::pin(async move {
            let first = host
                .credentials(&[CredField::User, CredField::Password], false)
                .await;
            saved_tx.send(first.map(|c| c.user)).unwrap();
            let second = host
                .credentials(&[CredField::User, CredField::Password], true)
                .await;
            saved_tx.send(second.map(|c| c.user)).unwrap();
            let _ = input.recv().await;
            Ok(())
        })
    });
    let (id, events) = open(&core, driver);
    let session_id = id.to_string();
    assert_eq!(
        saved_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        Some(Some("alice".into()))
    );
    assert!(events.wait_for(
        |items| items.iter().any(|event| {
            event["sessionId"].as_str() == Some(session_id.as_str())
                && event["phase"] == "needsCredentials"
                && event["credError"] == true
        }),
        Duration::from_secs(5),
    ));
    session::submit_credentials(&core, id, credentials("bob", "pw2"), true).unwrap();
    assert_eq!(
        saved_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        Some(Some("bob".into()))
    );
    // The driver has not called auth_succeeded, so no replacement has been saved.
    assert_eq!(store.len(), 1);
    session::close(&core, id);
    let _ = wait_ended(&events, id);
    secrets::set_registered(None);
}

#[test]
fn remember_is_saved_after_auth_success() {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap();
    secrets::set_registered(None);
    let core = support::test_core();
    let store = MemSecretStore::new();
    secrets::set_registered(Some(store.clone()));
    let (ready_tx, ready_rx) = mpsc::channel();
    let driver = ScriptDriver::new(move |_, host, _, mut input| {
        let ready_tx = ready_tx.clone();
        Box::pin(async move {
            let _ = host
                .credentials(&[CredField::User, CredField::Password], false)
                .await;
            host.auth_succeeded();
            ready_tx.send(()).unwrap();
            let _ = input.recv().await;
            Ok(())
        })
    });
    let (id, events) = open(&core, driver);
    assert!(events.wait_for(
        |items| items
            .iter()
            .any(|event| event["phase"] == "needsCredentials"),
        Duration::from_secs(5),
    ));
    session::submit_credentials(&core, id, credentials("alice", "pw"), true).unwrap();
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let started = std::time::Instant::now();
    while store.raw("vnc:h1").is_none() && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(store.raw("vnc:h1").is_some());
    session::close(&core, id);
    let _ = wait_ended(&events, id);
    secrets::set_registered(None);
}

#[test]
fn certificate_tofu_and_timeout() {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap();
    secrets::set_registered(None);
    let core = support::test_core();
    let (cert_tx, cert_rx) = mpsc::channel();
    let driver = ScriptDriver::new(move |_, host, _, mut input| {
        let cert_tx = cert_tx.clone();
        Box::pin(async move {
            cert_tx
                .send(host.confirm_cert(&"ab".repeat(32)).await)
                .unwrap();
            cert_tx
                .send(host.confirm_cert(&"ab".repeat(32)).await)
                .unwrap();
            let changed = host.confirm_cert(&"cd".repeat(32)).await;
            cert_tx.send(changed).unwrap();
            let _ = input.recv().await;
            Ok(())
        })
    });
    let (id, events) = open(&core, driver);
    assert!(cert_rx.recv_timeout(Duration::from_secs(5)).unwrap());
    assert!(cert_rx.recv_timeout(Duration::from_secs(5)).unwrap());
    assert!(events.wait_for(
        |items| items
            .iter()
            .any(|event| event["type"] == "adapter.certPrompt"),
        Duration::from_secs(5),
    ));
    session::trust_cert(&core, id, true);
    assert!(cert_rx.recv_timeout(Duration::from_secs(5)).unwrap());
    assert_eq!(
        core.store().kv_get("adapter.cert.vnc:h1").unwrap(),
        Some("cd".repeat(32))
    );
    session::close(&core, id);
    let _ = wait_ended(&events, id);

    let timeout_driver = ScriptDriver::new(|_, host, _, mut input| {
        Box::pin(async move {
            assert!(!host.confirm_cert(&"ef".repeat(32)).await);
            let _ = input.recv().await;
            Ok(())
        })
    });
    let collector = support::EventCollector::new();
    core.events().subscribe(collector.clone());
    let timeout_id = core
        .block_on(open_with_timings(
            &core,
            target(),
            opts(),
            timeout_driver,
            Arc::new(NoDial),
            SessionTimings {
                connect: Duration::from_secs(15),
                cert_prompt: Duration::from_millis(50),
            },
        ))
        .unwrap();
    let timeout_session_id = timeout_id.to_string();
    std::thread::sleep(Duration::from_millis(100));
    session::close(&core, timeout_id);
    assert!(collector.wait_for(
        |items| items.iter().any(|event| {
            event["phase"] == "ended"
                && event["sessionId"].as_str() == Some(timeout_session_id.as_str())
        }),
        Duration::from_secs(5),
    ));
    secrets::set_registered(None);
}

#[test]
fn net_mapping_invalid_targets_and_fingerprint() {
    assert_eq!(colon_fp(&"ab".repeat(32)).len(), 95);
    assert!(colon_fp(&"ab".repeat(32)).starts_with("ab:ab"));
    assert_eq!(colon_fp("abc"), "abc");

    let core = support::test_core();
    for addr in ["", "a b", "x\u{7}"] {
        let mut bad = target();
        bad.addr = addr.to_owned();
        let result = core.block_on(open_with(
            &core,
            bad,
            opts(),
            session::driver_for(AdapterProtocol::Vnc),
            Arc::new(NoDial),
        ));
        assert_eq!(result, Err(CoreError::internal("invalid adapter target")));
    }
    let mut bad = target();
    bad.port = Some(0);
    assert_eq!(
        core.block_on(open_with(
            &core,
            bad,
            opts(),
            session::driver_for(AdapterProtocol::Vnc),
            Arc::new(NoDial),
        )),
        Err(CoreError::internal("invalid adapter target"))
    );
    assert!(session::list(&core).is_empty());
}

#[derive(Debug)]
struct DummyNet;

impl NetDialer for DummyNet {
    fn shared_quic_endpoint(
        &self,
        _bind: std::net::SocketAddr,
    ) -> std::io::Result<Option<quinn::Endpoint>> {
        Ok(None)
    }

    fn dial_udp(
        &self,
        _peer: std::net::SocketAddr,
    ) -> NetFuture<'_, Arc<dyn quinn::AsyncUdpSocket>> {
        Box::pin(async { Err(std::io::Error::other("unused")) })
    }

    fn dial_tcp(&self, _peer: std::net::SocketAddr) -> NetFuture<'_, NetStream> {
        Box::pin(async { Err(std::io::Error::other("unused")) })
    }

    fn dial_tcp_host<'a>(&'a self, _host: &'a str, _port: u16) -> NetFuture<'a, NetStream> {
        Box::pin(async {
            Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "no",
            ))
        })
    }
}

#[test]
fn net_dial_refused_mapping() {
    let core = support::test_core_with(|_, options| options.net = Some(Arc::new(DummyNet)));
    let dial = NetDial::new(&core, &target(), Duration::from_millis(50));
    assert!(matches!(
        core.block_on(dial.dial()),
        Err(AdapterError::Refused)
    ));
}

#[test]
fn background_routes_visibility_idempotently_and_discards_paused_input() {
    use rs_mobile::rscore::api_sessions::background;
    use rsp_core::hid::InputMsg;

    let core = support::test_core();
    for protocol in [AdapterProtocol::Vnc, AdapterProtocol::Rdp] {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (input_tx, input_rx) = mpsc::channel();
        let driver = Arc::new(ScriptDriver {
            protocol,
            script: Arc::new(move |_, host, _, mut input| {
                ready_tx.send(host.background()).unwrap();
                let input_tx = input_tx.clone();
                Box::pin(async move {
                    while let Some(message) = input.recv().await {
                        let kind = match message {
                            DriverInput::Clipboard(text) => text,
                            _ => "input".into(),
                        };
                        input_tx.send(kind).unwrap();
                    }
                    Ok(())
                })
            }),
        });
        let mut destination = target();
        destination.protocol = protocol;
        let id = core
            .block_on(open_with(
                &core,
                destination,
                opts(),
                driver,
                Arc::new(NoDial),
            ))
            .unwrap();
        let mut visibility = ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!*visibility.borrow_and_update());
        for paused in [true, false] {
            background(&core, id, paused).unwrap();
            assert!(visibility.has_changed().unwrap());
            assert_eq!(*visibility.borrow_and_update(), paused);
            background(&core, id, paused).unwrap();
            assert!(!visibility.has_changed().unwrap());
            session::send_input(&core, id, DriverInput::Hid(InputMsg::Text("a".into()))).unwrap();
            session::send_input(&core, id, DriverInput::Clipboard("barrier".into())).unwrap();
            if !paused {
                assert_eq!(
                    input_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
                    "input"
                );
            }
            assert_eq!(
                input_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
                "barrier"
            );
            assert!(input_rx.try_recv().is_err());
        }
        assert!(session::close(&core, id));
        core.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                while session::entry(&core, id).is_some() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        });
        assert_eq!(
            background(&core, id, false),
            Err(CoreError::internal(format!("unknown session {id}")))
        );
    }
    for id in [999, ADAPTER_ID_BASE + 999] {
        for paused in [true, false] {
            assert_eq!(
                background(&core, id, paused),
                Err(CoreError::internal(format!("unknown session {id}")))
            );
        }
    }
}
