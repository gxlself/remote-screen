#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use host_core::ipc::Permission;
use host_linux::ctl::{ApprovalBook, ApprovalPrompt};
use host_linux::notify::{Notification, Notifier, NotifyEvent};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use zbus::connection::Builder;
use zbus::{Connection, OwnedVariant};

const SERVICE: &str = "org.freedesktop.Notifications";
const OBJECT: &str = "/org/freedesktop/Notifications";

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedNotification {
    id: u32,
    summary: String,
    body: String,
    actions: Vec<String>,
    urgency: u8,
    timeout_ms: i32,
}

#[derive(Default)]
struct MockState {
    calls: Vec<RecordedNotification>,
    closed: Vec<u32>,
    next_id: u32,
}

struct MockNotifications {
    state: Arc<Mutex<MockState>>,
    capabilities: Vec<String>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl MockNotifications {
    async fn get_capabilities(&self) -> Vec<String> {
        self.capabilities.clone()
    }

    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        _app_name: &str,
        _replaces_id: u32,
        _app_icon: &str,
        summary: &str,
        body: &str,
        actions: Vec<String>,
        hints: HashMap<String, OwnedVariant>,
        timeout_ms: i32,
    ) -> u32 {
        let urgency = hints
            .get("urgency")
            .and_then(|value| u8::try_from(value).ok())
            .unwrap_or(0);
        let mut state = self.state.lock().expect("notification state lock");
        state.next_id += 1;
        let id = state.next_id;
        state.calls.push(RecordedNotification {
            id,
            summary: summary.to_owned(),
            body: body.to_owned(),
            actions,
            urgency,
            timeout_ms,
        });
        id
    }

    async fn close_notification(&self, id: u32) {
        self.state
            .lock()
            .expect("notification state lock")
            .closed
            .push(id);
    }
}

struct PrivateBus {
    address: String,
    daemon: Child,
}

