use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use rs_vnc::{
    client::{self, AuthAnswer, ClientConfig, ClientEvent, ClientInput, CredentialPrompt},
    testkit::{pair, AuthCheck, SecurityPlan, ServerScript, Step, TestRect},
    types::{encoding, security, Rect, VncError},
    MemSurface, Screen, Surface,
};
use tokio::sync::mpsc;
use zeroize::Zeroizing;

const WAIT: Duration = Duration::from_secs(5);

#[derive(Clone)]
struct SharedSurface(Arc<Mutex<MemSurface>>);

impl SharedSurface {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(MemSurface::new())))
    }
    fn snapshot(&self) -> MemSurface {
        self.0.lock().expect("surface mutex").clone()
    }
}

impl Surface for SharedSurface {
    fn resize(&mut self, w: u16, h: u16) {
        self.0.lock().expect("surface mutex").resize(w, h);
    }
    fn size(&self) -> (u16, u16) {
        self.0.lock().expect("surface mutex").size()
    }
    fn put_bgra(&mut self, r: Rect, src: &[u8], stride: usize) {
        self.0
            .lock()
            .expect("surface mutex")
            .put_bgra(r, src, stride);
    }
    fn fill(&mut self, r: Rect, bgra: [u8; 4]) {
        self.0.lock().expect("surface mutex").fill(r, bgra);
    }
    fn copy_rect(&mut self, sx: u16, sy: u16, r: Rect) {
        self.0.lock().expect("surface mutex").copy_rect(sx, sy, r);
    }
    fn commit(&mut self, dirty: &[Rect]) {
        self.0.lock().expect("surface mutex").commit(dirty);
    }
    fn cursor(&mut self, w: u16, h: u16, hot_x: u16, hot_y: u16, bgra: &[u8]) {
        self.0
            .lock()
            .expect("surface mutex")
            .cursor(w, h, hot_x, hot_y, bgra);
    }
    fn pointer_pos(&mut self, x: u16, y: u16) {
        self.0.lock().expect("surface mutex").pointer_pos(x, y);
    }
    fn screens(&mut self, screens: &[Screen]) {
        self.0.lock().expect("surface mutex").screens(screens);
    }
}

struct Prompt {
    asked: Arc<Mutex<Vec<u8>>>,
    answer: Option<(String, String)>,
}

#[async_trait]
impl CredentialPrompt for Prompt {
    async fn ask(&mut self, sec: u8) -> Option<AuthAnswer> {
        self.asked.lock().expect("prompt mutex").push(sec);
        self.answer.as_ref().map(|(user, password)| AuthAnswer {
            user: user.clone(),
            password: Zeroizing::new(password.clone()),
        })
    }
}

fn prompt(answer: Option<(&str, &str)>) -> (Prompt, Arc<Mutex<Vec<u8>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let answer = answer.map(|(user, password)| (user.to_owned(), password.to_owned()));
    (
        Prompt {
            asked: Arc::clone(&asked),
            answer,
        },
        asked,
    )
}

async fn connect(
    script: ServerScript,
    answer: Option<(&str, &str)>,
) -> (
    client::Connected<tokio::io::DuplexStream>,
    rs_vnc::testkit::FakeHandle,
    Arc<Mutex<Vec<u8>>>,
) {
    let (stream, handle) = pair(script);
    let (mut prompt, asked) = prompt(answer);
    let conn = tokio::time::timeout(
        WAIT,
        client::handshake(stream, &ClientConfig::default(), &mut prompt),
    )
    .await
    .expect("handshake timeout")
    .expect("handshake");
    (conn, handle, asked)
}

#[tokio::test]
async fn handshake_none_and_auth() {
    let (conn, handle, asked) = connect(ServerScript::new(800, 600), None).await;
    assert_eq!(conn.version, client::Version::V3_8);
    assert_eq!(conn.security, security::NONE);
    assert_eq!((conn.init.width, conn.init.height), (800, 600));
    assert!(asked.lock().expect("prompt mutex").is_empty());
    assert_eq!(handle.log().shared, Some(true));
    drop(conn);
    let _ = handle.finished().await;

    let mut script = ServerScript::new(2, 2);
    script.security = SecurityPlan::Offer {
        types: vec![security::VNC_AUTH],
        check: AuthCheck::VncPassword("secret".to_owned()),
    };
    let (conn, handle, asked) = connect(script, Some(("", "secret"))).await;
    assert_eq!(
        *asked.lock().expect("prompt mutex"),
        vec![security::VNC_AUTH]
    );
    drop(conn);
    let log = handle.finished().await;
    assert_eq!(log.auth_ok, Some(true));
}

