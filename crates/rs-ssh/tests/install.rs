//! Local script tests and in-process russh peers on loopback ephemeral ports.

use std::{
    collections::HashMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Output, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use rs_ssh::{
    install::{
        build_script, install_keys, parse_report, InstallError, InstallReport, InstallRequest,
        LineStatus, SkipReason,
    },
    Credential, MemoryHostKeyStore, Prompter, SshOptions, SshSession, SshTarget,
};
use russh::{
    server::{self, Auth},
    Channel, ChannelId, MethodKind, MethodSet, SshId,
};
use ssh_key::{private::Ed25519Keypair, PrivateKey, PublicKey};
use tokio::{
    io::{AsyncWriteExt, ReadHalf, WriteHalf},
    net::TcpListener,
    process::Command,
};
use zeroize::Zeroizing;

const DEVICE_ID: &str = "k3jq7x2m4p6tbw2a";

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "rs-ssh-install-{}-{label}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        make_writable(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn make_writable(path: &Path) {
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    if metadata.is_dir() {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                make_writable(&entry.path());
            }
        }
    } else {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
}

fn private(seed: u8) -> PrivateKey {
    PrivateKey::new(Ed25519Keypair::from_seed(&[seed; 32]).into(), "").unwrap()
}

fn device_key_line(device_id: &str) -> String {
    let secret = p256::SecretKey::from_slice(&[7u8; 32]).unwrap();
    let point = secret.public_key().to_sec1_bytes();
    let key = ssh_key::public::EcdsaPublicKey::from_sec1_bytes(&point).unwrap();
    let mut key = PublicKey::from(key);
    key.set_comment(format!("rs-device={device_id}"));
    key.to_openssh().unwrap()
}

fn request() -> InstallRequest {
    InstallRequest {
        device_id: DEVICE_ID.to_owned(),
        device_key_line: device_key_line(DEVICE_ID),
    }
}

async fn run_local_script(home: &Path, script: &str) -> Output {
    let mut child = Command::new("/bin/sh")
        .arg("-s")
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(script.as_bytes()).await.unwrap();
    drop(stdin);
    child.wait_with_output().await.unwrap()
}

