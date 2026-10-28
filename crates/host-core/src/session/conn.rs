//! Control framing, feature dispatch and connection teardown.

use super::{
    handlers::{self, activity, approval, input, ConnCx, ConnFeatures},
    handshake::{self, HandshakeContext, HandshakeOutcome, SessionContext},
    manager::{ConnCommand, ConnInfo, ConnKey, EndReason, HostServices, SessionManager},
};
use crate::{
    pairing::{PairingManager, PairingSession},
    platform::Platform,
    store::Store,
    RuntimeError, RuntimeOptions, RuntimeResult,
};
use rsp_core::{
    control::{encode, ControlError, EnvelopeDecoder},
    errors::ErrorCode,
    features, pb,
};
use rsp_transport::{
    CloseCode, Conn, ConnError, ControlStream, DatagramKind, StreamKind, TransportKind, UniRecv,
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinSet,
    time::{sleep, timeout, Instant},
};
use tracing::{debug, info};

struct SessionGuard {
    conn: Conn,
    closed: bool,
}
impl Drop for SessionGuard {
    fn drop(&mut self) {
        if !self.closed {
            self.conn.close(CloseCode::Normal, b"session ended");
        }
    }
}

pub async fn run<P: Platform>(
    conn: Conn,
    platform: Arc<P>,
    options: RuntimeOptions,
) -> RuntimeResult<()> {
    let (_shutdown, shutdown) = watch::channel(false);
    run_with_shutdown(conn, platform, options, shutdown).await
}
pub async fn run_with_context<P: Platform>(
    conn: Conn,
    mgr: Arc<SessionManager<P>>,
    options: RuntimeOptions,
    context: SessionContext,
) -> RuntimeResult<()> {
    run_inner(conn, mgr, options, context, None).await
}
pub(crate) async fn run_with_shutdown<P: Platform>(
    conn: Conn,
    platform: Arc<P>,
    options: RuntimeOptions,
    shutdown: watch::Receiver<bool>,
) -> RuntimeResult<()> {
    let mgr = SessionManager::new(platform.clone(), HostServices::default());
    mgr.start();
    let identity =
        crate::identity::HostIdentity::load_or_create(platform.keystore(), mgr.now_us())?;
    let store = Arc::new(Store::open(&options.store_dir)?);
    let local = options.bind.ok_or(RuntimeError::InvalidOptions(
        "compatibility session requires --bind",
    ))?;
    let pairing = PairingManager::new(
        store.clone(),
        identity.fp(),
        platform.info().name,
        vec![local],
    );
    run_inner(
        conn,
        mgr,
        options,
        SessionContext {
            tcp_tokens: None,
            local,
            path: crate::listen::PathClass::Lan,
            security: Arc::new(HandshakeContext {
                identity: Arc::new(identity),
                store,
                pairing,
                approver: None,
                audit: None,
                whois: Arc::new(crate::whois::ChainedWhoIs::new(
                    crate::whois::LocalApiSource::default(),
                    crate::whois::CliSource::default(),
                )),
            }),
        },
        Some(shutdown),
    )
    .await
}

// A listener can be cancelled while other listeners continue sharing the manager.
// Keep this guard outside State so its pipeline and tasks drop before unregistering.
struct Registration<P: Platform> {
    mgr: Arc<SessionManager<P>>,
    key: ConnKey,
}
impl<P: Platform> Drop for Registration<P> {
    fn drop(&mut self) {
        handlers::roster::unregister_tcp(&self.mgr, self.key);
        // Cancellation performs on_closed's roster accounting; on the normal
        // path on_closed already left the roster, so this is a no-op.
        handlers::roster::on_dropped(&self.mgr, self.key);
        self.mgr.unregister(self.key);
    }
}

#[derive(Default)]
struct State {
    info: Option<ConnInfo>,
    st: ConnFeatures,
    out: Vec<pb::Envelope>,
    end: Option<EndReason>,
    farewell: Option<&'static str>,
    tasks: JoinSet<()>,
    done: Vec<oneshot::Sender<()>>,
}
impl State {
    fn cx<'a, P: Platform>(
        &'a mut self,
        mgr: &'a Arc<SessionManager<P>>,
        conn: &'a Conn,
        options: &'a RuntimeOptions,
    ) -> Option<ConnCx<'a, P>> {
        let info = self.info.as_mut()?;
        Some(ConnCx {
            key: info.key,
            mgr,
            conn,
            info,
            options,
            st: &mut self.st,
            out: &mut self.out,
            end: &mut self.end,
            tasks: &mut self.tasks,
        })
    }
    async fn flush(&mut self, send: &ControlStream) -> RuntimeResult<()> {
        for env in self.out.drain(..) {
            send_ctrl(send, env).await?;
        }
        Ok(())
    }
}