#[tokio::test]
async fn handshake_ard_fail_cancel_unsupported_and_v33() {
    let mut script = ServerScript::new(2, 2);
    script.version = *b"RFB 003.889\n";
    script.security = SecurityPlan::Offer {
        types: vec![security::ARD, security::VNC_AUTH],
        check: AuthCheck::Ard {
            user: "alice".to_owned(),
            password: "s3cret!".to_owned(),
            params: rs_vnc::testkit::ArdTestParams::small(),
        },
    };
    let (conn, handle, asked) = connect(script, Some(("alice", "s3cret!"))).await;
    assert_eq!(conn.security, security::ARD);
    assert_eq!(*asked.lock().expect("prompt mutex"), vec![security::ARD]);
    assert_eq!(
        handle.log().ard_credentials,
        Some(("alice".to_owned(), "s3cret!".to_owned()))
    );
    drop(conn);
    let _ = handle.finished().await;

    let mut script = ServerScript::new(2, 2);
    script.security = SecurityPlan::Offer {
        types: vec![security::VNC_AUTH],
        check: AuthCheck::VncPassword("secret".to_owned()),
    };
    let (stream, handle) = pair(script.clone());
    let (mut prompt_impl, asked) = prompt(Some(("", "wrong")));
    let result = tokio::time::timeout(
        WAIT,
        client::handshake(stream, &ClientConfig::default(), &mut prompt_impl),
    )
    .await
    .expect("handshake timeout");
    assert!(
        matches!(result, Err(VncError::AuthFailed(reason)) if reason == "Authentication failed")
    );
    assert_eq!(
        *asked.lock().expect("prompt mutex"),
        vec![security::VNC_AUTH]
    );
    let _ = handle.finished().await;

    let (stream, handle) = pair(script);
    let (mut prompt_impl, _) = prompt(None);
    let result = tokio::time::timeout(
        WAIT,
        client::handshake(stream, &ClientConfig::default(), &mut prompt_impl),
    )
    .await
    .expect("handshake timeout");
    assert!(matches!(result, Err(VncError::Cancelled)));
    let _ = handle.finished().await;

    let mut script = ServerScript::new(2, 2);
    script.security = SecurityPlan::Offer {
        types: vec![16, 35],
        check: AuthCheck::Any,
    };
    let (stream, handle) = pair(script);
    let (mut prompt_impl, _) = prompt(None);
    let result = tokio::time::timeout(
        WAIT,
        client::handshake(stream, &ClientConfig::default(), &mut prompt_impl),
    )
    .await
    .expect("handshake timeout");
    assert!(
        matches!(result, Err(VncError::UnsupportedSecurity(offered)) if offered == vec![16, 35])
    );
    let _ = handle.finished().await;

    let mut script = ServerScript::new(2, 2);
    script.version = *b"RFB 003.003\n";
    script.security = SecurityPlan::Offer {
        types: vec![security::VNC_AUTH],
        check: AuthCheck::VncPassword("pw".to_owned()),
    };
    let (conn, handle, asked) = connect(script, Some(("", "pw"))).await;
    assert_eq!(conn.version, client::Version::V3_3);
    assert_eq!(
        *asked.lock().expect("prompt mutex"),
        vec![security::VNC_AUTH]
    );
    drop(conn);
    let _ = handle.finished().await;
}

#[tokio::test]
async fn run_raw_events_and_inputs() {
    let script = ServerScript {
        steps: vec![
            Step::Update(vec![TestRect::Solid {
                rect: Rect::new(0, 0, 2, 2),
                bgra: [1, 2, 3, 255],
            }]),
            Step::Bell,
            Step::CutText(b"caf\xe9".to_vec()),
        ],
        ..ServerScript::new(800, 600)
    };
    let (conn, handle, _) = connect(script, None).await;
    let surface = SharedSurface::new();
    let view = surface.clone();
    let (input_tx, input_rx) = mpsc::channel(8);
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        let mut surface = surface;
        client::run(conn, &mut surface, input_rx, event_tx).await
    });
    handle.wait_for(|log| log.msgs.len() >= 3, WAIT).await;
    input_tx
        .send(ClientInput::Key {
            down: true,
            keysym: 0x61,
        })
        .await
        .expect("input");
    input_tx
        .send(ClientInput::Pointer {
            mask: 1,
            x: 10,
            y: 20,
        })
        .await
        .expect("input");
    input_tx
        .send(ClientInput::CutText(b"x".to_vec()))
        .await
        .expect("input");
    assert_eq!(
        tokio::time::timeout(WAIT, event_rx.recv())
            .await
            .expect("event timeout"),
        Some(ClientEvent::Bell)
    );
    assert_eq!(
        tokio::time::timeout(WAIT, event_rx.recv())
            .await
            .expect("event timeout"),
        Some(ClientEvent::CutText("café".to_owned()))
    );
    assert_eq!(view.snapshot().pixel(1, 1), [1, 2, 3, 255]);
    assert!(
        handle
            .wait_for(
                |log| log.msgs.iter().any(|msg| matches!(
                    msg,
                    rs_vnc::proto::ClientMsg::KeyEvent { keysym: 0x61, .. }
                )),
                WAIT
            )
            .await
    );
    drop(input_tx);
    assert!(tokio::time::timeout(WAIT, task)
        .await
        .expect("run timeout")
        .expect("join")
        .is_ok());
    let _ = handle.finished().await;
}