fn report(output: &Output) -> InstallReport {
    assert!(
        output.status.success(),
        "script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    parse_report(&String::from_utf8_lossy(&output.stdout)).unwrap()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn build_script_rejects_invalid_user_data() {
    let mut invalid = Vec::new();
    for device_id in [
        "x; rm -rf ~".to_owned(),
        "../aaaaaaaaaaaaa".to_owned(),
        "aaaaaaaaaaaaaaaaa".to_owned(),
        "AAAAAAAAAAAAAAAA".to_owned(),
    ] {
        let mut req = request();
        req.device_id = device_id;
        invalid.push(req);
    }

    let mut wrong_id = request();
    wrong_id.device_key_line = device_key_line("aaaaaaaaaaaaaaaa");
    invalid.push(wrong_id);

    let mut newline = request();
    newline.device_key_line.push('\n');
    invalid.push(newline);

    let mut command_substitution = request();
    command_substitution.device_key_line = command_substitution
        .device_key_line
        .replace(&format!("rs-device={DEVICE_ID}"), "rs-device=$(id)");
    invalid.push(command_substitution);

    let mut rsa = request();
    rsa.device_key_line = format!("ssh-rsa AAAA rs-device={DEVICE_ID}");
    invalid.push(rsa);

    for req in invalid {
        assert!(
            matches!(build_script(&req), Err(InstallError::InvalidInput(_))),
            "request was accepted unexpectedly"
        );
    }

    let script = build_script(&request()).unwrap();
    assert!(!script.contains("keyclean"));
    let mut ed25519_device = request();
    ed25519_device.device_key_line = {
        let mut key = private(9).public_key().clone();
        key.set_comment(format!("rs-device={DEVICE_ID}"));
        key.to_openssh().unwrap()
    };
    assert!(build_script(&ed25519_device).is_ok());
}

#[tokio::test]
async fn local_fresh_home_and_idempotence() {
    let home = TempDir::new("fresh");
    let req = request();
    let script = build_script(&req).unwrap();

    let first = run_local_script(home.path(), &script).await;
    assert_eq!(
        report(&first),
        InstallReport {
            device: LineStatus::Added,
        }
    );
    assert_eq!(
        String::from_utf8_lossy(&first.stdout),
        "device=added\ndone\n"
    );

    let ssh_dir = home.path().join(".ssh");
    let authorized = ssh_dir.join("authorized_keys");
    assert_eq!(mode(&ssh_dir), 0o700);
    assert_eq!(mode(&authorized), 0o600);
    assert_eq!(
        fs::read_to_string(&authorized).unwrap(),
        format!("{}\n", req.device_key_line)
    );
    let before = fs::read(&authorized).unwrap();

    let second = run_local_script(home.path(), &script).await;
    assert_eq!(
        report(&second),
        InstallReport {
            device: LineStatus::AlreadyPresent,
        }
    );
    assert_eq!(fs::read(&authorized).unwrap(), before);
}

#[tokio::test]
async fn local_preserves_existing_content_and_adds_missing_newline() {
    let home = TempDir::new("existing");
    let ssh_dir = home.path().join(".ssh");
    fs::create_dir(&ssh_dir).unwrap();
    let authorized = ssh_dir.join("authorized_keys");
    let existing = b"ssh-ed25519 AAAA old-one\nssh-ed25519 AAAA old-two";
    fs::write(&authorized, existing).unwrap();
    let req = request();

    let output = run_local_script(home.path(), &build_script(&req).unwrap()).await;
    assert_eq!(report(&output).device, LineStatus::Added);
    let expected = [
        existing.as_slice(),
        b"\n",
        req.device_key_line.as_bytes(),
        b"\n",
    ]
    .concat();
    assert_eq!(fs::read(&authorized).unwrap(), expected);
}

#[test]
fn parse_report_rejects_incomplete_or_unknown_output() {
    for output in [
        "device=added\n",
        "done\n",
        "device=wat\ndone\n",
        "device=added\ndone\nextra=1\n",
        "device=added\n\ndone\n",
        "device=added\nmaint=added\ndone\n",
        "device=added\nkeyclean=not-requested\ndone\n",
        "device=added\nkeyclean_path=/usr/local/bin/rs-keyclean\ndone\n",
        "device=added\ndevice=present\ndone\n",
        "device=added\ndone\ndone\n",
    ] {
        assert!(
            matches!(parse_report(output), Err(InstallError::BadOutput)),
            "accepted output: {output:?}"
        );
    }
}

#[test]
fn parse_report_accepts_the_device_line() {
    for (output, device) in [
        ("device=added\ndone\n", LineStatus::Added),
        ("device=present\ndone\n", LineStatus::AlreadyPresent),
    ] {
        assert_eq!(parse_report(output).unwrap(), InstallReport { device });
    }
}

struct AcceptPrompter;

#[async_trait]
impl Prompter for AcceptPrompter {
    async fn confirm_new_host_key(&self, _: &str, _: &str, _: &str) -> bool {
        true
    }

    async fn keyboard_interactive(
        &self,
        _: &str,
        _: &str,
        _: &[(String, bool)],
    ) -> Option<Vec<String>> {
        None
    }

    fn auth_url(&self, _: &str) {}
}

#[derive(Clone)]
enum ServerId {
    Tailscale,
    Default,
}

#[derive(Clone)]
enum ExecBehavior {
    RunSh(PathBuf),
    Fail,
}

struct Observed {
    commands: Vec<String>,
}

#[derive(Clone)]
struct ServerScenario {
    server_id: ServerId,
    behavior: ExecBehavior,
    observed: Arc<Mutex<Observed>>,
}

struct TestServer {
    address: std::net::SocketAddr,
    observed: Arc<Mutex<Observed>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl TestServer {
    async fn start(server_id: ServerId, behavior: ExecBehavior) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let host_key = Arc::new(Mutex::new(private(1)));
        let observed = Arc::new(Mutex::new(Observed {
            commands: Vec::new(),
        }));
        let scenario = ServerScenario {
            server_id,
            behavior,
            observed: observed.clone(),
        };
        let task = tokio::spawn(async move {
            let mut peers = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        let methods = [MethodKind::Password];
                        let config = Arc::new(server::Config {
                            server_id: match scenario.server_id {
                                ServerId::Tailscale => {
                                    SshId::Standard("SSH-2.0-Tailscale".into())
                                }
                                ServerId::Default => server::Config::default().server_id,
                            },
                            keys: vec![host_key.lock().unwrap().clone()],
                            methods: MethodSet::from(methods.as_slice()),
                            auth_rejection_time: Duration::from_millis(1),
                            auth_rejection_time_initial: Some(Duration::from_millis(1)),
                            max_auth_attempts: 5,
                            channel_buffer_size: 8,
                            event_buffer_size: 16,
                            nodelay: true,
                            ..Default::default()
                        });
                        let handler = TestHandler {
                            scenario: scenario.clone(),
                            channels: HashMap::new(),
                        };
                        peers.spawn(async move {
                            if let Ok(peer) = server::run_stream(config, stream, handler).await {
                                let _ = peer.await;
                            }
                        });
                    }
                    _ = peers.join_next(), if !peers.is_empty() => {}
                }
            }
        });
        Self {
            address,
            observed,
            task,
        }
    }

    async fn connect(&self) -> SshSession {
        let options = SshOptions {
            connect_timeout: Duration::from_secs(5),
            auth_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(5),
            ..Default::default()
        };
        SshSession::connect(
            SshTarget {
                host: "127.0.0.1".into(),
                port: self.address.port(),
                user: "test".into(),
                jump: None,
            },
            vec![Credential::Password(Zeroizing::new("correct".into()))],
            Arc::new(MemoryHostKeyStore::default()),
            Arc::new(AcceptPrompter),
            options,
        )
        .await
        .unwrap()
    }

    fn commands(&self) -> Vec<String> {
        self.observed.lock().unwrap().commands.clone()
    }
}

