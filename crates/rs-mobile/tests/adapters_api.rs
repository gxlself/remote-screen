//! RsCore.adapters exports and input routing against fake servers.

mod support;

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rs_mobile::rscore::adapters::{
    api::{
        adapters_can_save, adapters_cancel_credentials, adapters_close, adapters_forget,
        adapters_list, adapters_open, adapters_resize, adapters_saved, adapters_send_ctrl_alt_del,
        adapters_set_monitors, adapters_submit_credentials, adapters_trust_cert,
        adapters_type_password,
    },
    dto::AdapterTarget,
    is_adapter_id,
    secrets::{self, MemSecretStore},
    ADAPTER_ID_BASE,
};
use rs_mobile::rscore::api_sessions::{
    sessions_action, sessions_clipboard_send, sessions_combo, sessions_key, sessions_text,
};
use rs_mobile::rscore::error::CoreError;
use rs_mobile::rscore::ffi_session::{rs_input_button, rs_input_pointer};
use rs_mobile::rscore::runtime::{install_core, take_core, Core};
use rs_rdp::testkit::{FakeInput, FakeRdpServer, FakeSecurity};
use rs_vnc::{
    proto::ClientMsg,
    security,
    testkit::{listen_tcp, AuthCheck, FakeHandle, FakeLog, SecurityPlan, ServerScript},
};
use serde_json::{json, Value};
use tokio::sync::mpsc;

const WAIT: Duration = Duration::from_secs(20);
const SECRET: &str = "hunter2-not-for-logs";

/// install_core and the secret store registration are process-wide.
static LOCK: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

struct Env {
    core: Arc<Core>,
    events: Arc<support::EventCollector>,
    _serial: MutexGuard<'static, ()>,
}

impl Env {
    fn new() -> Self {
        let serial = serial();
        secrets::set_registered(None);
        let core = support::test_core();
        install_core(core.clone());
        let events = support::EventCollector::new();
        core.events().subscribe(events.clone());
        Self {
            core,
            events,
            _serial: serial,
        }
    }

    fn open(&self, target: &str, opts: &str) -> u64 {
        self.core
            .block_on(adapters_open(target.to_owned(), opts.to_owned()))
            .expect("adapters_open")
    }

    fn state(&self, id: u64, phase: &str) -> Option<Value> {
        let session_id = id.to_string();
        self.events
            .of_type("adapter.state")
            .into_iter()
            .find(|event| {
                event["sessionId"].as_str() == Some(session_id.as_str()) && event["phase"] == phase
            })
    }

    fn wait_state(&self, id: u64, phase: &str) -> Value {
        assert!(
            self.events
                .wait_for(|_| self.state(id, phase).is_some(), WAIT),
            "no {phase} event for {id}"
        );
        self.state(id, phase).unwrap()
    }

    fn next_server(&self, servers: &mut mpsc::UnboundedReceiver<FakeHandle>) -> FakeHandle {
        self.core
            .block_on(async { tokio::time::timeout(WAIT, servers.recv()).await })
            .expect("server timeout")
            .expect("server handle")
    }

    /// Exactly `expected` input messages, in order; end with a marker so the
    /// inputs before it are known to be complete.
    fn expect_inputs(&self, server: &FakeHandle, expected: &[ClientMsg]) {
        let arrived = self
            .core
            .block_on(server.wait_for(|log| inputs(log).len() >= expected.len(), WAIT));
        assert!(arrived, "server saw {:?}", inputs(&server.log()));
        assert_eq!(inputs(&server.log()), expected);
    }

    fn close(&self, id: u64) {
        assert_eq!(adapters_close(id), Ok(()));
        let ended = self.wait_state(id, "ended");
        assert!(ended.get("error").is_none(), "{ended}");
        let session_id = id.to_string();
        wait_until("session removed", || {
            list()
                .iter()
                .all(|summary| summary["sessionId"] != session_id.as_str())
        });
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        secrets::set_registered(None);
        take_core();
    }
}