#[tokio::test]
async fn run_resize_and_protocol_errors() {
    let script = ServerScript {
        steps: vec![Step::Update(vec![TestRect::DesktopSize {
            w: 1024,
            h: 768,
        }])],
        ..ServerScript::new(800, 600)
    };
    let (conn, handle, _) = connect(script, None).await;
    let surface = SharedSurface::new();
    let view = surface.clone();
    let (input_tx, input_rx) = mpsc::channel(1);
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        let mut surface = surface;
        client::run(conn, &mut surface, input_rx, event_tx).await
    });
    assert_eq!(
        tokio::time::timeout(WAIT, event_rx.recv())
            .await
            .expect("event timeout"),
        Some(ClientEvent::Resized { w: 1024, h: 768 })
    );
    assert_eq!(view.snapshot().size(), (1024, 768));
    handle.push(Step::Close);
    drop(input_tx);
    let _ = tokio::time::timeout(WAIT, task).await.expect("run timeout");
    let _ = handle.finished().await;

    let mut script = ServerScript::new(800, 600);
    script.steps = vec![Step::Update(vec![TestRect::Bytes(
        rs_vnc::proto::encode_rect_header(&rs_vnc::proto::RectHeader {
            rect: Rect::new(0, 0, 801, 1),
            encoding: encoding::RAW,
        })
        .to_vec(),
    )])];
    let (conn, handle, _) = connect(script, None).await;
    let mut surface = MemSurface::new();
    let (_input_tx, input_rx) = mpsc::channel(1);
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let result = tokio::time::timeout(WAIT, client::run(conn, &mut surface, input_rx, event_tx))
        .await
        .expect("run timeout");
    assert!(matches!(result, Err(VncError::Protocol(_))));
    let _ = handle.finished().await;
}

#[tokio::test]
async fn server_close_returns_closed_and_input_close_returns_ok() {
    let script = ServerScript {
        steps: vec![Step::Close],
        ..ServerScript::new(2, 2)
    };
    let (conn, handle, _) = connect(script, None).await;
    let mut surface = MemSurface::new();
    let (_input_tx, input_rx) = mpsc::channel(1);
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let result = tokio::time::timeout(WAIT, client::run(conn, &mut surface, input_rx, event_tx))
        .await
        .expect("run timeout");
    assert!(matches!(result, Err(VncError::Closed(_))));
    let _ = handle.finished().await;

    let (conn, handle, _) = connect(ServerScript::new(2, 2), None).await;
    let mut surface = MemSurface::new();
    let (input_tx, input_rx) = mpsc::channel(1);
    drop(input_tx);
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let result = tokio::time::timeout(WAIT, client::run(conn, &mut surface, input_rx, event_tx))
        .await
        .expect("run timeout");
    assert!(result.is_ok());
    let _ = handle.finished().await;
}

#[tokio::test]
async fn handshake_keeps_server_version() {
    let (conn, handle, _) = connect(ServerScript::new(2, 2), None).await;
    assert_eq!(conn.server_version, "003.008");
    drop(conn);
    let _ = handle.finished().await;

    let mut script = ServerScript::new(2, 2);
    script.version = *b"RFB 003.889\n";
    let (conn, handle, _) = connect(script, None).await;
    assert_eq!(conn.version, client::Version::V3_8);
    assert_eq!(conn.server_version, "003.889");
    drop(conn);
    let _ = handle.finished().await;
}

#[tokio::test]
async fn run_reports_extended_desktop_size_screens() {
    let screens = vec![
        Screen {
            id: 1,
            rect: Rect::new(0, 0, 2560, 1440),
            flags: 0,
        },
        Screen {
            id: 2,
            rect: Rect::new(2560, 0, 1920, 1200),
            flags: 0,
        },
    ];
    let script = ServerScript {
        steps: vec![Step::Update(vec![TestRect::ExtendedDesktopSize {
            reason: 0,
            status: 0,
            w: 4480,
            h: 1440,
            screens: screens.clone(),
        }])],
        ..ServerScript::new(800, 600)
    };
    let (conn, handle, _) = connect(script, None).await;
    let surface = SharedSurface::new();
    let view = surface.clone();
    let (input_tx, input_rx) = mpsc::channel(1);
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        let mut surface = surface;
        client::run(conn, &mut surface, input_rx, event_tx).await
    });
    assert_eq!(
        tokio::time::timeout(WAIT, event_rx.recv())
            .await
            .expect("event timeout"),
        Some(ClientEvent::Resized { w: 4480, h: 1440 })
    );
    let snapshot = view.snapshot();
    assert_eq!(snapshot.size(), (4480, 1440));
    assert_eq!(snapshot.screens, Some(screens));

    let encodings = handle
        .log()
        .msgs
        .iter()
        .find_map(|msg| match msg {
            rs_vnc::proto::ClientMsg::SetEncodings(list) => Some(list.clone()),
            _ => None,
        })
        .expect("SetEncodings sent");
    assert!(encodings.contains(&encoding::EXTENDED_DESKTOP_SIZE));
    assert!(encodings.contains(&-308));

    handle.push(Step::Close);
    drop(input_tx);
    let _ = tokio::time::timeout(WAIT, task).await.expect("run timeout");
    let _ = handle.finished().await;
}