struct TestHandler {
    scenario: ServerScenario,
    channels: HashMap<ChannelId, Channel<server::Msg>>,
}

impl server::Handler for TestHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, _: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if password == "correct" {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        command: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.scenario
            .observed
            .lock()
            .unwrap()
            .commands
            .push(String::from_utf8_lossy(command).into_owned());
        session.channel_success(channel)?;
        let Some(channel_stream) = self.channels.remove(&channel) else {
            return session.close(channel);
        };
        let handle = session.handle();
        let behavior = self.scenario.behavior.clone();
        tokio::spawn(async move {
            match behavior {
                ExecBehavior::Fail => {
                    let _ = handle.extended_data(channel, 1, vec![b'e'; 600]).await;
                    let _ = handle.exit_status_request(channel, 3).await;
                    let _ = handle.eof(channel).await;
                    let _ = handle.close(channel).await;
                }
                ExecBehavior::RunSh(home) => {
                    let mut child = Command::new("/bin/sh")
                        .arg("-s")
                        .env("HOME", home)
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()
                        .expect("spawn /bin/sh");
                    let mut stdin = child.stdin.take().expect("child stdin");
                    let stream = channel_stream.into_stream();
                    let (mut reader, _writer): (ReadHalf<_>, WriteHalf<_>) =
                        tokio::io::split(stream);
                    let feed = tokio::spawn(async move {
                        let _ = tokio::io::copy(&mut reader, &mut stdin).await;
                        let _ = stdin.shutdown().await;
                    });
                    let output = child.wait_with_output().await.expect("wait for /bin/sh");
                    let _ = feed.await;
                    if !output.stdout.is_empty() {
                        let _ = handle.data(channel, output.stdout).await;
                    }
                    if !output.stderr.is_empty() {
                        let _ = handle.extended_data(channel, 1, output.stderr).await;
                    }
                    let status = output.status.code().unwrap_or(255) as u32;
                    let _ = handle.exit_status_request(channel, status).await;
                    let _ = handle.eof(channel).await;
                    let _ = handle.close(channel).await;
                }
            }
        });
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.channels.remove(&channel);
        Ok(())
    }
}

#[tokio::test]
async fn ssh_install_sends_only_fixed_command() {
    let home = TempDir::new("ssh-home");
    let server = TestServer::start(
        ServerId::Default,
        ExecBehavior::RunSh(home.path().to_owned()),
    )
    .await;
    let session = server.connect().await;
    assert!(!session.server_info().tailscale);
    assert!(session.server_info().ssh_id.starts_with("SSH-2.0-"));

    let req = request();
    let report = install_keys(&session, &req).await.unwrap();
    assert_eq!(report.device, LineStatus::Added);
    assert_eq!(server.commands(), vec!["/bin/sh -s".to_owned()]);
    let authorized = home.path().join(".ssh/authorized_keys");
    assert_eq!(mode(&authorized), 0o600);
    assert_eq!(
        fs::read_to_string(&authorized).unwrap(),
        format!("{}\n", req.device_key_line)
    );
    session.close().await;
}

#[tokio::test]
async fn tailscale_ssh_is_detected_and_skipped() {
    let home = TempDir::new("tailscale-home");
    let server = TestServer::start(
        ServerId::Tailscale,
        ExecBehavior::RunSh(home.path().to_owned()),
    )
    .await;
    let session = server.connect().await;
    let info = session.server_info();
    assert_eq!(info.ssh_id, "SSH-2.0-Tailscale");
    assert!(info.tailscale);

    assert_eq!(
        install_keys(&session, &request()).await.unwrap(),
        InstallReport {
            device: LineStatus::Skipped(SkipReason::TailscaleSsh),
        }
    );
    assert!(server.commands().is_empty());
    assert!(!home.path().join(".ssh").exists());
    session.close().await;
}

#[tokio::test]
async fn script_failure_reports_status_and_truncated_stderr() {
    let server = TestServer::start(ServerId::Default, ExecBehavior::Fail).await;
    let session = server.connect().await;
    let error = install_keys(&session, &request()).await.unwrap_err();
    match error {
        InstallError::ScriptFailed { status, stderr } => {
            assert_eq!(status, Some(3));
            assert!(stderr.len() <= 512);
            assert_eq!(stderr.len(), 512);
        }
        other => panic!("unexpected error: {other:?}"),
    }
    session.close().await;
}