fn wait_until(what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn list() -> Vec<Value> {
    serde_json::from_str(&adapters_list().unwrap()).unwrap()
}

fn saved(target: &str) -> Value {
    serde_json::from_str(&adapters_saved(target.to_owned()).unwrap()).unwrap()
}

fn vnc_target(port: u16) -> String {
    json!({"protocol": "vnc", "addr": "127.0.0.1", "port": port}).to_string()
}

fn opts() -> String {
    json!({"viewWidth": 390, "viewHeight": 844, "scale": 3}).to_string()
}

fn inputs(log: &FakeLog) -> Vec<ClientMsg> {
    log.msgs
        .iter()
        .filter(|message| {
            matches!(
                message,
                ClientMsg::KeyEvent { .. }
                    | ClientMsg::PointerEvent { .. }
                    | ClientMsg::ClientCutText(_)
            )
        })
        .cloned()
        .collect()
}

fn key(down: bool, keysym: u32) -> ClientMsg {
    ClientMsg::KeyEvent { down, keysym }
}

fn ptr(mask: u8, x: u16, y: u16) -> ClientMsg {
    ClientMsg::PointerEvent { mask, x, y }
}

fn typed(text: &str) -> Vec<ClientMsg> {
    text.chars()
        .flat_map(|c| [key(true, u32::from(c)), key(false, u32::from(c))])
        .collect()
}

fn vnc_password(password: &str) -> ServerScript {
    ServerScript {
        security: SecurityPlan::Offer {
            types: vec![security::VNC_AUTH],
            check: AuthCheck::VncPassword(password.to_owned()),
        },
        ..ServerScript::new(801, 601)
    }
}

fn internal(result: Result<(), CoreError>) -> String {
    match result {
        Err(CoreError::Internal { message }) => message,
        other => panic!("expected an internal error, got {other:?}"),
    }
}

#[test]
fn vnc_open_list_input_close() {
    let env = Env::new();
    let (addr, mut servers) = env
        .core
        .block_on(listen_tcp(vec![ServerScript::new(801, 601)]))
        .expect("listen on localhost");
    let id = env.open(&vnc_target(addr.port()), &opts());
    assert!(is_adapter_id(id));
    let server = env.next_server(&mut servers);
    let streaming = env.wait_state(id, "streaming");
    assert_eq!(streaming["protocol"], "vnc");
    assert_eq!(streaming["width"], 801);
    assert_eq!(streaming["height"], 601);

    let sessions = list();
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    assert_eq!(sessions[0]["sessionId"], id.to_string());
    assert_eq!(sessions[0]["phase"], "streaming");
    assert_eq!(
        sessions[0]["target"],
        json!({"protocol": "vnc", "addr": "127.0.0.1", "port": addr.port()})
    );

    rs_input_pointer(id, 0.5, 0.5);
    rs_input_button(id, 0, true, 0.5, 0.5, 1);
    rs_input_button(id, 0, false, 0.5, 0.5, 1);
    sessions_key(id, 0x04, true, 0).unwrap();
    sessions_key(id, 0x04, false, 0).unwrap();
    sessions_text(id, "b".to_owned()).unwrap();
    sessions_combo(id, "altTab".to_owned()).unwrap();
    sessions_clipboard_send(id, "café".to_owned()).unwrap();
    adapters_send_ctrl_alt_del(id).unwrap();
    // VNC ignores the view size and the monitor count.
    assert_eq!(adapters_resize(id, 844, 390, 3.0), Ok(()));
    assert_eq!(adapters_set_monitors(id, 2), Ok(()));
    rs_input_pointer(id, 0.0, 0.0);
    env.expect_inputs(
        &server,
        &[
            ptr(0, 400, 300),
            ptr(1, 400, 300),
            ptr(0, 400, 300),
            key(true, 0x61),
            key(false, 0x61),
            key(true, 0x62),
            key(false, 0x62),
            key(true, 0xffe9),
            key(true, 0xff09),
            key(false, 0xff09),
            key(false, 0xffe9),
            ClientMsg::ClientCutText(b"caf\xe9".to_vec()),
            key(true, 0xffe3),
            key(true, 0xffe9),
            key(true, 0xffff),
            key(false, 0xffff),
            key(false, 0xffe9),
            key(false, 0xffe3),
            ptr(0, 0, 0),
        ],
    );

    assert!(matches!(
        sessions_clipboard_send(id, "你好".to_owned()),
        Err(CoreError::Unsupported { .. })
    ));
    assert!(matches!(
        sessions_clipboard_send(id, String::new()),
        Err(CoreError::Unsupported { .. })
    ));
    assert!(sessions_combo(id, "notACombo".to_owned()).is_err());
    assert_eq!(
        adapters_resize(id, 0, 390, 3.0),
        Err(CoreError::internal("invalid view size"))
    );
    assert_eq!(
        adapters_resize(id, 844, 390, f32::NAN),
        Err(CoreError::internal("invalid view size"))
    );
    for count in [0, 5] {
        assert_eq!(
            adapters_set_monitors(id, count),
            Err(CoreError::internal("invalid monitor count"))
        );
    }
    // No secret store, nothing submitted: nothing to type.
    assert!(internal(adapters_type_password(id)).contains("no saved password"));
    // Not waiting for credentials or a certificate answer.
    assert!(adapters_submit_credentials(id, None, "pw".to_owned(), None, false).is_err());
    assert_eq!(adapters_cancel_credentials(id), Ok(()));
    assert_eq!(adapters_trust_cert(id, true), Ok(()));
    assert!(env.state(id, "ended").is_none());

    env.close(id);
    assert!(list().is_empty());
    assert!(internal(sessions_key(id, 0x04, true, 0)).contains("unknown adapter session"));
}

/// The keyboard bar's Ctrl+Alt+Del goes through sessions.action.
#[test]
fn vnc_session_action_ctrl_alt_del_and_ctrl_space() {
    let env = Env::new();
    let (addr, mut servers) = env
        .core
        .block_on(listen_tcp(vec![ServerScript::new(801, 601)]))
        .expect("listen on localhost");
    let id = env.open(&vnc_target(addr.port()), &opts());
    let server = env.next_server(&mut servers);
    env.wait_state(id, "streaming");

    sessions_action(id, "ctrlAltDel".to_owned(), None).unwrap();
    sessions_combo(id, "ctrlSpace".to_owned()).unwrap();
    env.expect_inputs(
        &server,
        &[
            key(true, 0xffe3),
            key(true, 0xffe9),
            key(true, 0xffff),
            key(false, 0xffff),
            key(false, 0xffe9),
            key(false, 0xffe3),
            key(true, 0xffe3),
            key(true, 0x20),
            key(false, 0x20),
            key(false, 0xffe3),
        ],
    );

    for kind in ["lock", "lockOnEnd"] {
        assert!(
            matches!(
                sessions_action(id, kind.to_owned(), None),
                Err(CoreError::Unsupported { .. })
            ),
            "{kind}"
        );
    }
    assert!(sessions_action(id, "notAnAction".to_owned(), None).is_err());

    env.close(id);
    assert!(internal(sessions_action(id, "ctrlAltDel".to_owned(), None))
        .contains("unknown adapter session"));
}

#[test]
fn vnc_password_saved_typed_and_forgotten() {
    let env = Env::new();
    let store = MemSecretStore::new();
    secrets::set_registered(Some(store.clone()));
    assert!(adapters_can_save());
    let (addr, mut servers) = env
        .core
        .block_on(listen_tcp(vec![vnc_password("pw-1")]))
        .expect("listen on localhost");
    let target = vnc_target(addr.port());
    assert_eq!(saved(&target), json!({"saved": false}));

    let id = env.open(&target, &opts());
    let asked = env.wait_state(id, "needsCredentials");
    assert_eq!(asked["credFields"], json!(["password"]));
    assert_eq!(asked["credError"], false);
    assert!(internal(adapters_type_password(id)).contains("no saved password"));

    // Rejected before reaching the session, which keeps waiting.
    for (user, password) in [
        (None, format!("{SECRET}\n")),
        (None, "x".repeat(1025)),
        (Some("u".repeat(257)), "pw-1".to_owned()),
    ] {
        let message = internal(adapters_submit_credentials(id, user, password, None, true));
        assert!(message.contains("invalid credentials"), "{message}");
        assert!(!message.contains(SECRET), "{message}");
    }
    adapters_submit_credentials(id, None, "pw-1".to_owned(), None, true).unwrap();
    let server = env.next_server(&mut servers);
    env.wait_state(id, "streaming");
    assert_eq!(server.log().auth_ok, Some(true));
    wait_until("credentials saved", || saved(&target)["saved"] == true);
    assert_eq!(store.len(), 1);

    adapters_type_password(id).unwrap();
    rs_input_pointer(id, 0.0, 0.0);
    let mut expected = typed("pw-1");
    expected.push(ptr(0, 0, 0));
    env.expect_inputs(&server, &expected);
    env.close(id);

    // A new session uses the saved password without asking.
    let again = env.open(&target, &opts());
    let server = env.next_server(&mut servers);
    env.wait_state(again, "streaming");
    assert!(env.state(again, "needsCredentials").is_none());
    assert_eq!(server.log().auth_ok, Some(true));

    adapters_forget(target.clone()).unwrap();
    assert_eq!(saved(&target), json!({"saved": false}));
    assert!(store.is_empty());
    assert!(internal(adapters_type_password(again)).contains("no saved password"));
    env.close(again);

    for (_, event) in env.events.events() {
        assert!(!event.to_string().contains("pw-1"), "{event}");
    }
}

#[test]
fn bad_json_does_not_echo_values() {
    let env = Env::new();
    let bad_port =
        json!({"protocol": "vnc", "addr": "127.0.0.1", "port": SECRET, "password": SECRET})
            .to_string();
    // serde_json's own message quotes the value; the export must not.
    let raw = serde_json::from_str::<AdapterTarget>(&bad_port)
        .unwrap_err()
        .to_string();
    assert!(raw.contains(SECRET), "{raw}");

    let cases = [
        (bad_port.clone(), opts()),
        (
            json!({"protocol": SECRET, "addr": "127.0.0.1", "password": SECRET}).to_string(),
            opts(),
        ),
        (
            vnc_target(5900),
            json!({"viewWidth": SECRET, "password": SECRET}).to_string(),
        ),
        (format!("{{\"password\":\"{SECRET}\""), opts()),
    ];
    for (target, opts) in cases {
        let error = env.core.block_on(adapters_open(target, opts)).unwrap_err();
        assert!(matches!(error, CoreError::Internal { .. }), "{error:?}");
        assert!(!error.to_string().contains(SECRET), "{error}");
        assert!(!format!("{error:?}").contains(SECRET), "{error:?}");
    }
    for error in [
        adapters_saved(bad_port.clone()).unwrap_err(),
        adapters_forget(bad_port).unwrap_err(),
    ] {
        assert!(!error.to_string().contains(SECRET), "{error}");
    }

    let empty_addr = json!({"protocol": "vnc", "addr": ""}).to_string();
    assert_eq!(
        env.core.block_on(adapters_open(empty_addr, opts())),
        Err(CoreError::internal("invalid adapter target"))
    );
    assert!(list().is_empty());
}

#[test]
fn exports_without_core() {
    let _serial = serial();
    take_core();
    let id = ADAPTER_ID_BASE + 1;
    let not_initialized = Err(CoreError::internal("core not initialized"));
    let target = vnc_target(5900);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    assert_eq!(
        runtime.block_on(adapters_open(target.clone(), opts())),
        Err(CoreError::internal("core not initialized"))
    );
    assert_eq!(adapters_close(id), not_initialized);
    assert!(adapters_list().is_err());
    assert_eq!(
        adapters_submit_credentials(id, None, "pw".to_owned(), None, false),
        not_initialized
    );
    assert_eq!(adapters_cancel_credentials(id), not_initialized);
    assert!(adapters_saved(target.clone()).is_err());
    assert_eq!(adapters_forget(target), not_initialized);
    assert_eq!(adapters_type_password(id), not_initialized);
    assert_eq!(adapters_send_ctrl_alt_del(id), not_initialized);
    assert_eq!(adapters_trust_cert(id, true), not_initialized);
    assert_eq!(adapters_resize(id, 390, 844, 3.0), not_initialized);
    assert_eq!(sessions_key(id, 4, true, 0), not_initialized);
}

#[test]
fn rdp_open_resize_close() {
    let env = Env::new();
    let mut server = env
        .core
        .block_on(FakeRdpServer::start(
            FakeSecurity::Hybrid {
                user: "alice".into(),
                password: "pw-1".into(),
                domain: None,
            },
            1024,
            768,
        ))
        .unwrap();
    let target =
        json!({"protocol": "rdp", "addr": "127.0.0.1", "port": server.addr().port()}).to_string();
    let opts = json!({"viewWidth": 1024, "viewHeight": 768, "scale": 1}).to_string();
    let id = env.open(&target, &opts);
    let asked = env.wait_state(id, "needsCredentials");
    assert_eq!(asked["credFields"], json!(["user", "password", "domain"]));
    adapters_submit_credentials(id, Some("alice".to_owned()), "pw-1".to_owned(), None, false)
        .unwrap();
    let streaming = env.wait_state(id, "streaming");
    assert_eq!(streaming["protocol"], "rdp");
    assert_eq!(streaming["width"], 1024);
    assert_eq!(streaming["height"], 768);
    assert_eq!(list()[0]["target"]["protocol"], "rdp");

    // The fake server has no displaycontrol: resizing is a no-op and the
    // session keeps taking input.
    assert_eq!(adapters_resize(id, 800, 600, 2.0), Ok(()));
    adapters_send_ctrl_alt_del(id).unwrap();
    for (scancode, extended, down) in [
        (0x1D, false, true),
        (0x38, false, true),
        (0x4F, true, true),
        (0x4F, true, false),
        (0x38, false, false),
        (0x1D, false, false),
    ] {
        assert_eq!(
            env.core.block_on(server.next_input(WAIT)),
            Some(FakeInput::Key {
                scancode,
                extended,
                down
            })
        );
    }
    assert!(env.state(id, "ended").is_none());

    env.close(id);
    assert!(list().is_empty());
    drop(server);
}