async fn run_inner<P: Platform>(
    conn: Conn,
    mgr: Arc<SessionManager<P>>,
    options: RuntimeOptions,
    context: SessionContext,
    mut shutdown: Option<watch::Receiver<bool>>,
) -> RuntimeResult<()> {
    let mut guard = SessionGuard {
        conn: conn.clone(),
        closed: false,
    };
    let key = mgr.next_key();
    let (tx, mut rx) = mpsc::channel(64);
    let _registration = Registration {
        mgr: mgr.clone(),
        key,
    };
    let mut state = State::default();
    let port = input::InputPort {
        platform: mgr.platform().clone(),
        gate: state.st.input.gate_tx.subscribe(),
        display: state.st.input.display_tx.subscribe(),
        cmd: tx.clone(),
        dropped: state.st.input.dropped.clone(),
        stats: state.st.input.stats.clone(),
    };
    let (uni_tx, mut uni_rx) = mpsc::channel(64);
    state
        .tasks
        .spawn(dispatch_uni(conn.clone(), port.clone(), uni_tx));
    state.tasks.spawn(read_datagrams(conn.clone(), port));
    let send = conn.accept_control().await?;
    info!(remote = %conn.remote_addr(), "connected");
    let control = Control {
        conn: &conn,
        mgr: &mgr,
        key,
        options: &options,
        context: &context,
        tx,
        pairing: None,
    };
    let result = tokio::select! {
        biased;
        result = control_loop(control, &send, &mut rx, &mut uni_rx, &mut state) => result,
        () = async {
            if let Some(shutdown) = shutdown.as_mut() {
                loop {
                    if *shutdown.borrow_and_update() { break; }
                    if shutdown.changed().await.is_err() { std::future::pending::<()>().await; }
                }
            } else { std::future::pending::<()>().await; }
        } => Ok(EndReason::Replaced),
        // Dropping a pending handshake also drops its OpenPrompt guard.
        _ = conn.closed() => Ok(EndReason::PeerClosed),
    };
    let reason = result.as_ref().copied().unwrap_or(EndReason::PeerClosed);
    handlers::roster::unregister_tcp(&mgr, key);
    if let Some(mut cx) = state.cx(&mgr, &conn, &options) {
        handlers::on_closed(&mut cx, reason);
    }
    // Stop capture before unregistering or acknowledging replacement.
    drop(std::mem::take(&mut state.st));
    state.tasks.abort_all();
    mgr.unregister(key);
    rx.close();
    while let Ok(cmd) = rx.try_recv() {
        if let ConnCommand::End {
            done: Some(done), ..
        } = cmd
        {
            state.done.push(done);
        }
    }
    if reason == EndReason::Replaced {
        debug!(key = key.0, "replacement acknowledged before drain");
        for done in state.done.drain(..) {
            let _ = done.send(());
        }
    }
    let _ = state.flush(&send).await;
    if let Some(goodbye) = reason.goodbye() {
        let _ = send_ctrl(
            &send,
            pb::Goodbye {
                reason: goodbye as i32,
                message: state
                    .farewell
                    .unwrap_or(match reason {
                        EndReason::Replaced => "replaced by a new connection",
                        _ => "",
                    })
                    .into(),
            },
        )
        .await;
    }
    if reason.waits_for_drain() {
        if conn.kind() == TransportKind::Tcp {
            let _ = timeout(Duration::from_secs(2), send.flush()).await;
        } else {
            let _ = send.finish().await;
            let _ = timeout(Duration::from_secs(2), send.stopped()).await;
        }
    }
    let close_reason: &[u8] = match reason {
        EndReason::Protocol | EndReason::ProtocolImmediate => b"protocol error",
        EndReason::Replaced => b"session replaced",
        EndReason::HostUpdating => b"host updating",
        EndReason::AuthDenied => b"authentication denied",
        _ => b"session ended",
    };
    let close_code = match reason.close_code() {
        1 => CloseCode::Protocol,
        2 => CloseCode::Auth,
        3 => CloseCode::Replaced,
        4 => CloseCode::HostShutdown,
        5 => CloseCode::Idle,
        6 => CloseCode::Revoked,
        _ => CloseCode::Normal,
    };
    conn.close(close_code, close_reason);
    guard.closed = true;
    drop(guard);
    for done in state.done {
        let _ = done.send(());
    }
    result.map(|_| ())
}