impl PrivateBus {
    fn start() -> Self {
        let mut daemon = Command::new("dbus-daemon")
            .args([
                "--session",
                "--nofork",
                "--print-address=1",
                "--print-pid=1",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start private dbus-daemon");
        let stdout = daemon.stdout.as_mut().expect("dbus stdout");
        let mut lines = std::io::BufReader::new(stdout).lines();
        let address = lines
            .next()
            .expect("dbus address line")
            .expect("read dbus address");
        Self { address, daemon }
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

async fn service(address: &str, capabilities: Vec<&str>) -> (Connection, Arc<Mutex<MockState>>) {
    let state = Arc::new(Mutex::new(MockState::default()));
    let service = MockNotifications {
        state: state.clone(),
        capabilities: capabilities.into_iter().map(str::to_owned).collect(),
    };
    let connection = Builder::address(address)
        .name(SERVICE)
        .serve_at(OBJECT, service)
        .build()
        .await
        .expect("build notification service");
    (connection, state)
}

enum AgentCommand {
    Event { method: String, params: Value },
    Close,
}

struct FakeAgent {
    path: PathBuf,
    commands: UnboundedSender<AgentCommand>,
    requests: AsyncMutex<UnboundedReceiver<Value>>,
    task: JoinHandle<()>,
}

impl FakeAgent {
    async fn start() -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "remote-host-ctl-notify-{}-{}.sock",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind fake agent socket");
        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let (request_tx, request_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let (reader, mut writer) = stream.into_split();
                let mut reader = BufReader::new(reader);
                let mut line = String::new();
                loop {
                    tokio::select! {
                        read = reader.read_line(&mut line) => {
                            let Ok(read) = read else { break };
                            if read == 0 { break; }
                            let request: Value = match serde_json::from_str(line.trim()) {
                                Ok(value) => value,
                                Err(_) => {
                                    line.clear();
                                    continue;
                                }
                            };
                            let id = request.get("id").cloned().unwrap_or(Value::Null);
                            let method = request.get("method").and_then(Value::as_str).unwrap_or("");
                            let _ = request_tx.send(request.clone());
                            let response = if method == "approval.pending" {
                                json!({"jsonrpc":"2.0","id":id,"result":{"requests":[]}})
                            } else {
                                json!({"jsonrpc":"2.0","id":id,"result":{}})
                            };
                            let mut encoded = response.to_string();
                            encoded.push('\n');
                            if writer.write_all(encoded.as_bytes()).await.is_err() {
                                break;
                            }
                            line.clear();
                        }
                        command = command_rx.recv() => {
                            match command {
                                Some(AgentCommand::Event { method, params }) => {
                                    let event = json!({"jsonrpc":"2.0","method":method,"params":params});
                                    let mut encoded = event.to_string();
                                    encoded.push('\n');
                                    if writer.write_all(encoded.as_bytes()).await.is_err() {
                                        break;
                                    }
                                }
                                Some(AgentCommand::Close) | None => break,
                            }
                        }
                    }
                }
            }
        });
        Self {
            path,
            commands,
            requests: AsyncMutex::new(request_rx),
            task,
        }
    }

    async fn event(&self, method: &str, params: Value) {
        self.commands
            .send(AgentCommand::Event {
                method: method.to_owned(),
                params,
            })
            .expect("fake agent event receiver");
    }

    async fn close(&self) {
        self.commands
            .send(AgentCommand::Close)
            .expect("fake agent close receiver");
    }

    async fn next_request(&self) -> Value {
        timeout(Duration::from_secs(10), self.requests.lock().await.recv())
            .await
            .expect("fake agent request timeout")
            .expect("fake agent request stream closed")
    }
}

impl Drop for FakeAgent {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}

fn approval_event(request_id: &str, permission_choice: bool) -> Value {
    json!({
        "request_id": request_id,
        "kind": "connection",
        "device_name": "test device",
        "platform": "Linux",
        "whois_login": "alice",
        "intent": "control",
        "timeout_s": 30,
        "permission_choice": permission_choice,
    })
}

async fn wait_for_call(state: &Arc<Mutex<MockState>>, index: usize) -> RecordedNotification {
    timeout(Duration::from_secs(10), async {
        loop {
            if let Some(call) = state
                .lock()
                .expect("notification state lock")
                .calls
                .get(index)
                .cloned()
            {
                return call;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("notification call timeout")
}

fn approval(permission_choice: bool) -> ApprovalPrompt {
    ApprovalPrompt {
        request_id: "request-1".into(),
        kind: "connection".into(),
        device_name: "test device".into(),
        platform: Some("Linux".into()),
        intent: Some("control".into()),
        whois_login: Some("alice".into()),
        timeout_s: 30,
        permission_choice,
    }
}

#[test]
fn notification_values_are_owned_and_comparable() {
    let notification = Notification {
        summary: "Remote Host".into(),
        body: "approval".into(),
        actions: vec![
            ("allow".into(), "允许".into()),
            ("deny".into(), "拒绝".into()),
        ],
        critical: true,
        timeout_ms: 30_000,
    };
    assert_eq!(notification.actions.len(), 2);
    assert_eq!(notification.timeout_ms, 30_000);
    assert_eq!(
        NotifyEvent::Action {
            id: 7,
            key: "allow".into()
        },
        NotifyEvent::Action {
            id: 7,
            key: "allow".into()
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn notifier_sends_buttons_and_reads_signals() {
    let bus = PrivateBus::start();
    let (server, state) = service(&bus.address, vec!["actions"]).await;
    let client = Builder::address(bus.address.as_str())
        .build()
        .await
        .expect("connect notification client");
    let mut notifier = Notifier::new(client).await.expect("create notifier");
    let id = notifier
        .show(&Notification {
            summary: "test device 请求控制这台电脑".into(),
            body: "30 秒内不处理将自动拒绝".into(),
            actions: vec![
                ("allow".into(), "允许".into()),
                ("deny".into(), "拒绝".into()),
            ],
            critical: true,
            timeout_ms: 30_000,
        })
        .await
        .expect("show notification");
    assert_eq!(id, 1);
    let call = state.lock().expect("notification state lock").calls[0].clone();
    assert_eq!(call.actions, ["allow", "允许", "deny", "拒绝"]);
    assert_eq!(call.urgency, 2);
    assert_eq!(call.timeout_ms, 30_000);
    server
        .emit_signal(
            None::<&str>,
            OBJECT,
            SERVICE,
            "ActionInvoked",
            &(id, "allow"),
        )
        .await
        .expect("emit action signal");
    assert_eq!(
        timeout(Duration::from_secs(10), notifier.next_event())
            .await
            .expect("action signal timeout"),
        Some(NotifyEvent::Action {
            id,
            key: "allow".into()
        })
    );
    server
        .emit_signal(
            None::<&str>,
            OBJECT,
            SERVICE,
            "NotificationClosed",
            &(id, 2_u32),
        )
        .await
        .expect("emit closed signal");
    assert_eq!(
        timeout(Duration::from_secs(10), notifier.next_event())
            .await
            .expect("closed signal timeout"),
        Some(NotifyEvent::Closed { id, reason: 2 })
    );
    notifier.close(id).await.expect("close notification");
    assert_eq!(state.lock().expect("notification state lock").closed, [id]);
}

#[tokio::test(flavor = "multi_thread")]
async fn approval_book_notification_actions_are_safe() {
    let mut book = ApprovalBook::default();
    book.add(approval(true), 7);
    assert_eq!(
        book.on_action(7, "allow_control").unwrap().permission,
        Some(Permission::Control)
    );
    book.add(approval(false), 8);
    assert!(book.on_action(8, "unknown").is_none());
    assert!(book.on_closed(8, 1).is_none());
    assert!(book.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn run_notify_responds_to_action_and_dismissal() {
    let bus = PrivateBus::start();
    let (server, state) = service(&bus.address, vec!["actions"]).await;
    let agent = FakeAgent::start().await;
    let client = Builder::address(bus.address.as_str())
        .build()
        .await
        .expect("connect notification client");
    let notifier = Notifier::new(client).await.expect("create notifier");
    let task = tokio::spawn(host_linux::notify::run_notify(
        host_linux::notify::NotifyLoopOptions {
            socket: agent.path.clone(),
            reconnect: Duration::from_millis(20),
        },
        notifier,
    ));
    assert_eq!(agent.next_request().await["method"], "approval.pending");
    agent
        .event("event.approvalRequested", approval_event("r-action", false))
        .await;
    let first = wait_for_call(&state, 0).await;
    server
        .emit_signal(
            None::<&str>,
            OBJECT,
            SERVICE,
            "ActionInvoked",
            &(first.id, "allow"),
        )
        .await
        .expect("emit action signal");
    let response = agent.next_request().await;
    assert_eq!(response["params"]["request_id"], "r-action");
    assert_eq!(response["params"]["allow"], true);
    assert_eq!(response["params"]["permission"], Value::Null);

    agent
        .event(
            "event.approvalRequested",
            approval_event("r-dismiss", false),
        )
        .await;
    let second = wait_for_call(&state, 1).await;
    server
        .emit_signal(
            None::<&str>,
            OBJECT,
            SERVICE,
            "NotificationClosed",
            &(second.id, 2_u32),
        )
        .await
        .expect("emit dismissal signal");
    let response = agent.next_request().await;
    assert_eq!(response["params"]["request_id"], "r-dismiss");
    assert_eq!(response["params"]["allow"], false);
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn run_notify_reconnects_and_closes_old_notifications() {
    let bus = PrivateBus::start();
    let (_server, state) = service(&bus.address, vec!["actions"]).await;
    let agent = FakeAgent::start().await;
    let client = Builder::address(bus.address.as_str())
        .build()
        .await
        .expect("connect notification client");
    let notifier = Notifier::new(client).await.expect("create notifier");
    let task = tokio::spawn(host_linux::notify::run_notify(
        host_linux::notify::NotifyLoopOptions {
            socket: agent.path.clone(),
            reconnect: Duration::from_millis(20),
        },
        notifier,
    ));
    assert_eq!(agent.next_request().await["method"], "approval.pending");
    agent
        .event("event.approvalRequested", approval_event("r-old", false))
        .await;
    let old = wait_for_call(&state, 0).await;
    agent.close().await;
    timeout(Duration::from_secs(10), async {
        loop {
            if state
                .lock()
                .expect("notification state lock")
                .closed
                .contains(&old.id)
            {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("old notification close timeout");
    assert_eq!(agent.next_request().await["method"], "approval.pending");
    agent
        .event("event.approvalRequested", approval_event("r-new", false))
        .await;
    let new = wait_for_call(&state, 1).await;
    assert_ne!(old.id, new.id);
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn run_notify_falls_back_without_action_capability() {
    let bus = PrivateBus::start();
    let (server, state) = service(&bus.address, Vec::new()).await;
    let agent = FakeAgent::start().await;
    let client = Builder::address(bus.address.as_str())
        .build()
        .await
        .expect("connect notification client");
    let notifier = Notifier::new(client).await.expect("create notifier");
    let task = tokio::spawn(host_linux::notify::run_notify(
        host_linux::notify::NotifyLoopOptions {
            socket: agent.path.clone(),
            reconnect: Duration::from_millis(20),
        },
        notifier,
    ));
    assert_eq!(agent.next_request().await["method"], "approval.pending");
    agent
        .event(
            "event.approvalRequested",
            approval_event("r-fallback", false),
        )
        .await;
    let call = wait_for_call(&state, 0).await;
    assert!(call.actions.is_empty());
    assert!(call.body.contains("remote-host-ctl approve"));
    server
        .emit_signal(
            None::<&str>,
            OBJECT,
            SERVICE,
            "NotificationClosed",
            &(call.id, 2_u32),
        )
        .await
        .expect("emit fallback dismissal");
    let response = agent.next_request().await;
    assert_eq!(response["params"]["request_id"], "r-fallback");
    assert_eq!(response["params"]["allow"], false);
    task.abort();
}
