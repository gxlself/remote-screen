#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{Shutdown, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::{
    server::{self, Auth},
    Channel, ChannelId, ChannelMsg, MethodKind, MethodSet, Pty,
};
use ssh_key::{private::Ed25519Keypair, PrivateKey, PublicKey};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct ServerCfg {
    pub host_key: PrivateKey,
    pub password: Option<String>,
    pub keyboard_interactive: bool,
    pub accept_keys: Vec<PublicKey>,
    pub allow_none: bool,
    pub auth_banner_url: Option<String>,
    pub exec_with_sh: Option<PathBuf>,
}

impl Default for ServerCfg {
    fn default() -> Self {
        Self {
            host_key: PrivateKey::new(Ed25519Keypair::from_seed(&[1; 32]).into(), "")
                .expect("fixed Ed25519 test key is valid"),
            password: None,
            keyboard_interactive: false,
            accept_keys: Vec::new(),
            allow_none: false,
            auth_banner_url: None,
            exec_with_sh: None,
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct Observed {
    pub attempts: Vec<&'static str>,
    pub commands: Vec<String>,
    pub ptys: Vec<(String, u32, u32)>,
    pub sizes: Vec<(u32, u32)>,
    pub shells: usize,
    pub verified: Vec<PublicKey>,
}

pub struct SshTestServer {
    pub addr: SocketAddr,
    pub cfg: ServerCfg,
    pub observations: Arc<Mutex<Observed>>,
    pub sockets: Arc<Mutex<Vec<std::net::TcpStream>>>,
    pub stopped: Arc<AtomicBool>,
    pub task: JoinHandle<()>,
}

pub async fn start(cfg: ServerCfg) -> SshTestServer {
    SshTestServer::start(cfg).await
}

impl SshTestServer {
    pub async fn start(cfg: ServerCfg) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let observations = Arc::new(Mutex::new(Observed::default()));
        let sockets = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let task = Self::listen(
            listener,
            cfg.clone(),
            observations.clone(),
            sockets.clone(),
            stopped.clone(),
        );
        Self {
            addr,
            cfg,
            observations,
            sockets,
            stopped,
            task,
        }
    }

    pub fn listen(
        listener: TcpListener,
        cfg: ServerCfg,
        observed: Arc<Mutex<Observed>>,
        sockets: Arc<Mutex<Vec<std::net::TcpStream>>>,
        stopped: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut peers = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        // Keep a duplicate fd so stop() also interrupts russh's internal task.
                        let stream = stream.into_std().unwrap();
                        {
                            let mut sockets = sockets.lock().unwrap();
                            if stopped.load(Ordering::Acquire) { break; }
                            sockets.push(stream.try_clone().unwrap());
                        }
                        let stream = tokio::net::TcpStream::from_std(stream).unwrap();
                        let config = Arc::new(server::Config {
                            keys: vec![cfg.host_key.clone()],
                            methods: MethodSet::from(&[
                                MethodKind::PublicKey, MethodKind::Password,
                                MethodKind::KeyboardInteractive,
                            ][..]),
                            auth_rejection_time: Duration::from_millis(1),
                            auth_rejection_time_initial: Some(Duration::from_millis(1)),
                            max_auth_attempts: 20,
                            nodelay: true,
                            ..Default::default()
                        });
                        let handler = TestHandler {
                            cfg: cfg.clone(), observed: observed.clone(),
                            channels: HashMap::new(), forwarded: HashSet::new(),
                            lines: HashMap::new(), io_tasks: JoinSet::new(),
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
        })
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.task.abort();
        for socket in self.sockets.lock().unwrap().drain(..) {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }

    pub fn observed(&self) -> Observed {
        self.observations.lock().unwrap().clone()
    }

    /// Keeps configuration and observations; returns bind errors (including AddrInUse).
    pub async fn restart(&mut self) -> io::Result<()> {
        self.stop();
        if !self.task.is_finished() {
            let _ = (&mut self.task).await;
        }
        let listener = TcpListener::bind(self.addr).await?;
        self.stopped.store(false, Ordering::Release);
        self.task = Self::listen(
            listener,
            self.cfg.clone(),
            self.observations.clone(),
            self.sockets.clone(),
            self.stopped.clone(),
        );
        Ok(())
    }
}

impl Drop for SshTestServer {
    fn drop(&mut self) {
        self.stop();
    }
}

pub struct TestHandler {
    pub cfg: ServerCfg,
    pub observed: Arc<Mutex<Observed>>,
    pub channels: HashMap<ChannelId, Channel<server::Msg>>,
    pub forwarded: HashSet<ChannelId>,
    pub lines: HashMap<ChannelId, Zeroizing<Vec<u8>>>,
    pub io_tasks: JoinSet<()>,
}

impl TestHandler {
    pub fn accepts_key(&self, key: &PublicKey) -> Auth {
        if self
            .cfg
            .accept_keys
            .iter()
            .any(|accepted| accepted.key_data() == key.key_data())
        {
            Auth::Accept
        } else {
            Auth::reject()
        }
    }
}

impl server::Handler for TestHandler {
    type Error = russh::Error;

    async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
        self.observed.lock().unwrap().attempts.push("none");
        Ok(if self.cfg.allow_none {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn authentication_banner(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(self.cfg.auth_banner_url.clone())
    }

    async fn auth_password(&mut self, _: &str, password: &str) -> Result<Auth, Self::Error> {
        self.observed.lock().unwrap().attempts.push("password");
        Ok(if self.cfg.password.as_deref() == Some(password) {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn auth_publickey_offered(
        &mut self,
        _: &str,
        key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        self.observed.lock().unwrap().attempts.push("publickey");
        Ok(self.accepts_key(key))
    }

    async fn auth_publickey(&mut self, _: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        self.observed.lock().unwrap().verified.push(key.clone());
        Ok(self.accepts_key(key))
    }

    async fn auth_keyboard_interactive(
        &mut self,
        _: &str,
        _: &str,
        response: Option<server::Response<'_>>,
    ) -> Result<Auth, Self::Error> {
        if !self.cfg.keyboard_interactive {
            return Ok(Auth::reject());
        }
        if let Some(response) = response {
            let answers: Vec<_> = response.collect();
            return Ok(
                if answers.len() == 1
                    && self
                        .cfg
                        .password
                        .as_ref()
                        .is_some_and(|password| answers[0] == password.as_bytes())
                {
                    Auth::Accept
                } else {
                    Auth::reject()
                },
            );
        }
        self.observed
            .lock()
            .unwrap()
            .attempts
            .push("keyboard-interactive");
        Ok(Auth::Partial {
            name: "SSH authentication".into(),
            instructions: "Enter password".into(),
            prompts: vec![("Password: ".into(), false)].into(),
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

    #[allow(clippy::too_many_arguments)]
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        cols: u32,
        rows: u32,
        _: u32,
        _: u32,
        _: &[(Pty, u32)],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.observed
            .lock()
            .unwrap()
            .ptys
            .push((term.into(), cols, rows));
        session.channel_success(channel)
    }

    async fn window_change_request(
        &mut self,
        _: ChannelId,
        cols: u32,
        rows: u32,
        _: u32,
        _: u32,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.observed.lock().unwrap().sizes.push((cols, rows));
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.observed.lock().unwrap().shells += 1;
        session.channel_success(channel)?;
        session.data(channel, &b"ready\r\n"[..])
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if self.forwarded.contains(&channel) {
            return Ok(());
        }
        let line = self.lines.entry(channel).or_default();
        for &byte in data {
            if line.len() < 1024 {
                line.push(byte);
            }
            if byte == b'\n' || byte == b'\r' {
                if line.as_slice() == b"exit\n" || line.as_slice() == b"exit\r" {
                    session.eof(channel)?;
                    session.exit_status_request(channel, 0)?;
                    return session.close(channel);
                }
                line.clear();
            }
        }
        session.data(channel, data.to_vec())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        cmd: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let cmd = String::from_utf8_lossy(cmd).into_owned();
        self.observed.lock().unwrap().commands.push(cmd.clone());
        session.channel_success(channel)?;
        if let Some(home) = &self.cfg.exec_with_sh {
            self.forwarded.insert(channel);
            let (mut reader, writer) = self.channels.remove(&channel).unwrap().split();
            let home = home.clone();
            self.io_tasks.spawn(async move {
                let child = tokio::process::Command::new("/bin/sh")
                    .arg("-c")
                    .arg(cmd)
                    .env("HOME", &home)
                    .current_dir(home)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true)
                    .spawn();
                let mut inputs = JoinSet::new();
                let output = match child {
                    Ok(mut child) => {
                        let mut stdin = child.stdin.take().unwrap();
                        inputs.spawn(async move {
                            while let Some(msg) = reader.wait().await {
                                match msg {
                                    ChannelMsg::Data { data } => {
                                        if stdin.write_all(&data).await.is_err() {
                                            break;
                                        }
                                    }
                                    ChannelMsg::Eof | ChannelMsg::Close => break,
                                    _ => {}
                                }
                            }
                            let _ = stdin.shutdown().await;
                        });
                        child.wait_with_output().await
                    }
                    Err(error) => Err(error),
                };
                drop(inputs);
                match output {
                    Ok(output) => {
                        let _ = writer.data(&output.stdout[..]).await;
                        let _ = writer.extended_data(1, &output.stderr[..]).await;
                        let _ = writer
                            .exit_status(output.status.code().unwrap_or(1) as u32)
                            .await;
                    }
                    Err(_) => {
                        let _ = writer.exit_status(127).await;
                    }
                }
                let _ = writer.eof().await;
                let _ = writer.close().await;
            });
        } else if cmd.starts_with("tmux ") {
            session.data(channel, &b"tmux-ready\r\n"[..])?;
        } else {
            session.exit_status_request(channel, 0)?;
            session.eof(channel)?;
            session.close(channel)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<server::Msg>,
        host: &str,
        port: u32,
        _: &str,
        _: u32,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        // The fixture never forwards beyond loopback.
        if host != "127.0.0.1" || u16::try_from(port).is_err() {
            return Ok(());
        }
        let mut stream = tokio::net::TcpStream::connect((host, port as u16)).await?;
        self.forwarded.insert(channel.id());
        reply.accept().await;
        self.io_tasks.spawn(async move {
            let mut channel = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut channel, &mut stream).await;
        });
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.channels.remove(&channel);
        self.lines.remove(&channel);
        Ok(())
    }
}