async fn send_ctrl(send: &ControlStream, env: impl Into<pb::Envelope>) -> RuntimeResult<()> {
    let frame = encode(&env.into());
    send.send(&frame[4..]).await?;
    Ok(())
}
async fn send_error(
    send: &ControlStream,
    code: ErrorCode,
    message: impl Into<String>,
) -> RuntimeResult<()> {
    send_ctrl(
        send,
        pb::ErrorInfo {
            code: u32::from(code.code()),
            message: message.into(),
            ..Default::default()
        },
    )
    .await
}

struct Control<'a, P: Platform> {
    conn: &'a Conn,
    mgr: &'a Arc<SessionManager<P>>,
    key: ConnKey,
    options: &'a RuntimeOptions,
    context: &'a SessionContext,
    tx: mpsc::Sender<ConnCommand>,
    pairing: Option<Box<PairingSession>>,
}

impl<P: Platform> Control<'_, P> {
    async fn command(
        &self,
        command: Option<ConnCommand>,
        send: &ControlStream,
        state: &mut State,
    ) -> RuntimeResult<Option<EndReason>> {
        match command {
            Some(ConnCommand::Send(env)) => send_ctrl(send, env).await?,
            Some(ConnCommand::End { reason, done }) => {
                if let Some(done) = done {
                    state.done.push(done);
                }
                return Ok(Some(reason));
            }
            Some(ConnCommand::SetRole(role)) => {
                if let Some(mut cx) = state.cx(self.mgr, self.conn, self.options) {
                    let old = cx.info.role;
                    cx.info.role = role;
                    cx.sync_info();
                    handlers::on_role_changed(&mut cx, old);
                    handlers::recompute_gate(&mut cx);
                }
            }
            Some(ConnCommand::Feature(ev)) => {
                if let Some(mut cx) = state.cx(self.mgr, self.conn, self.options) {
                    handlers::on_event(&mut cx, ev);
                }
            }
            Some(ConnCommand::Tick) => {
                if let Some(mut cx) = state.cx(self.mgr, self.conn, self.options) {
                    handlers::on_tick(&mut cx, self.mgr.now_us());
                }
            }
            None => return Ok(Some(EndReason::PeerClosed)),
        }
        Ok(None)
    }

    async fn message(
        &mut self,
        msg: pb::envelope::Msg,
        send: &ControlStream,
        state: &mut State,
    ) -> RuntimeResult<Option<EndReason>> {
        match msg {
            pb::envelope::Msg::ClientHello(hello) => return self.hello(hello, send, state).await,
            message @ (pb::envelope::Msg::PairStart(_)
            | pb::envelope::Msg::PairPassword(_)
            | pb::envelope::Msg::PairSpake(_)
            | pb::envelope::Msg::PairConfirm(_)) => {
                let Some(session) = self.pairing.as_mut() else {
                    return Ok(Some(EndReason::ProtocolImmediate));
                };
                if let Some(code) = handshake::pairing_message(
                    session,
                    message,
                    &mut handshake::HandshakeWriter::Transport(send),
                    &self.context.security,
                    self.mgr.now_us(),
                    self.mgr,
                )
                .await?
                {
                    return Ok(Some(match code {
                        rsp_transport::CloseCode::Normal => EndReason::Normal,
                        rsp_transport::CloseCode::Auth => EndReason::AuthDenied,
                        _ => EndReason::ProtocolImmediate,
                    }));
                }
            }
            pb::envelope::Msg::Ping(ping) => {
                send_ctrl(
                    send,
                    pb::Pong {
                        t1_us: ping.t1_us,
                        t2_us: self.mgr.now_us(),
                    },
                )
                .await?
            }
            pb::envelope::Msg::Pong(_) => {}
            pb::envelope::Msg::Goodbye(goodbye) => {
                return Ok(Some(EndReason::Peer(goodbye.reason)))
            }
            other => {
                if let Some(mut cx) = state.cx(self.mgr, self.conn, self.options) {
                    if !handlers::on_control(&mut cx, other) {
                        debug!("ignoring unsupported control message");
                    }
                }
            }
        }
        Ok(None)
    }

    async fn hello(
        &mut self,
        hello: pb::ClientHello,
        send: &ControlStream,
        state: &mut State,
    ) -> RuntimeResult<Option<EndReason>> {
        if state.info.is_some() || self.pairing.is_some() {
            return Ok(Some(EndReason::ProtocolImmediate));
        }
        if hello.protocol != features::PROTOCOL_VERSION {
            let code = if hello.protocol > features::PROTOCOL_VERSION {
                ErrorCode::HostTooOld
            } else {
                ErrorCode::AppTooOld
            };
            send_error(send, code, "unsupported protocol version").await?;
            state.farewell = Some("protocol mismatch");
            return Ok(Some(EndReason::HostUpdating));
        }
        if hello.required_features & !handlers::host_features() != 0 {
            state.farewell = Some("required feature mismatch");
            send_ctrl(
                send,
                pb::ErrorInfo {
                    code: u32::from(ErrorCode::HostTooOld.code()),
                    message: "required feature is unavailable".into(),
                    detail: HashMap::from([
                        ("host_version".into(), env!("CARGO_PKG_VERSION").into()),
                        ("app_version".into(), hello.app_version),
                    ]),
                    ..Default::default()
                },
            )
            .await?;
            return Ok(Some(EndReason::HostUpdating));
        }
        // Keep decode preferences in connection-local stream state for rebuilds.
        state.st.stream.codecs = hello
            .caps
            .as_ref()
            .map(|caps| {
                caps.decode
                    .iter()
                    .filter_map(|&codec| pb::Codec::try_from(codec).ok())
                    .collect()
            })
            .unwrap_or_default();
        state.st.audio.caps_audio = hello.caps.as_ref().is_some_and(|c| c.audio_opus);
        match handshake::authenticate(
            self.conn,
            self.mgr,
            self.key,
            self.options,
            hello,
            send,
            self.context,
        )
        .await?
        {
            HandshakeOutcome::Accepted(accepted) => {
                return self.admit(*accepted, send, state).await
            }
            HandshakeOutcome::Probe => return Ok(Some(EndReason::Normal)),
            HandshakeOutcome::Denied | HandshakeOutcome::PairDenied => {
                return Ok(Some(EndReason::AuthDenied))
            }
            HandshakeOutcome::Pair(session) => self.pairing = Some(session),
        }
        Ok(None)
    }

    async fn admit(
        &self,
        accepted: handshake::Accepted,
        send: &ControlStream,
        state: &mut State,
    ) -> RuntimeResult<Option<EndReason>> {
        let mgr = self.mgr;
        // M0 admission and replacement are atomic across all listeners.
        let _admission = mgr.admission_lock.lock().await;
        let mut info = ConnInfo {
            key: self.key,
            session_id: String::new(),
            device_id: if accepted.peer.device_id.is_empty() {
                format!("anon-{}", self.key.0)
            } else {
                accepted.peer.device_id
            },
            device_fp: accepted.device_fp.unwrap_or([0; 32]),
            device_name: accepted.device_name,
            platform: accepted.platform,
            user: accepted.peer.user,
            node: accepted.peer.node,
            path: pb::PathKind::try_from(accepted.peer.path).unwrap_or_default(),
            derp_region: accepted.peer.derp_region,
            intent: accepted.intent,
            client_features: accepted.client_features,
            role: accepted.role,
            can_control: accepted.can_control,
            since_us: mgr.now_us(),
            resumed: accepted.resumed.is_some(),
        };
        let admission = match accepted
            .resumed
            .map(Ok)
            .unwrap_or_else(|| handlers::roster::admit(mgr, &info, accepted.role))
        {
            Ok(a) => a,
            Err(code) => return Self::deny(send, code, code.as_str()).await,
        };
        let mut replacing = JoinSet::new();
        for &key in &admission.replaced {
            let mgr = mgr.clone();
            replacing.spawn(async move { mgr.end(key, EndReason::Replaced).await });
        }
        let _ = timeout(Duration::from_secs(2), async {
            while replacing.join_next().await.is_some() {}
        })
        .await;
        // A stalled control writer must not allow overlapping M0 captures.
        if admission
            .replaced
            .iter()
            .any(|key| mgr.info(*key).is_some())
        {
            return Self::deny(send, ErrorCode::Other, "previous session did not stop").await;
        }
        info.session_id = admission.session_id;
        info.role = admission.role;
        mgr.register(info.clone(), self.tx.clone());
        state.info = Some(info);
        if self.conn.kind() == TransportKind::Tcp {
            if let Some(registry) = &self.context.tcp_tokens {
                handlers::roster::register_tcp(
                    mgr,
                    self.key,
                    registry,
                    admission.token.to_vec(),
                    self.conn,
                )?;
            }
        }
        send_ctrl(
            send,
            pb::AuthResult {
                status: pb::auth_result::Status::Ok as i32,
                role: handshake::role_to_pb(admission.role),
                session_token: admission.token.to_vec(),
                resumed: admission.resumed,
                ..Default::default()
            },
        )
        .await?;
        if let Some(mut cx) = state.cx(mgr, self.conn, self.options) {
            handlers::on_admitted(&mut cx);
            handlers::recompute_gate(&mut cx);
        }
        Ok(None)
    }

    async fn deny(
        send: &ControlStream,
        code: ErrorCode,
        message: &str,
    ) -> RuntimeResult<Option<EndReason>> {
        send_ctrl(
            send,
            pb::AuthResult {
                status: pb::auth_result::Status::Denied as i32,
                error: Some(pb::ErrorInfo {
                    code: u32::from(code.code()),
                    message: message.into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await?;
        Ok(Some(EndReason::AuthDenied))
    }
}

async fn control_loop<P: Platform>(
    mut control: Control<'_, P>,
    send: &ControlStream,
    rx: &mut mpsc::Receiver<ConnCommand>,
    uni_rx: &mut mpsc::Receiver<(u8, UniRecv)>,
    state: &mut State,
) -> RuntimeResult<EndReason> {
    let mut decoder = EnvelopeDecoder::new();
    let mut buf = [0u8; 4096];
    let mut paused = activity::paused_watch(control.mgr);
    let mut prompt = approval::prompt_watch(control.mgr);
    let mut uni_open = true;
    loop {
        let deadline = state
            .cx(control.mgr, control.conn, control.options)
            .and_then(|cx| handlers::deadline(&cx));
        let delay = handlers::timer_delay(deadline, control.mgr.now_us());
        let end = tokio::select! {
            biased;
            command = rx.recv() => control.command(command, send, state).await?,
            () = watch_changed(&mut paused) => {
                if let Some(mut cx) = state.cx(control.mgr, control.conn, control.options) {
                    handlers::recompute_gate(&mut cx);
                }
                None
            }
            () = watch_changed(&mut prompt) => {
                if let Some(mut cx) = state.cx(control.mgr, control.conn, control.options) {
                    handlers::recompute_gate(&mut cx);
                }
                None
            }
            stream = uni_rx.recv(), if state.info.is_some() && uni_open => {
                if let Some((kind, recv)) = stream {
                    if let Some(mut cx) = state.cx(control.mgr, control.conn, control.options) {
                        handlers::on_uni_stream(&mut cx, kind, recv);
                    }
                } else { uni_open = false; }
                None
            }
            () = sleep(delay) => {
                let now = control.mgr.now_us();
                if deadline.is_some_and(|at| now >= at) {
                    if let Some(mut cx) = state.cx(control.mgr, control.conn, control.options) {
                        handlers::on_tick(&mut cx, now);
                    }
                }
                None
            }
            read = async {
                if control.conn.kind() == TransportKind::Tcp {
                    send.recv().await
                } else {
                    send.read_raw(&mut buf).await.map(|n| n.map(|n| bytes::Bytes::copy_from_slice(&buf[..n])))
                }
            } => {
                let bytes = match read {
                    Ok(Some(bytes)) => bytes,
                    Ok(None) => {
                        if control.conn.kind() == TransportKind::Tcp && control.conn.closed().await.code == Some(1) {
                            return Ok(EndReason::ProtocolImmediate);
                        }
                        return Ok(EndReason::PeerClosed);
                    }
                    Err(ConnError::Protocol(_)) => return Ok(EndReason::ProtocolImmediate),
                    Err(error) => return Err(error.into()),
                };
                if control.conn.kind() == TransportKind::Tcp {
                    decoder.push(&(bytes.len() as u32).to_be_bytes());
                }
                decoder.push(&bytes);
                loop {
                    let env = match decoder.next() {
                        Ok(Some(env)) => env,
                        Ok(None) => break,
                        Err(ControlError::FrameTooLarge(_)) => return Ok(EndReason::ProtocolImmediate),
                        Err(error @ ControlError::Protobuf(_)) => {
                            let _ = send_error(send, ErrorCode::Other, format!("invalid protobuf control frame: {error}")).await;
                            return Ok(EndReason::Protocol);
                        }
                    };
                    if let Some(msg) = env.msg {
                        if let Some(reason) = control.message(msg, send, state).await? { return Ok(reason); }
                    }
                    state.flush(send).await?;
                    if let Some(reason) = state.end.take() { return Ok(reason); }
                }
                None
            }
        };
        state.flush(send).await?;
        if let Some(reason) = end.or_else(|| state.end.take()) {
            return Ok(reason);
        }
    }
}

async fn watch_changed(watch: &mut Option<watch::Receiver<bool>>) {
    if let Some(receiver) = watch {
        if receiver.changed().await.is_ok() {
            return;
        }
        *watch = None;
    }
    std::future::pending().await
}

async fn dispatch_uni<P: Platform>(
    conn: Conn,
    port: input::InputPort<P>,
    streams: mpsc::Sender<(u8, UniRecv)>,
) {
    // Owning the children cancels incomplete input and type reads on teardown.
    let mut tasks = JoinSet::new();
    let Some(quic) = conn.as_quic() else {
        while let Ok((kind, recv)) = conn.accept_uni().await {
            match kind {
                StreamKind::Input => {
                    let port = port.clone();
                    // Keep ownership with the dispatcher, as on QUIC.
                    tasks.spawn(input::read_stream(port, recv, false));
                }
                _ => {
                    let _ = streams.send((kind as u8, recv)).await;
                }
            }
        }
        return;
    };
    // Conn::accept_uni reads the type inline. The QUIC compatibility branch
    // keeps one type reader per stream, the ten-second timeout, and M0 0x00
    // input, so a stalled type byte cannot block later input or control.
    loop {
        tokio::select! {
            stream = quic.accept_uni() => {
                let Ok(mut recv) = stream else { return; };
                let port = port.clone();
                let streams = streams.clone();
                tasks.spawn(async move {
                    let mut kind = [0];
                    if !matches!(timeout(Duration::from_secs(10), recv.read_exact(&mut kind)).await, Ok(Ok(()))) {
                        return;
                    }
                    let mut recv = UniRecv::from(recv);
                    match kind[0] {
                        0x00 => input::read_stream(port, recv, true).await,
                        0x01 => input::read_stream(port, recv, false).await,
                        0x03 | 0x04 => { let _ = streams.send((kind[0], recv)).await; }
                        _ => { let _ = recv.stop(1); }
                    }
                });
            }
            _ = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
}

async fn read_datagrams<P: Platform>(conn: Conn, port: input::InputPort<P>) {
    let mut filter = rsp_core::pointer::PointerFilter::new();
    let mut unknown_datagrams = 0u64;
    let mut last_log = None;
    let Some(quic) = conn.as_quic() else {
        while let Ok((kind, payload)) = conn.recv_datagram().await {
            if kind == DatagramKind::Pointer {
                let mut datagram = Vec::with_capacity(1 + payload.len());
                datagram.push(kind as u8);
                datagram.extend_from_slice(&payload);
                input::on_pointer_datagram(&port, &mut filter, &datagram);
            }
        }
        return;
    };
    // Retain session-level unknown-datagram accounting and empty heartbeats.
    while let Ok(datagram) = quic.read_datagram().await {
        match datagram.first() {
            None => {}
            Some(0x12) => input::on_pointer_datagram(&port, &mut filter, &datagram),
            Some(_) => {
                unknown_datagrams = unknown_datagrams.saturating_add(1);
                let now = Instant::now();
                if last_log.is_none_or(|last| now.duration_since(last) >= Duration::from_secs(10)) {
                    debug!(unknown_datagrams, "discarding unknown datagrams");
                    last_log = Some(now);
                }
            }
        }
    }
}
