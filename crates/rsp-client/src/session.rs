//! RSP session state, commands and events. Network I/O is isolated in `io`.
mod audio_rx;
mod feedback;
mod io;
mod pathio;
mod redial;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicBool, AtomicI32, Ordering},
    Arc, Mutex, MutexGuard,
};
use std::time::Duration;

use prost::Message;
use rsp_core::clock::{unix_micros, ClockSync, PingSchedule};
use rsp_core::errors::ErrorCode;
use rsp_core::pairing::{AppPairing, AppPasswordPairing, Outcome as PairingOutcome, PairingError};
use rsp_core::stats::{ClientStats, ClientStatsCollector, DelayGradient};
use rsp_core::video::{KeyframeReason, Reassembler, ReassemblyEvent};
use rsp_core::{features, hid::InputMsg, pb, pointer::PointerDatagram};
use rsp_transport::dial::{
    self, Candidate, CandidateSource, DialOpts, IpNet, LocalNet, Observations, PathKind,
};
use rsp_transport::dialer::Dialer;
use rsp_transport::identity::LocalIdentity;
use rsp_transport::{CloseCode, Conn, ConnError, StreamKind, TransportKind, UniSend};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{timeout, Instant};
use zeroize::{Zeroize, Zeroizing};

use crate::clipboard::{ClipboardClientError, ClipboardPolicy};
use crate::recovery::driver::{DriverConfig, PathDriver};
use crate::recovery::failover::TicketSlot;
use crate::recovery::liveness::PathSig;
use crate::viewport::{ViewportInput, ViewportReporter};
use crate::{files::FileCtl, prewarm::PrewarmPool, ReceivedFrame};
use io::{Connection, Control, Datagram, Input};
use redial::DialContext;

pub const CLIENT_FEATURES: u64 = features::FEC_RS
    | features::CURSOR_SHAPE
    | features::POINTER_DGRAM
    | features::MULTI_DISPLAY
    | features::KEY_HID
    | features::TEXT_INPUT
    | features::TCP_TRANSPORT
    | features::RESUME
    | features::MULTI_VIEWER
    | features::HQ_REFRESH
    | features::RATE_FEEDBACK
    | features::SESSION_ACTIONS
    | features::VIEWPORT
    | features::CLIPBOARD_TEXT;

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStateKind {
    Connecting,
    Authenticating,
    PendingApproval,
    Streaming,
    Reconnecting,
    Ended,
}
pub type SessionState = SessionStateKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateTransition {
    Connected,
    Authenticated,
    ApprovalPending,
    ApprovalGranted,
    StartStreaming,
    Disconnected,
    End(ErrorCode),
}
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error("invalid session transition from {from:?} on {event:?}")]
pub struct InvalidTransition {
    pub from: SessionStateKind,
    pub event: StateTransition,
}

/// Pure transition table: no I/O, clocks, or panics.
pub fn transition(
    state: SessionStateKind,
    event: StateTransition,
) -> Result<SessionStateKind, InvalidTransition> {
    use SessionStateKind as S;
    use StateTransition as E;
    match (state, event) {
        (S::Connecting, E::Connected) => Ok(S::Authenticating),
        (S::Authenticating, E::ApprovalPending) => Ok(S::PendingApproval),
        (S::Authenticating, E::Authenticated | E::StartStreaming)
        | (S::PendingApproval, E::ApprovalGranted) => Ok(S::Streaming),
        (S::Streaming, E::Disconnected) => Ok(S::Reconnecting),
        (S::Reconnecting, E::Authenticated) => Ok(S::Streaming),
        (S::Reconnecting, E::ApprovalPending) => Ok(S::PendingApproval),
        (
            S::Connecting | S::Authenticating | S::PendingApproval | S::Streaming | S::Reconnecting,
            E::End(_),
        ) => Ok(S::Ended),
        _ => Err(InvalidTransition { from: state, event }),
    }
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("dial failed: {code:?}")]
    Dial {
        code: ErrorCode,
        detail: BTreeMap<String, String>,
    },
    #[error("transport error: {0}")]
    Transport(#[from] rsp_transport::ConnError),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("identity configuration failed: {0}")]
    Identity(String),
    #[error("probe ended: {0:?}")]
    Probe(EndReason),
}

/// Used for `intent=PAIR`; when set, this takes precedence over `pairing_code`.
pub enum PairingCredential {
    Code(String),
    File {
        file_id: [u8; 16],
        secret: Zeroizing<[u8; 32]>,
    },
    Password {
        account: String,
        password: Zeroizing<Vec<u8>>,
    },
}

impl std::fmt::Debug for PairingCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Code(_) => f.write_str("Code(<redacted>)"),
            Self::File { file_id, .. } => {
                write!(
                    f,
                    "File({})",
                    data_encoding::BASE64URL_NOPAD.encode(file_id)
                )
            }
            Self::Password { .. } => f.write_str("Password(<redacted>)"),
        }
    }
}

/// Identity is owned by an Arc, so LocalIdentity backed by ExternalSigner is
/// usable without copying private material. Fingerprints are SPKI SHA-256.
pub struct SessionParams {
    pub candidates: Vec<Candidate>,
    pub host_fingerprint: [u8; 32],
    pub local_identity: Arc<LocalIdentity>,
    pub intent: pb::Intent,
    pub resume_token: Option<Vec<u8>>,
    pub device_name: String,
    pub platform: pb::Platform,
    pub start: crate::StartParams,
    pub display_id: Option<u32>,
    pub quality: pb::Quality,
    /// 握手时请求声音（ClientCaps.audio_opus）。默认 false。
    pub audio: bool,
    /// 接收声音的回调。None 时不声明 AUDIO_OPUS。默认 None。
    pub audio_sink: Option<Arc<dyn crate::audio::AudioSink>>,
    pub derp_only: bool,
    pub features: u64,
    pub required_features: u64,
    pub dialer: Option<Arc<dyn Dialer>>,
    /// Network for the default dialer; ignored when `dialer` is `Some`.
    pub net: Arc<dyn rsp_transport::NetDialer>,
    pub prewarm: Option<Arc<PrewarmPool>>,
    pub local_net: LocalNet,
    pub intranet_cidrs: Vec<IpNet>,
    pub observations: Observations,
    pub dial_options: DialOpts,
    /// Pairing code used when `intent` is `PAIR`.
    pub pairing_code: Option<String>,
    /// Pairing credential used for `PAIR`, in preference to `pairing_code`.
    pub pairing: Option<PairingCredential>,
    /// Retained M0 Host input streams omit the StreamKind byte. Leave false
    /// for the current protocol; used during Host input migration only.
    pub legacy_quic_input: bool,
}
impl std::fmt::Debug for SessionParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionParams")
            .field("net", &self.net)
            .field("intent", &self.intent)
            .field("required_features", &self.required_features)
            .field(
                "pairing_code",
                &self.pairing_code.as_ref().map(|_| "<redacted>"),
            )
            .field("pairing", &self.pairing)
            .finish_non_exhaustive()
    }
}
impl SessionParams {
    pub fn new(addresses: Vec<SocketAddr>, fingerprint: [u8; 32], identity: LocalIdentity) -> Self {
        Self::from_candidates(
            addresses
                .into_iter()
                .map(|addr| Candidate {
                    addr,
                    kind: PathKind::Manual,
                    source: CandidateSource::Manual,
                })
                .collect(),
            fingerprint,
            identity,
        )
    }
    pub fn from_candidates(
        candidates: Vec<Candidate>,
        host_fingerprint: [u8; 32],
        identity: LocalIdentity,
    ) -> Self {
        Self::from_shared(candidates, host_fingerprint, Arc::new(identity))
    }
    pub fn from_shared(
        candidates: Vec<Candidate>,
        host_fingerprint: [u8; 32],
        identity: Arc<LocalIdentity>,
    ) -> Self {
        Self {
            candidates,
            host_fingerprint,
            local_identity: identity,
            intent: pb::Intent::Control,
            resume_token: None,
            device_name: "rsp-client".into(),
            platform: pb::Platform::Unspecified,
            start: crate::StartParams::default(),
            display_id: None,
            quality: pb::Quality::Auto,
            audio: false,
            audio_sink: None,
            derp_only: false,
            features: CLIENT_FEATURES,
            required_features: 0,
            dialer: None,
            net: rsp_transport::system_net(),
            prewarm: None,
            local_net: LocalNet::default(),
            intranet_cidrs: Vec::new(),
            observations: Observations::default(),
            dial_options: DialOpts::default(),
            pairing_code: None,
            pairing: None,
            legacy_quic_input: false,
        }
    }
    pub fn single(addr: SocketAddr, fingerprint: [u8; 32], identity: LocalIdentity) -> Self {
        Self::new(vec![addr], fingerprint, identity)
    }
    pub fn single_shared(
        addr: SocketAddr,
        fingerprint: [u8; 32],
        identity: Arc<LocalIdentity>,
    ) -> Self {
        Self::from_shared(
            vec![Candidate {
                addr,
                kind: PathKind::Manual,
                source: CandidateSource::Manual,
            }],
            fingerprint,
            identity,
        )
    }
    pub fn with_pairing_code(mut self, code: impl Into<String>) -> Self {
        self.pairing_code = Some(code.into());
        self
    }
    pub fn with_pairing_file(mut self, file_id: [u8; 16], secret: Zeroizing<[u8; 32]>) -> Self {
        self.pairing = Some(PairingCredential::File { file_id, secret });
        self
    }
    pub fn with_pairing_password(
        mut self,
        account: impl Into<String>,
        password: Zeroizing<Vec<u8>>,
    ) -> Self {
        self.pairing = Some(PairingCredential::Password {
            account: account.into(),
            password,
        });
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionStats {
    pub frames_received: u64,
    pub frames_lost: u64,
    pub frames_recovered: u64,
    pub keyframe_requests: u64,
    pub pings_sent: u64,
    pub rtt_us: Option<u64>,
    pub offset_us: Option<i64>,
    pub recv_kbps: u32,
    /// A9/A10 HUD snapshot, updated at 2 Hz.
    pub hud: Option<ClientStats>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct CursorEvent {
    pub x: f32,
    pub y: f32,
    pub visible: bool,
}

/// Normal protocol endings have no ErrorCode. ConnectionLost is retained as a
/// typed reason because errors.rs has no distinct connection-loss code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    Goodbye(pb::goodbye::Reason),
    ConnectionLost,
    Protocol,
    Error(ErrorCode),
    Close(u64),
}
pub struct IncomingFile(pub rsp_transport::UniRecv);

impl std::fmt::Debug for IncomingFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("IncomingFile")
    }
}

#[derive(Debug)]
pub enum SessionEvent {
    StateChanged(SessionStateKind),
    ServerHello(pb::ServerHello),
    SessionState(pb::SessionState),
    Role(pb::Role),
    PairResult(pb::PairResult),
    Displays(Vec<pb::Display>),
    /// 回应 CreateVirtualDisplay（03 §3「虚拟显示器」）。
    VirtualDisplay(pb::VirtualDisplayResult),

    StreamConfig(pb::StreamConfig),
    Frame(ReceivedFrame),
    Cursor(CursorEvent),
    CursorMessage(pb::CursorMsg),
    FileStream(IncomingFile),
    FileListing(pb::FileListing),
    FileStatus(pb::FileStatus),
    FileCancel(pb::FileCancel),
    ControlRequested(pb::ControlRequest),
    IdleWarning(pb::IdleWarning),
    LocalUse(bool),
    RateUpdate(pb::RateUpdate),
    HostStats(pb::HostStats),
    ClockSync {
        rtt_us: u64,
        offset_us: i64,
    },
    Stats(SessionStats),
    /// Extra context for the immediately following `Error` with the same code.
    ErrorDetail {
        code: ErrorCode,
        detail: BTreeMap<String, String>,
        retry_after_s: Option<u32>,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
    Clipboard(rsp_core::clipboard::ClipboardItem),
    Ended {
        code: Option<ErrorCode>,
        reason: String,
        kind: EndReason,
    },
}
pub type EventStream = mpsc::UnboundedReceiver<SessionEvent>;

// Commands are intentionally not Debug: input may contain typed text.
enum Command {
    Close,
    CloseWith(pb::goodbye::Reason),
    SetAudio(bool),
    SetRecording(bool),
    Viewport(ViewportInput),
    Control(Box<pb::Envelope>),
    OpenFile(oneshot::Sender<Result<UniSend, ConnError>>),
    ClipboardSend(rsp_core::clipboard::ClipboardItem),
    Input(Vec<u8>),
    Pointer(f32, f32),
    Keyframe(pb::keyframe_request::Reason),
    Decoded {
        decode_us: u64,
        latency_us: u64,
        queue: u32,
    },
}
struct Shared {
    state: Mutex<SessionStateKind>,
    stats: Mutex<SessionStats>,
    resume_token: Mutex<TicketSlot>,
    host_fingerprint: [u8; 32],
    resumed: AtomicBool,
    viewport_mode: AtomicI32,
    timings: Mutex<HandshakeTimings>,
    low_power: Mutex<bool>,
    role: Mutex<pb::Role>,
    peer_features: std::sync::atomic::AtomicU64,
    local_features: u64,
    dial_report: DialReport,
}
#[derive(Clone)]
pub struct SessionHandle {
    commands: mpsc::UnboundedSender<Command>,
    shared: Arc<Shared>,
}
impl SessionHandle {
    pub fn close(&self) {
        let _ = self.commands.send(Command::Close);
    }
    /// Say goodbye with a specific reason and close this connection.
    pub fn close_with(&self, reason: pb::goodbye::Reason) {
        let _ = self.commands.send(Command::CloseWith(reason));
    }
    pub fn send_file_ctl(&self, msg: FileCtl) {
        self.control(msg);
    }
    pub async fn open_file_stream(&self) -> Result<UniSend, SessionError> {
        if self.state() == SessionStateKind::Ended {
            return Err(ConnError::Closed.into());
        }
        let (send, recv) = oneshot::channel();
        self.commands
            .send(Command::OpenFile(send))
            .map_err(|_| ConnError::Closed)?;
        recv.await
            .map_err(|_| ConnError::Closed)?
            .map_err(Into::into)
    }
    /// Sends one UTF-8 text clipboard item to the Host.
    pub fn send_clipboard_text(&self, text: &str) -> Result<(), ClipboardClientError> {
        let item = crate::clipboard::text_item(text)?;
        let policy = ClipboardPolicy {
            peer_features: self.shared.peer_features.load(Ordering::Acquire),
            role: *lock(&self.shared.role),
            to_phone: true,
            from_phone: true,
        };
        crate::clipboard::check_send_policy(item.kind, &policy)?;
        if self.shared.local_features & features::CLIPBOARD_TEXT == 0 {
            return Err(ClipboardClientError::FeatureUnsupported(item.kind));
        }
        if self.state() != SessionStateKind::Streaming {
            return Err(ClipboardClientError::Transport(ConnError::Closed));
        }
        self.commands
            .send(Command::ClipboardSend(item))
            .map_err(|_| ClipboardClientError::Transport(ConnError::Closed))
    }
    pub fn request_control(&self) {
        self.control(pb::RequestControl {});
    }
    pub fn respond_control(&self, requester_device_id: impl Into<String>, allow: bool) {
        self.control(pb::ControlResponse {
            requester_device_id: requester_device_id.into(),
            allow,
        });
    }
    pub fn select_display(&self, display_id: u32) {
        self.control(pb::SelectDisplay { display_id });
    }
    /// 请 Host 新建虚拟显示器；结果以 SessionEvent::VirtualDisplay 返回。Host 没声明 VIRTUAL_DISPLAY 时调用方不该调用。
    pub fn create_virtual_display(&self, request: pb::CreateVirtualDisplay) {
        self.control(request);
    }
    /// 移除本会话建的虚拟显示器（不回应）。
    pub fn release_virtual_display(&self, display_id: u32) {
        self.control(pb::ReleaseVirtualDisplay { display_id });
    }

    pub fn set_quality(&self, quality: pb::Quality) {
        self.control(pb::SetQuality {
            quality: quality as i32,
        });
    }
    pub fn pause(&self) {
        self.control(pb::PauseStream {});
    }
    /// Toggle remote audio when the streaming Host supports AUDIO_OPUS.
    pub fn set_audio(&self, enabled: bool) {
        let _ = self.commands.send(Command::SetAudio(enabled));
    }
    /// Tell a Host that advertises RECORDING_NOTICE that local recording started or stopped (M3 V5).
    pub fn set_recording(&self, active: bool) {
        let _ = self.commands.send(Command::SetRecording(active));
    }
    /// 本地放大后的可见区（03 §5.5）。任何线程可调，不阻塞；节流、去重、换算在会话循环里做。
    pub fn set_viewport(&self, input: crate::viewport::ViewportInput) {
        let _ = self.commands.send(Command::Viewport(input));
    }
    /// Host 最近一条有效 ViewportAck 的模式；没有 → Off。
    pub fn viewport_mode(&self) -> pb::viewport_ack::Mode {
        pb::viewport_ack::Mode::try_from(self.shared.viewport_mode.load(Ordering::Acquire))
            .unwrap_or(pb::viewport_ack::Mode::Off)
    }
    pub fn resume(&self) {
        self.control(pb::ResumeStream {});
    }
    pub fn action(&self, kind: pb::session_action::Kind, enable: bool) {
        self.control(pb::SessionAction {
            kind: kind as i32,
            enable,
        });
    }
    pub fn send_input(&self, input: InputMsg) -> Result<(), rsp_core::hid::EncodeError> {
        let _ = self.commands.send(Command::Input(input.to_frame()?));
        Ok(())
    }
    pub(crate) fn send_legacy_input(&self, input: rsp_core::input::InputEvent) {
        let _ = self.commands.send(Command::Input(input.to_frame()));
    }
    pub fn send_pointer(&self, x: f32, y: f32) {
        if x.is_finite() && y.is_finite() {
            let _ = self
                .commands
                .send(Command::Pointer(x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)));
        }
    }
    pub fn request_keyframe(&self) {
        self.request_keyframe_for(pb::keyframe_request::Reason::DecoderReset);
    }
    pub fn request_keyframe_for(&self, reason: pb::keyframe_request::Reason) {
        let _ = self.commands.send(Command::Keyframe(reason));
    }
    pub fn report_decoded(&self, decode_us: u64, latency_us: u64, queue: u32) {
        let _ = self.commands.send(Command::Decoded {
            decode_us,
            latency_us,
            queue,
        });
    }
    pub fn stats(&self) -> SessionStats {
        lock(&self.shared.stats).clone()
    }
    pub fn state(&self) -> SessionStateKind {
        *lock(&self.shared.state)
    }
    pub fn resume_token(&self) -> Option<Vec<u8>> {
        lock(&self.shared.resume_token)
            .token_for(&self.shared.host_fingerprint, Instant::now())
            .map(|token| token.to_vec())
    }
    /// Whether this connection's AuthResult resumed a retained Host session.
    pub fn resumed(&self) -> bool {
        self.shared.resumed.load(Ordering::Acquire)
    }
    pub fn handshake_timings(&self) -> HandshakeTimings {
        lock(&self.shared.timings).clone()
    }
    /// Empty when the session reused a prewarmed connection.
    pub fn dial_report(&self) -> DialReport {
        self.shared.dial_report.clone()
    }
    pub fn set_low_power(&self, low_power: bool) {
        *lock(&self.shared.low_power) = low_power;
    }
    pub fn low_power(&self) -> bool {
        *lock(&self.shared.low_power)
    }
    fn control(&self, value: impl Into<pb::Envelope>) {
        let _ = self.commands.send(Command::Control(Box::new(value.into())));
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HandshakeTimings {
    pub dial_us: u64,
    pub tls_us: u64,
    pub hello_us: u64,
    pub auth_us: u64,
    pub reused: bool,
}
/// Results of the dial that opened a session; in-session redials are not included.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DialReport {
    pub attempts: Vec<dial::AttemptLog>,
    pub primary: Option<Candidate>,
    pub transport: Option<TransportKind>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerHelloSummary {
    pub protocol: u32,
    pub host_version: String,
    pub features: u64,
    pub host_id: String,
    pub host_name: String,
}
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProbeResult {
    pub server_hello: Option<ServerHelloSummary>,
    pub peer: Option<pb::PeerInfo>,
    pub timings: HandshakeTimings,
    pub completed: bool,
}

fn make_dialer(params: &SessionParams) -> Result<Arc<dyn Dialer>, SessionError> {
    if let Some(dialer) = &params.dialer {
        return Ok(Arc::clone(dialer));
    }

    let loopback_v4 = !params.candidates.is_empty()
        && params
            .candidates
            .iter()
            .all(|candidate| candidate.addr.is_ipv4() && candidate.addr.ip().is_loopback());
    let bind = if loopback_v4 {
        SocketAddr::from(([127, 0, 0, 1], 0))
    } else if params
        .candidates
        .iter()
        .any(|candidate| candidate.addr.is_ipv6())
    {
        // Quinn's IPv6 endpoint also handles IPv4 candidates, preserving
        // Happy Eyeballs across address families.
        SocketAddr::from(([0u16; 8], 0))
    } else {
        SocketAddr::from(([0, 0, 0, 0], 0))
    };
    let endpoint = params
        .net
        .shared_quic_endpoint(bind)
        .map_err(rsp_transport::ConnError::from)?;
    Ok(Arc::new(
        dial::DefaultDialer::with_net(params.net.clone(), endpoint, &params.local_identity)
            .map_err(|error| SessionError::Identity(error.to_string()))?,
    ))
}

pub struct Session;
impl Session {
    pub async fn open(
        mut params: SessionParams,
    ) -> Result<(SessionHandle, EventStream), SessionError> {
        let pairing = if params.intent == pb::Intent::Pair {
            let credential = params
                .pairing
                .take()
                .or_else(|| params.pairing_code.take().map(PairingCredential::Code))
                .ok_or_else(|| SessionError::Protocol("pairing intent requires a code".into()))?;
            Some(match credential {
                PairingCredential::Code(code) => {
                    let code = Zeroizing::new(code);
                    SessionPairing::Spake(Box::new(
                        AppPairing::new(
                            code.as_str(),
                            params.local_identity.fingerprint(),
                            params.host_fingerprint,
                            params.device_name.clone(),
                            params.platform,
                        )
                        .map_err(|error| {
                            SessionError::Protocol(format!("pairing setup failed: {error}"))
                        })?,
                    ))
                }
                PairingCredential::File { file_id, secret } => {
                    params.required_features |= features::PAIR_FILE;
                    SessionPairing::Spake(Box::new(AppPairing::new_file(
                        file_id,
                        &secret,
                        params.local_identity.fingerprint(),
                        params.host_fingerprint,
                        params.device_name.clone(),
                        params.platform,
                    )))
                }
                PairingCredential::Password { account, password } => {
                    params.required_features |= features::PAIR_PASSWORD;
                    let account = Zeroizing::new(account);
                    SessionPairing::Password(
                        AppPasswordPairing::new(
                            &account,
                            password,
                            params.host_fingerprint,
                            params.device_name.clone(),
                            params.platform,
                        )
                        .map_err(|error| match error {
                            PairingError::InvalidCredential => {
                                SessionError::Protocol("invalid pairing credential".into())
                            }
                            _ => SessionError::Protocol(format!("pairing setup failed: {error}")),
                        })?,
                    )
                }
            })
        } else {
            None
        };
        let started = Instant::now();
        let keepalive;
        let tls_us;
        let dial_report;
        let mut backup: Option<(Conn, Candidate)> = None;
        let dialer = make_dialer(&params)?;
        let dial_context = DialContext {
            candidates: params.candidates.clone(),
            local_net: params.local_net.clone(),
            intranet_cidrs: params.intranet_cidrs.clone(),
            derp_only: params.derp_only,
            dial_options: params.dial_options.clone(),
            host_fingerprint: params.host_fingerprint,
            dialer: Arc::clone(&dialer),
        };
        let warm = params
            .prewarm
            .as_ref()
            .and_then(|p| p.take_owned(params.host_fingerprint));
        let reused = warm.is_some();
        let mut path = pb::PathKind::PathUnspecified;
        let conn = if let Some((conn, keeper)) = warm {
            keepalive = keeper;
            tls_us = 0;
            dial_report = DialReport::default();
            conn
        } else {
            let plan = dial_context.plan(params.candidates.clone());
            let outcome = dial::race(
                &plan,
                dialer.as_ref(),
                params.host_fingerprint,
                &params.dial_options,
            )
            .await
            .map_err(|failure| {
                let mut obs = params.observations.clone();
                obs.plan = plan;
                obs.local = params.local_net.clone();
                obs.failure = Some(failure);
                let code = dial::classify_failure(&obs);
                let detail = if code == ErrorCode::FingerprintMismatch {
                    obs.failure
                        .as_ref()
                        .and_then(|failure| {
                            failure.attempts.iter().find_map(|attempt| {
                                let dial::AttemptResult::FingerprintMismatch { got } =
                                    &attempt.result
                                else {
                                    return None;
                                };
                                let path = match attempt.candidate.kind.original() {
                                    PathKind::Tailnet => "tailnet",
                                    PathKind::Intranet => "intranet",
                                    PathKind::Lan => "lan",
                                    _ => "manual",
                                };
                                Some(BTreeMap::from([
                                    ("fp".into(), rsp_core::ids::fp_b64url(got)),
                                    ("addr".into(), attempt.candidate.addr.ip().to_string()),
                                    ("path".into(), path.into()),
                                ]))
                            })
                        })
                        .unwrap_or_default()
                } else {
                    BTreeMap::new()
                };
                SessionError::Dial { code, detail }
            })?;
            tls_us = outcome
                .attempts
                .iter()
                .find(|attempt| {
                    attempt.candidate == outcome.primary_path
                        && attempt.transport == Some(outcome.transport)
                })
                .and_then(|a| Some(a.finished_at_us?.saturating_sub(a.started_at_us?)))
                .unwrap_or(0);
            path = match outcome.primary_path.kind.original() {
                PathKind::Lan => pb::PathKind::Lan,
                PathKind::Intranet => pb::PathKind::Intranet,
                PathKind::Tailnet if params.derp_only => pb::PathKind::TailnetDerp,
                PathKind::Tailnet => pb::PathKind::TailnetDirect,
                _ => pb::PathKind::Manual,
            };
            dial_report = DialReport {
                attempts: outcome.attempts,
                primary: Some(outcome.primary_path),
                transport: Some(outcome.transport),
            };
            backup = outcome.backup;
            keepalive = Some(Arc::clone(&dialer));
            outcome.primary
        };
        let mut setup = SetupGuard {
            conn: Some(Connection::Current(conn.clone())),
            backup: backup.as_ref().map(|(conn, _)| conn.clone()),
        };
        // A Conn owns background tasks. Explicitly close it when setup fails.
        let control = match timeout(Duration::from_secs(5), conn.open_control()).await {
            Ok(Ok(c)) => c,
            result => {
                conn.close(CloseCode::Protocol, b"control setup failed");
                return Err(SessionError::Protocol(format!(
                    "control setup failed: {}",
                    if result.is_err() {
                        "timeout"
                    } else {
                        "transport"
                    }
                )));
            }
        };
        let transport = match conn.kind() {
            TransportKind::Quic => pb::Transport::Quic,
            TransportKind::Tcp => pb::Transport::Tcp,
        };
        let options = Options {
            start: params.start,
            display: params.display_id.unwrap_or(0),
            quality: params.quality,
            audio: params.audio,
            audio_capable: params.audio_sink.is_some(),
            intent: params.intent,
            features: params.features & (CLIENT_FEATURES | crate::files::FILES_FEATURES),
            required: params.required_features,
            device: params.device_name,
            platform: params.platform,
            token: params.resume_token,
            path,
            transport,
            legacy_quic_input: params.legacy_quic_input,
        };
        let timings = HandshakeTimings {
            dial_us: micros(started.elapsed()),
            tls_us,
            reused,
            ..Default::default()
        };
        let result = spawn(
            Connection::Current(conn),
            Control::Current(control),
            options,
            timings,
            keepalive,
            pairing,
            audio_rx::AudioRx::new(params.audio_sink),
            params.host_fingerprint,
            backup,
            Some(dial_context),
            dial_report,
        )
        .await;
        setup.conn = None;
        setup.backup = None;
        result
    }
    pub async fn probe(params: SessionParams) -> Result<ProbeResult, SessionError> {
        probe(params).await
    }
}

pub async fn probe(mut params: SessionParams) -> Result<ProbeResult, SessionError> {
    params.intent = pb::Intent::Probe;
    let (handle, mut events) = Session::open(params).await?;
    let mut result = ProbeResult::default();
    let outcome = timeout(Duration::from_secs(5), async {
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::ServerHello(h) => {
                    result.peer = h.peer;
                    result.server_hello = Some(ServerHelloSummary {
                        protocol: h.protocol,
                        host_version: h.host_version,
                        features: h.features,
                        host_id: h.host_id,
                        host_name: h.host_name,
                    });
                }
                SessionEvent::Ended {
                    kind: EndReason::Goodbye(pb::goodbye::Reason::UserClosed),
                    ..
                } if result.server_hello.is_some() => {
                    result.completed = true;
                    return Ok(());
                }
                SessionEvent::Ended { kind, .. } => return Err(SessionError::Probe(kind)),
                _ => {}
            }
        }
        Err(SessionError::Probe(EndReason::ConnectionLost))
    })
    .await;
    result.timings = handle.handshake_timings();
    handle.close();
    outcome.map_err(|_| SessionError::Protocol("probe timeout".into()))??;
    Ok(result)
}

struct Options {
    start: crate::StartParams,
    display: u32,
    quality: pb::Quality,
    audio: bool,
    audio_capable: bool,
    intent: pb::Intent,
    features: u64,
    required: u64,
    device: String,
    platform: pb::Platform,
    token: Option<Vec<u8>>,
    path: pb::PathKind,
    transport: pb::Transport,
    legacy_quic_input: bool,
}
impl Options {
    fn hello(&self) -> pb::Envelope {
        self.hello_with(
            self.token.clone().unwrap_or_default(),
            self.path,
            self.transport,
        )
    }
    /// ClientHello for another connection to the same Host (TCP-to-QUIC upgrade).
    fn hello_with(
        &self,
        resume_token: Vec<u8>,
        path: pb::PathKind,
        transport: pb::Transport,
    ) -> pb::Envelope {
        let mut features = self.features;
        if self.audio_capable {
            features |= features::AUDIO_OPUS;
        }
        pb::ClientHello {
            protocol: features::PROTOCOL_VERSION,
            app_version: env!("CARGO_PKG_VERSION").into(),
            features,
            required_features: self.required,
            device_name: self.device.clone(),
            platform: self.platform as i32,
            intent: self.intent as i32,
            resume_token,
            caps: Some(pb::ClientCaps {
                decode: vec![self.start.codec as i32],
                max_width: self.start.max_width,
                max_fps: u32::from(self.start.fps),
                audio_opus: self.audio && self.audio_capable,
                ..Default::default()
            }),
            path: path as i32,
            transport: transport as i32,
        }
        .into()
    }
}

fn path_kind_from_pb(path: pb::PathKind) -> PathKind {
    match path {
        pb::PathKind::Lan => PathKind::Lan,
        pb::PathKind::Intranet => PathKind::Intranet,
        pb::PathKind::TailnetDirect | pb::PathKind::TailnetDerp => PathKind::Tailnet,
        _ => PathKind::Manual,
    }
}

pub(crate) async fn open_legacy(
    conn: quinn::Connection,
    opts: &crate::ClientOptions,
) -> Result<(SessionHandle, EventStream), SessionError> {
    let (send, recv) = conn
        .open_bi()
        .await
        .map_err(rsp_transport::ConnError::from)?;
    let options = Options {
        start: opts.start.clone(),
        display: 0,
        quality: pb::Quality::Auto,
        audio: false,
        audio_capable: false,
        intent: pb::Intent::Control,
        features: features::FEC_RS | features::MULTI_DISPLAY,
        required: 0,
        device: opts.client_name.clone(),
        platform: pb::Platform::Unspecified,
        token: None,
        path: pb::PathKind::PathUnspecified,
        transport: pb::Transport::Quic,
        legacy_quic_input: true,
    };
    spawn(
        Connection::Legacy(conn),
        Control::legacy(send, recv),
        options,
        HandshakeTimings::default(),
        None,
        None,
        audio_rx::AudioRx::new(None),
        [0; 32],
        None,
        None,
        DialReport::default(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn spawn(
    conn: Connection,
    control: Control,
    options: Options,
    timings: HandshakeTimings,
    keepalive: Option<Arc<dyn Dialer>>,
    pairing: Option<SessionPairing>,
    audio_rx: audio_rx::AudioRx,
    host_fingerprint: [u8; 32],
    backup: Option<(Conn, Candidate)>,
    dial_context: Option<DialContext>,
    dial_report: DialReport,
) -> Result<(SessionHandle, EventStream), SessionError> {
    let mut setup = SetupGuard {
        conn: Some(conn.clone()),
        backup: backup.as_ref().map(|(conn, _)| conn.clone()),
    };
    if let Err(error) = control.send(&options.hello().encode_to_vec()).await {
        conn.close(CloseCode::Protocol, b"hello failed");
        return Err(error.into());
    }
    let mut slot = TicketSlot::new();
    if let Some(token) = options.token.as_deref() {
        let _ = slot.store(token, host_fingerprint);
    }
    let shared = Arc::new(Shared {
        state: Mutex::new(SessionStateKind::Connecting),
        stats: Mutex::new(SessionStats::default()),
        resume_token: Mutex::new(slot),
        host_fingerprint,
        resumed: AtomicBool::new(false),
        viewport_mode: AtomicI32::new(pb::viewport_ack::Mode::Off as i32),
        timings: Mutex::new(timings),
        low_power: Mutex::new(false),
        role: Mutex::new(pb::Role::Unspecified),
        peer_features: std::sync::atomic::AtomicU64::new(0),
        local_features: options.features,
        dial_report,
    });
    let (tx, rx) = mpsc::unbounded_channel();
    let (commands, command_rx) = mpsc::unbounded_channel();
    let handle = SessionHandle {
        commands,
        shared: Arc::clone(&shared),
    };
    let _ = tx.send(SessionEvent::StateChanged(SessionStateKind::Connecting));
    let now = Instant::now();
    let backup_sig = backup
        .as_ref()
        .map(|(conn, candidate)| PathSig::new(&candidate.kind, conn.kind()));
    let driver = if matches!(
        options.intent,
        pb::Intent::Control | pb::Intent::View | pb::Intent::Unspecified
    ) && matches!(conn, Connection::Current(_))
    {
        let primary_sig = match &conn {
            Connection::Current(conn) => {
                PathSig::new(&path_kind_from_pb(options.path), conn.kind())
            }
            Connection::Legacy(_) => unreachable!("path driver only uses current connections"),
        };
        Some(PathDriver::new(
            DriverConfig {
                redial: true,
                ..Default::default()
            },
            host_fingerprint,
            primary_sig,
            backup_sig.clone(),
            now,
        ))
    } else {
        None
    };
    let backup_conn_id = backup
        .as_ref()
        .map(|_| crate::recovery::failover::ConnId(2));
    let runner = Runner {
        reassembler: Reassembler::with_fps(u32::from(options.start.fps)),
        reception: feedback::Reception::default(),
        audio_rx,
        viewport: ViewportReporter::new(),
        conn,
        control: Some(control),
        backup: backup.map(|(conn, _)| conn),
        backup_conn_id,
        backup_sig,
        current_conn_id: crate::recovery::failover::ConnId(1),
        uni_reader: None,
        driver,
        options,
        events: tx,
        commands: command_rx,
        shared,
        input: None,
        clock: ClockSync::new(30),
        collector: ClientStatsCollector::starting_at(unix_micros()),
        gradient: DelayGradient::new(),
        hello_seen: false,
        server_features: 0,
        approval: None,
        ping_due: None,
        ping_schedule: PingSchedule::new(),
        feedback_due: None,
        last_key: None,
        last_good_frame: 0,
        epoch: 0,
        pointer_seq: 0,
        decode_queue: 0,
        started: Instant::now(),
        base_us: unix_micros(),
        last_error: None,
        pairing,
        _keepalive: keepalive,
        host_fingerprint,
        last_stream_config: None,
        reassembly_poll: None,
        processed_datagrams: 0,
        last_reassembly_tick_us: 0,
        dial_context,
        redial_task: None,
        backup_task: None,
        pending_conn: None,
        closing_conn: None,
        upgrade_probe: None,
        upgrade_resume: None,
        upgrade_conn: None,
        upgrade_ready: None,
        primary_replaced: false,
    };
    tokio::spawn(runner.run());
    setup.conn = None;
    setup.backup = None;
    Ok((handle, rx))
}

struct SetupGuard {
    conn: Option<Connection>,
    backup: Option<Conn>,
}
impl Drop for SetupGuard {
    fn drop(&mut self) {
        if let Some(conn) = &self.conn {
            conn.close(CloseCode::Normal, b"setup cancelled");
        }
        if let Some(conn) = &self.backup {
            conn.close(CloseCode::Normal, b"setup cancelled");
        }
    }
}

enum SessionPairing {
    Spake(Box<AppPairing>),
    Password(AppPasswordPairing),
}

struct Runner {
    conn: Connection,
    control: Option<Control>,
    backup: Option<Conn>,
    backup_conn_id: Option<crate::recovery::failover::ConnId>,
    backup_sig: Option<PathSig>,
    current_conn_id: crate::recovery::failover::ConnId,
    uni_reader: Option<tokio::task::JoinHandle<()>>,
    driver: Option<PathDriver>,
    options: Options,
    events: mpsc::UnboundedSender<SessionEvent>,
    commands: mpsc::UnboundedReceiver<Command>,
    shared: Arc<Shared>,
    input: Option<Input>,
    clock: ClockSync,
    collector: ClientStatsCollector,
    gradient: DelayGradient,
    reassembler: Reassembler,
    reception: feedback::Reception,
    audio_rx: audio_rx::AudioRx,
    viewport: ViewportReporter,
    hello_seen: bool,
    server_features: u64,
    approval: Option<Instant>,
    ping_due: Option<Instant>,
    ping_schedule: PingSchedule,
    feedback_due: Option<Instant>,
    last_key: Option<Instant>,
    last_good_frame: u32,
    epoch: u32,
    pointer_seq: u32,
    decode_queue: u32,
    started: Instant,
    base_us: u64,
    last_error: Option<ErrorCode>,
    _keepalive: Option<Arc<dyn Dialer>>,
    pairing: Option<SessionPairing>,
    host_fingerprint: [u8; 32],
    last_stream_config: Option<pb::StreamConfig>,
    reassembly_poll: Option<ReassemblyPoll>,
    processed_datagrams: u64,
    last_reassembly_tick_us: u64,
    dial_context: Option<DialContext>,
    redial_task: Option<redial::DialTask>,
    backup_task: Option<redial::DialTask>,
    pending_conn: Option<(crate::recovery::failover::ConnId, Conn, PathSig)>,
    /// A connection closed by a failover action already has its state-machine
    /// transition applied.  Ignore the later transport `closed()` notification
    /// so it cannot be counted as a second redial failure.
    closing_conn: Option<crate::recovery::failover::ConnId>,
    /// QUIC-only race started by a TCP primary's upgrade probe.
    upgrade_probe: Option<redial::DialTask>,
    /// Reads the Host's answer to the upgrade resume on the candidate.
    upgrade_resume: Option<redial::ResumeTask>,
    upgrade_conn: Option<(crate::recovery::failover::ConnId, Conn, PathSig)>,
    /// The candidate's answer while the driver decides whether to commit.
    upgrade_ready: Option<redial::Resumed>,
    /// The Host replaced the TCP primary before the upgrade committed.
    primary_replaced: bool,
}

/// Drain the finite prefix that was queued at a timer poll, not an endlessly
/// replenished receive queue. Expiry uses the poll's time, so shards arriving
/// while this prefix is processed cannot prematurely age newer frame slots.
struct ReassemblyPoll {
    at_us: u64,
    packets_left: u64,
    bytes_left: usize,
}

fn received_datagrams(conn: &Connection) -> Option<u64> {
    match conn {
        Connection::Current(conn) => conn.as_quic().map(|q| q.stats().frame_rx.datagram),
        Connection::Legacy(conn) => Some(conn.stats().frame_rx.datagram),
    }
}

impl ReassemblyPoll {
    fn new(conn: &Connection, processed: u64, at_us: u64) -> Self {
        Self {
            at_us,
            // TCP's receive channel holds at most 256 datagrams. QUIC's
            // lifetime receive count minus processed media is conservative:
            // filtered heartbeats/unknown kinds and buffer evictions can only
            // make us drain more, never skip a previously arrived shard.
            packets_left: received_datagrams(conn)
                .map_or(256, |received| received.saturating_sub(processed)),
            // Also bound that conservative count after drops/filtered packets.
            // Current QUIC buffers 4 MiB; retained M0 QUIC buffers 8 MiB.
            // Consuming this much FIFO data necessarily passes the old tail.
            // Quinn also accounts for per-packet metadata, including empty
            // legacy datagrams (charged at least one byte below).
            bytes_left: match conn {
                Connection::Current(conn) if conn.kind() == TransportKind::Tcp => usize::MAX,
                Connection::Current(_) => 4 << 20,
                Connection::Legacy(_) => 8 << 20,
            },
        }
    }

    fn consumed(&mut self, bytes: usize) {
        self.packets_left = self.packets_left.saturating_sub(1);
        self.bytes_left = self.bytes_left.saturating_sub(bytes.max(1));
    }

    fn drained(&self) -> bool {
        self.packets_left == 0 || self.bytes_left == 0
    }
}

impl Runner {
    fn emit(&self, event: SessionEvent) {
        let _ = self.events.send(event);
    }
    fn emit_error(
        &self,
        code: ErrorCode,
        message: String,
        detail: BTreeMap<String, String>,
        retry_after_s: u32,
    ) {
        let retry_after_s = (retry_after_s != 0).then_some(retry_after_s);
        if !detail.is_empty() || retry_after_s.is_some() {
            self.emit(SessionEvent::ErrorDetail {
                code,
                detail,
                retry_after_s,
            });
        }
        self.emit(SessionEvent::Error { code, message });
    }
    fn now_us(&self) -> u64 {
        self.base_us.saturating_add(micros(self.started.elapsed()))
    }
    fn state(&self) -> SessionStateKind {
        *lock(&self.shared.state)
    }
    fn advance(&self, event: StateTransition) -> Result<(), SessionError> {
        let next =
            transition(self.state(), event).map_err(|e| SessionError::Protocol(e.to_string()))?;
        *lock(&self.shared.state) = next;
        self.emit(SessionEvent::StateChanged(next));
        Ok(())
    }
    async fn send(&self, message: impl Into<pb::Envelope>) -> Result<(), SessionError> {
        self.control
            .as_ref()
            .ok_or(ConnError::Closed)?
            .send(&message.into().encode_to_vec())
            .await?;
        Ok(())
    }
    async fn run(mut self) {
        self.audio_rx.reset();
        self.restart_uni_reader();
        let _ = self.advance(StateTransition::Connected);
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        self.last_reassembly_tick_us = self.now_us();
        let mut receive_batch = 0_u32;
        let end = loop {
            if self
                .reassembly_poll
                .as_ref()
                .is_some_and(ReassemblyPoll::drained)
            {
                if let Some(poll) = self.reassembly_poll.take() {
                    self.tick_reassembly(poll.at_us).await;
                }
            }
            if receive_batch >= 16 {
                tokio::task::yield_now().await;
                receive_batch = 0;
            }
            let received_before_poll = received_datagrams(&self.conn);
            let path_deadline = self.driver.as_ref().and_then(PathDriver::next_deadline);
            let backup = self.backup.clone();
            let backup_conn_id = self.backup_conn_id;
            let backup_present = backup.is_some();
            tokio::select! {
                biased;
                command = self.commands.recv() => match command {
                    Some(Command::Close) | None => {
                        self.cancel_path_tasks().await;
                        if self.state() != SessionStateKind::Reconnecting {
                            let _ = self.send(pb::Goodbye {
                                reason: pb::goodbye::Reason::UserClosed as i32,
                                message: String::new(),
                            }).await;
                            self.drain().await;
                        }
                        break EndReason::Goodbye(pb::goodbye::Reason::UserClosed);
                    }
                    Some(Command::CloseWith(reason)) => {
                        self.cancel_path_tasks().await;
                        if self.state() != SessionStateKind::Reconnecting {
                            let _ = self.send(pb::Goodbye {
                                reason: reason as i32,
                                message: String::new(),
                            }).await;
                            self.drain().await;
                        }
                        break EndReason::Goodbye(reason);
                    }
                    Some(command) => if let Err(error) = self.command(command).await {
                        tracing::debug!(%error, "session command failed");
                        if matches!(self.state(), SessionStateKind::Streaming | SessionStateKind::Reconnecting) {
                            match self.transport_failure(self.current_conn_id).await {
                                Ok(Some(end)) => break end,
                                Ok(None) => {},
                                Err(_) => break EndReason::ConnectionLost,
                            }
                        } else {
                            break EndReason::ConnectionLost;
                        }
                    },
                },
                _ = tick.tick() => {
                    let mut reassembly_tick = self.last_reassembly_tick_us;
                    let tick_result = self.tick(&mut reassembly_tick).await;
                    self.last_reassembly_tick_us = reassembly_tick;
                    match tick_result {
                        Ok(Some(end)) => break end,
                        Ok(None) => {},
                        Err(error) => {
                            tracing::debug!(%error, "session tick failed");
                            match self.transport_failure(self.current_conn_id).await {
                                Ok(Some(end)) => break end,
                                Ok(None) => {},
                                Err(_) => break EndReason::ConnectionLost,
                            }
                        }
                    }
                    if let Ok(Some(end)) = self.poll_path().await {
                        break end;
                    }
                    if self.reassembly_poll.is_none() {
                        self.reassembly_poll = Some(ReassemblyPoll::new(
                            &self.conn,
                            self.processed_datagrams,
                            self.now_us(),
                        ));
                    }
                },
                _ = async {
                    if let Some(deadline) = path_deadline {
                        tokio::time::sleep_until(deadline).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                }, if path_deadline.is_some() => {
                    if let Ok(Some(end)) = self.poll_path().await {
                        break end;
                    }
                },
                redial_result = async {
                    match self.redial_task.as_mut() {
                        Some(task) => task.recv().await,
                        None => std::future::pending::<redial::DialResult>().await,
                    }
                }, if self.redial_task.is_some() => {
                    self.redial_task.take();
                    match self.redial_finished(redial_result).await {
                        Ok(Some(end)) => break end,
                        Ok(None) => {},
                        Err(_) => break EndReason::ConnectionLost,
                    }
                },
                backup_result = async {
                    match self.backup_task.as_mut() {
                        Some(task) => task.recv().await,
                        None => std::future::pending::<redial::DialResult>().await,
                    }
                }, if self.backup_task.is_some() => {
                    self.backup_task.take();
                    self.backup_finished(backup_result).await;
                },
                probe_result = async {
                    match self.upgrade_probe.as_mut() {
                        Some(task) => task.recv().await,
                        None => std::future::pending::<redial::DialResult>().await,
                    }
                }, if self.upgrade_probe.is_some() => {
                    self.upgrade_probe.take();
                    match self.upgrade_probed(probe_result).await {
                        Ok(Some(end)) => break end,
                        Ok(None) => {},
                        Err(_) => break EndReason::ConnectionLost,
                    }
                },
                resume_result = async {
                    match self.upgrade_resume.as_mut() {
                        Some(task) => task.recv().await,
                        None => std::future::pending::<redial::ResumeResult>().await,
                    }
                }, if self.upgrade_resume.is_some() => {
                    self.upgrade_resume.take();
                    match self.upgrade_resumed(resume_result).await {
                        Ok(Some(end)) => break end,
                        Ok(None) => {},
                        Err(_) => break EndReason::ConnectionLost,
                    }
                },
                message = async {
                    if let Some(control) = self.control.as_ref() {
                        control.recv().await
                    } else {
                        Err(ConnError::Closed)
                    }
                }, if self.closing_conn.is_none() => match message {
                    Ok(Some(bytes)) => {
                        self.path_inbound();
                        match pb::Envelope::decode(bytes) {
                            Ok(env) => match self.message(env).await {
                                Ok(Some(end)) => break end,
                                Ok(None) => {},
                                Err(error) => {
                                    tracing::debug!(%error, "session protocol error");
                                    break EndReason::Protocol;
                                }
                            },
                            Err(_) => break EndReason::Protocol,
                        }
                    }
                    Ok(None) | Err(_) => {
                        if self.primary_replaced_by_upgrade() {
                            continue;
                        }
                        match self.transport_end().await {
                            EndReason::ConnectionLost
                                if self.driver.is_some()
                                    && matches!(
                                        self.state(),
                                        SessionStateKind::Streaming
                                            | SessionStateKind::Reconnecting
                                    ) => {
                                match self.transport_failure(self.current_conn_id).await {
                                    Ok(Some(end)) => break end,
                                    Ok(None) => {},
                                    Err(_) => break EndReason::ConnectionLost,
                                }
                            }
                            EndReason::Close(code)
                                if self.driver.is_some()
                                    && self.state() == SessionStateKind::Reconnecting =>
                            {
                                let actions = self.conn_closed_actions(
                                    self.current_conn_id,
                                    Some(code),
                                );
                                match self.apply_path_actions(actions).await {
                                    Ok(Some(end)) => break end,
                                    Ok(None) => {},
                                    Err(_) => break EndReason::ConnectionLost,
                                }
                            }
                            end => break end,
                        }
                    }
                },
                _ = self.events.closed() => {
                    let _ = self.send(pb::Goodbye {
                        reason: pb::goodbye::Reason::UserClosed as i32,
                        message: String::new(),
                    }).await;
                    self.drain().await;
                    break EndReason::Goodbye(pb::goodbye::Reason::UserClosed);
                },
                data = self.conn.datagram(), if self.state() == SessionStateKind::Streaming
                    && self.closing_conn.is_none() => {
                    receive_batch += 1;
                    self.processed_datagrams = self.processed_datagrams.saturating_add(1);
                    if let (Some(poll), Ok(Datagram::Audio(bytes) | Datagram::Video(bytes))) =
                        (&mut self.reassembly_poll, &data)
                    {
                        poll.consumed(bytes.len());
                    }
                    self.path_inbound();
                    match data {
                        Ok(Datagram::Audio(bytes)) => self.audio_rx.on_datagram(&bytes, unix_micros()),
                        Ok(Datagram::Video(bytes)) => {
                            let now = self.now_us();
                            match self.reassembler.push(&bytes, now) {
                                Ok(events) => {
                                    self.reception.packet(&bytes, now);
                                    self.video_events(events).await;
                                },
                                Err(error) => tracing::debug!(%error, "invalid video datagram"),
                            }
                        },
                        Err(_) => match self.transport_failure(self.current_conn_id).await {
                            Ok(Some(end)) => break end,
                            Ok(None) => {},
                            Err(_) => break EndReason::ConnectionLost,
                        },
                    }
                },
                _ = std::future::ready(()), if self.reassembly_poll.is_some() => {
                    if self.state() == SessionStateKind::Streaming {
                        if let Some(received) = received_before_poll {
                            self.processed_datagrams = self.processed_datagrams.max(received);
                        }
                    }
                    if let Some(poll) = self.reassembly_poll.take() {
                        self.tick_reassembly(poll.at_us).await;
                    }
                },
                reason = self.conn.closed(), if self.closing_conn.is_none() => {
                    if matches!(reason.code, None | Some(3)) && self.primary_replaced_by_upgrade() {
                        continue;
                    }
                    if self.driver.is_some()
                        && self.state() == SessionStateKind::Reconnecting
                    {
                        let actions = self.conn_closed_actions(self.current_conn_id, reason.code);
                        match self.apply_path_actions(actions).await {
                            Ok(Some(end)) => break end,
                            Ok(None) => {},
                            Err(_) => break EndReason::ConnectionLost,
                        }
                    } else if reason.code.is_none()
                        && self.driver.is_some()
                        && self.state() == SessionStateKind::Streaming
                    {
                        match self.transport_failure(self.current_conn_id).await {
                            Ok(Some(end)) => break end,
                            Ok(None) => {},
                            Err(_) => break EndReason::ConnectionLost,
                        }
                    } else {
                        break reason.code.map(EndReason::Close).unwrap_or(EndReason::ConnectionLost);
                    }
                },
                reason = async move {
                    match backup {
                        Some(conn) => Some(conn.closed().await),
                        None => std::future::pending::<Option<rsp_transport::CloseReason>>().await,
                    }
                }, if backup_present => {
                    self.backup = None;
                    self.backup_conn_id = None;
                    self.backup_sig = None;
                    let Some(reason) = reason else {
                        continue;
                    };
                    let actions = self.conn_closed_actions(
                        backup_conn_id.unwrap_or(crate::recovery::failover::ConnId(2)),
                        reason.code,
                    );
                    match self.apply_path_actions(actions).await {
                        Ok(Some(end)) => break end,
                        Ok(None) => {},
                        Err(_) => break EndReason::ConnectionLost,
                    }
                },
            }
        };
        let error = match end {
            EndReason::Goodbye(reason) => goodbye_reason_error(reason),
            EndReason::Close(code) => self.last_error.or(close_code_to_error(code)),
            EndReason::Error(code) => Some(code),
            EndReason::Protocol | EndReason::ConnectionLost => Some(ErrorCode::Other),
        };
        if end == EndReason::ConnectionLost && self.state() == SessionStateKind::Streaming {
            let _ = self.advance(StateTransition::Disconnected);
        }
        let _ = self.advance(StateTransition::End(error.unwrap_or(ErrorCode::Other)));
        let close_code = match end {
            EndReason::Protocol => CloseCode::Protocol,
            EndReason::Error(_) => CloseCode::Auth,
            _ => CloseCode::Normal,
        };
        self.conn.close(close_code, b"session ended");
        self.cancel_path_tasks().await;
        if let Some(backup) = self.backup.take() {
            backup.close(CloseCode::Normal, b"backup dropped");
        }
        if let Some(uni) = self.uni_reader.take() {
            uni.abort();
            let _ = uni.await;
        }
        let reason = match end {
            EndReason::Goodbye(r) => r.as_str_name().to_owned(),
            EndReason::ConnectionLost => "ConnectionLost".into(),
            _ => format!("{end:?}"),
        };
        self.emit(SessionEvent::Ended {
            code: error,
            reason,
            kind: end,
        });
    }
    async fn drain(&self) {
        if let Some(control) = &self.control {
            control.drain().await;
        }
        // Current ControlStream has no acknowledgement API. Wait for the Host
        // to close after reading Goodbye before issuing the local QUIC close.
        let _ = timeout(Duration::from_millis(500), self.conn.closed()).await;
    }
    async fn transport_end(&self) -> EndReason {
        match timeout(Duration::from_millis(500), self.conn.closed()).await {
            Ok(reason) => reason
                .code
                .map(EndReason::Close)
                .unwrap_or(EndReason::ConnectionLost),
            Err(_) => EndReason::ConnectionLost,
        }
    }
    async fn command(&mut self, command: Command) -> Result<(), SessionError> {
        match command {
            Command::Viewport(input) => self.viewport.on_input(self.now_us(), input),
            // Commands requiring authorization are dropped while awaiting approval.
            _ if self.state() != SessionStateKind::Streaming => return Ok(()),
            Command::SetAudio(enabled) => {
                self.options.audio = enabled;
                if self.server_features & features::AUDIO_OPUS != 0 {
                    self.send(pb::SetAudio { enabled }).await?;
                } else {
                    tracing::debug!("Host does not support AUDIO_OPUS");
                }
            }
            Command::SetRecording(active) => {
                if self.server_features & features::RECORDING_NOTICE != 0 {
                    self.send(pb::RecordingNotice { active }).await?;
                } else {
                    tracing::debug!("Host does not support RECORDING_NOTICE");
                }
            }
            Command::Control(env) => {
                let is_pause = matches!(env.msg.as_ref(), Some(pb::envelope::Msg::PauseStream(_)));
                let is_resume =
                    matches!(env.msg.as_ref(), Some(pb::envelope::Msg::ResumeStream(_)));
                self.send(*env).await?;
                if is_pause {
                    self.cancel_backup_task().await;
                    let dropped = self
                        .driver
                        .as_mut()
                        .is_some_and(|driver| !driver.suspend().is_empty());
                    if dropped {
                        // suspend() only drops an upgrade. A primary the Host
                        // already replaced fails its first Ping after resume.
                        self.cancel_upgrade().await;
                    }
                } else if is_resume {
                    if let Some(driver) = &mut self.driver {
                        driver.resume(Instant::now());
                    }
                }
            }
            Command::OpenFile(reply) => {
                let conn = self.conn.clone();
                tokio::spawn(async move {
                    let result = match conn {
                        Connection::Current(conn) => conn.open_uni(StreamKind::File).await,
                        Connection::Legacy(_) => Err(ConnError::Unsupported(
                            "file streams require the current transport API",
                        )),
                    };
                    let _ = reply.send(result);
                });
            }
            Command::ClipboardSend(item) => {
                if self.options.features & features::CLIPBOARD_TEXT == 0 {
                    tracing::debug!(
                        kind = %item.kind,
                        len = item.data.len(),
                        "clipboard text feature is disabled locally"
                    );
                    return Ok(());
                }
                let policy = ClipboardPolicy {
                    peer_features: self.server_features,
                    role: *lock(&self.shared.role),
                    to_phone: true,
                    from_phone: true,
                };
                let conn = match &self.conn {
                    Connection::Current(conn) => conn.clone(),
                    Connection::Legacy(_) => {
                        tracing::debug!(
                            kind = %item.kind,
                            len = item.data.len(),
                            "clipboard stream unsupported on legacy transport"
                        );
                        return Ok(());
                    }
                };
                let kind = item.kind;
                let len = item.data.len();
                tokio::spawn(async move {
                    match conn.open_uni(StreamKind::Clipboard).await {
                        Ok(mut send) => {
                            if let Err(error) =
                                crate::clipboard::send(&mut send, &item, &policy).await
                            {
                                tracing::debug!(%error, kind = %kind, len, "clipboard stream send failed");
                                let _ = send.reset(1);
                            }
                        }
                        Err(error) => {
                            tracing::debug!(%error, kind = %kind, len, "clipboard stream open failed");
                        }
                    }
                });
            }
            Command::Input(bytes) => {
                if let Some(driver) = &mut self.driver {
                    if matches!(
                        driver.input_verdict(),
                        crate::recovery::failover::InputVerdict::Drop
                    ) {
                        return Ok(());
                    }
                }
                if self.input.is_none() {
                    self.input = Some(self.conn.open_input(self.options.legacy_quic_input).await?);
                }
                if let Some(input) = &mut self.input {
                    input.send(&bytes).await?;
                }
            }
            Command::Pointer(x, y) => {
                if let Some(driver) = &mut self.driver {
                    if matches!(
                        driver.input_verdict(),
                        crate::recovery::failover::InputVerdict::Drop
                    ) {
                        return Ok(());
                    }
                }
                self.pointer_seq = self.pointer_seq.wrapping_add(1);
                let wire = PointerDatagram {
                    seq: self.pointer_seq,
                    display: self.options.display as u8,
                    visible: true,
                    x,
                    y,
                }
                .encode();
                self.conn.pointer(&wire)?;
            }
            Command::Keyframe(reason) => self.keyframe(reason, self.last_good_frame).await?,
            Command::Decoded {
                decode_us,
                latency_us,
                queue,
            } => {
                self.decode_queue = queue;
                self.collector
                    .on_decoded(self.now_us(), decode_us, latency_us);
            }
            Command::Close => {}
            Command::CloseWith(_) => {}
        }
        Ok(())
    }
    async fn message(&mut self, envelope: pb::Envelope) -> Result<Option<EndReason>, SessionError> {
        let Some(message) = envelope.msg else {
            return Ok(None);
        };
        match message {
            pb::envelope::Msg::ServerHello(h) => {
                if self.hello_seen
                    || !matches!(
                        self.state(),
                        SessionStateKind::Authenticating | SessionStateKind::Reconnecting
                    )
                {
                    return Err(SessionError::Protocol("unexpected ServerHello".into()));
                }
                let mismatch = if h.protocol > features::PROTOCOL_VERSION {
                    Some(ErrorCode::AppTooOld)
                } else if h.protocol < features::PROTOCOL_VERSION {
                    Some(ErrorCode::HostTooOld)
                } else if h.required_features & !self.options.features != 0 {
                    Some(ErrorCode::AppTooOld)
                } else if self.options.required & !h.features != 0 {
                    Some(ErrorCode::HostTooOld)
                } else {
                    None
                };
                if let Some(code) = mismatch {
                    self.emit_error(
                        code,
                        "incompatible protocol or required features".into(),
                        BTreeMap::from([("host_version".into(), h.host_version)]),
                        0,
                    );
                    return Ok(Some(EndReason::Error(code)));
                }
                self.hello_seen = true;
                self.server_features = h.features;
                self.shared
                    .peer_features
                    .store(h.features, Ordering::Release);
                lock(&self.shared.timings).hello_us = micros(self.started.elapsed());
                let peer_path = h
                    .peer
                    .as_ref()
                    .map(|peer| {
                        pb::PathKind::try_from(peer.path).unwrap_or(pb::PathKind::PathUnspecified)
                    })
                    .unwrap_or(self.options.path);
                self.collector.set_path(peer_path, self.options.transport);
                self.emit(SessionEvent::Displays(h.displays.clone()));
                self.emit(SessionEvent::ServerHello(h));
                if self.options.intent == pb::Intent::Pair {
                    let pairing = self
                        .pairing
                        .as_mut()
                        .ok_or_else(|| SessionError::Protocol("pairing state missing".into()))?;
                    match pairing {
                        SessionPairing::Spake(pairing) => {
                            let start = pairing.try_start().map_err(|error| {
                                SessionError::Protocol(format!("pairing start failed: {error}"))
                            })?;
                            self.send(start).await?;
                        }
                        SessionPairing::Password(pairing) => {
                            let start = pairing.try_start().map_err(|error| {
                                SessionError::Protocol(format!("pairing start failed: {error}"))
                            })?;
                            let mut message: pb::Envelope = start.into();
                            let payload = Zeroizing::new(message.encode_to_vec());
                            // Erase the protobuf credentials before the first await, including
                            // when sending fails or this future is cancelled. The wire buffer
                            // is erased on drop after send completes or fails.
                            if let Some(pb::envelope::Msg::PairPassword(start)) = &mut message.msg {
                                start.account.zeroize();
                                start.password.zeroize();
                            }
                            self.control
                                .as_ref()
                                .ok_or(ConnError::Closed)?
                                .send(&payload)
                                .await?;
                        }
                    }
                }
            }
            pb::envelope::Msg::AuthResult(auth) => {
                // A rejected Host handshake can deny before disclosing its hello.
                if !self.hello_seen && auth.status != pb::auth_result::Status::Denied as i32 {
                    return Err(SessionError::Protocol(
                        "AuthResult before ServerHello".into(),
                    ));
                }
                if self.state() == SessionStateKind::Reconnecting {
                    let actions = if let Some(driver) = &mut self.driver {
                        let mut slot = lock(&self.shared.resume_token);
                        driver.on_auth_result(
                            self.current_conn_id,
                            &auth,
                            &mut slot,
                            Instant::now(),
                        )
                    } else {
                        Vec::new()
                    };
                    if !auth.session_token.is_empty() {
                        self.conn.token(auth.session_token.clone())?;
                    }
                    self.options.token =
                        (!auth.session_token.is_empty()).then_some(auth.session_token.clone());
                    self.input = None;
                    if pb::auth_result::Status::try_from(auth.status)
                        == Ok(pb::auth_result::Status::Ok)
                    {
                        self.emit(SessionEvent::Role(
                            pb::Role::try_from(auth.role).unwrap_or(pb::Role::Unspecified),
                        ));
                    }
                    return self.apply_path_actions(actions).await;
                }
                match pb::auth_result::Status::try_from(auth.status) {
                    Ok(pb::auth_result::Status::PendingApproval) => {
                        self.advance(StateTransition::ApprovalPending)?;
                        self.approval = Some(
                            Instant::now()
                                + Duration::from_secs(u64::from(auth.approval_timeout_s) + 5),
                        );
                    }
                    Ok(pb::auth_result::Status::Ok) => {
                        let edge = if self.state() == SessionStateKind::PendingApproval {
                            StateTransition::ApprovalGranted
                        } else {
                            StateTransition::Authenticated
                        };
                        self.shared.resumed.store(auth.resumed, Ordering::Release);
                        self.approval = None;
                        if !auth.session_token.is_empty() {
                            self.conn.token(auth.session_token.clone())?;
                        }
                        let _ = lock(&self.shared.resume_token)
                            .store(&auth.session_token, self.host_fingerprint);
                        self.advance(edge)?;
                        if edge == StateTransition::ApprovalGranted
                            && self
                                .driver
                                .as_ref()
                                .is_some_and(|driver| driver.primary().0 != self.current_conn_id.0)
                        {
                            let primary = PathSig::new(
                                &path_kind_from_pb(self.options.path),
                                match &self.conn {
                                    Connection::Current(conn) => conn.kind(),
                                    Connection::Legacy(_) => TransportKind::Quic,
                                },
                            );
                            self.driver = Some(PathDriver::new(
                                DriverConfig {
                                    redial: true,
                                    ..Default::default()
                                },
                                self.host_fingerprint,
                                primary,
                                None,
                                Instant::now(),
                            ));
                        }
                        // Approval can complete before the Host starts media.
                        // Start that path's monitor on its first StreamConfig.
                        if edge != StateTransition::ApprovalGranted {
                            if let Some(driver) = &mut self.driver {
                                driver.on_streaming(Instant::now());
                            }
                        }
                        {
                            let mut timings = lock(&self.shared.timings);
                            timings.auth_us =
                                micros(self.started.elapsed()).saturating_sub(timings.hello_us);
                        }
                        let role = pb::Role::try_from(auth.role).unwrap_or(pb::Role::Unspecified);
                        *lock(&self.shared.role) = role;
                        self.emit(SessionEvent::Role(role));
                        if self.options.intent != pb::Intent::Probe {
                            if self.options.intent != pb::Intent::Files {
                                self.send(pb::StartStream {
                                    display_id: self.options.display,
                                    codec: self.options.start.codec as i32,
                                    max_width: self.options.start.max_width,
                                    fps: u32::from(self.options.start.fps),
                                    bitrate_kbps: self.options.start.bitrate_kbps,
                                    quality: self.options.quality as i32,
                                    ..Default::default()
                                })
                                .await?;
                                self.feedback_due =
                                    Some(Instant::now() + Duration::from_millis(200));
                            }
                            self.ping_due = Some(Instant::now() + Duration::from_millis(60));
                            self.ping_schedule =
                                PingSchedule::fast_starting_at(self.now_us() + 60_000);
                        }
                    }
                    Ok(pb::auth_result::Status::Denied) => {
                        let error = auth.error.unwrap_or_else(|| pb::ErrorInfo {
                            message: "authentication denied".into(),
                            ..Default::default()
                        });
                        let code = error_code(error.code).unwrap_or(ErrorCode::ApprovalDenied);
                        self.emit_error(
                            code,
                            error.message,
                            error.detail.into_iter().collect(),
                            error.retry_after_s,
                        );
                        return Ok(Some(EndReason::Error(code)));
                    }
                    _ => {
                        return Err(SessionError::Protocol(
                            "unknown authorization status".into(),
                        ))
                    }
                }
            }
            pb::envelope::Msg::PairSpake(spake) => {
                let Some(SessionPairing::Spake(pairing)) = self.pairing.as_mut() else {
                    return Err(SessionError::Protocol("unexpected PairSpake".into()));
                };
                let (reply, confirm) = pairing.on_spake(spake).map_err(|error| {
                    SessionError::Protocol(format!("pairing SPAKE failed: {error}"))
                })?;
                self.send(reply).await?;
                self.send(confirm).await?;
            }
            pb::envelope::Msg::PairConfirm(confirm) => {
                let Some(SessionPairing::Spake(pairing)) = self.pairing.as_mut() else {
                    return Err(SessionError::Protocol("unexpected PairConfirm".into()));
                };
                pairing.on_confirm(confirm).map_err(|error| {
                    SessionError::Protocol(format!("pairing confirmation failed: {error}"))
                })?;
            }
            pb::envelope::Msg::PairResult(result) => {
                let Some(pairing) = self.pairing.as_mut() else {
                    return Err(SessionError::Protocol("unexpected PairResult".into()));
                };
                let outcome = match pairing {
                    SessionPairing::Spake(pairing) => pairing.on_result(result.clone()),
                    SessionPairing::Password(pairing) => pairing.on_result(result.clone()),
                };
                if let PairingOutcome::Failed(error) = outcome {
                    return Err(SessionError::Protocol(format!(
                        "invalid pairing result: {error}"
                    )));
                }
                self.emit(SessionEvent::PairResult(result));
                match outcome {
                    PairingOutcome::WaitingLocal => {}
                    PairingOutcome::Paired { .. } => {
                        return Ok(Some(EndReason::Goodbye(pb::goodbye::Reason::UserClosed)))
                    }
                    PairingOutcome::Denied
                    | PairingOutcome::BadCode { .. }
                    | PairingOutcome::Expired
                    | PairingOutcome::Locked
                    | PairingOutcome::MethodDisabled
                    | PairingOutcome::DisabledOnPath
                    | PairingOutcome::Failed(_) => {
                        return Ok(Some(EndReason::Error(ErrorCode::PolicyDenied)))
                    }
                }
            }
            pb::envelope::Msg::SessionState(state) => {
                self.require_streaming()?;
                *lock(&self.shared.role) =
                    pb::Role::try_from(state.your_role).unwrap_or(pb::Role::Unspecified);
                self.emit(SessionEvent::Role(
                    pb::Role::try_from(state.your_role).unwrap_or(pb::Role::Unspecified),
                ));
                self.emit(SessionEvent::LocalUse(state.input_paused_local));
                self.emit(SessionEvent::SessionState(state));
            }
            pb::envelope::Msg::StreamConfig(config) => {
                self.require_streaming()?;
                if let Some(driver) = &mut self.driver {
                    driver.on_streaming(Instant::now());
                }
                if self.epoch != config.epoch {
                    self.last_good_frame = 0;
                }
                self.epoch = config.epoch;
                self.options.display = config.display_id;
                self.reassembler.set_fps(config.fps);
                self.last_stream_config = Some(config);
                self.viewport.on_stream_config(
                    config.display_id,
                    config.display_width_px,
                    config.display_height_px,
                );
                self.emit(SessionEvent::Cursor(CursorEvent {
                    x: config.cursor_x,
                    y: config.cursor_y,
                    visible: config.cursor_visible,
                }));
                self.emit(SessionEvent::StreamConfig(config));
            }
            pb::envelope::Msg::ViewportAck(ack) => {
                if self.state() != SessionStateKind::Streaming {
                    return Ok(None);
                }
                self.viewport.on_ack(&ack);
                self.shared
                    .viewport_mode
                    .store(self.viewport.host_mode() as i32, Ordering::Release);
            }
            pb::envelope::Msg::Pong(pong) => {
                self.path_pong(pong.t1_us);
                let sample = self.clock.add(pong.t1_us, pong.t2_us, unix_micros());
                self.collector.on_rtt(sample.rtt_us);
                let best = self.clock.best();
                {
                    let mut stats = lock(&self.shared.stats);
                    stats.rtt_us = Some(sample.rtt_us);
                    stats.offset_us = best.map(|s| s.offset_us);
                }
                self.emit(SessionEvent::ClockSync {
                    rtt_us: sample.rtt_us,
                    offset_us: sample.offset_us,
                });
            }
            pb::envelope::Msg::Ping(ping) => {
                self.send(pb::Pong {
                    t1_us: ping.t1_us,
                    t2_us: unix_micros(),
                })
                .await?
            }
            pb::envelope::Msg::VirtualDisplayResult(result) => {
                self.emit(SessionEvent::VirtualDisplay(result))
            }
            pb::envelope::Msg::DisplayList(list) => {
                self.emit(SessionEvent::Displays(list.displays))
            }
            pb::envelope::Msg::ControlRequest(request) => {
                self.emit(SessionEvent::ControlRequested(request))
            }
            pb::envelope::Msg::IdleWarning(warning) => {
                self.emit(SessionEvent::IdleWarning(warning))
            }
            pb::envelope::Msg::LocalActivity(activity) => {
                self.emit(SessionEvent::LocalUse(activity.active))
            }
            pb::envelope::Msg::RateUpdate(update) => self.emit(SessionEvent::RateUpdate(update)),
            pb::envelope::Msg::HostStats(stats) => self.emit(SessionEvent::HostStats(stats)),
            pb::envelope::Msg::FileListing(message) => {
                self.emit(SessionEvent::FileListing(message))
            }
            pb::envelope::Msg::FileStatus(message) => self.emit(SessionEvent::FileStatus(message)),
            pb::envelope::Msg::FileCancel(message) => self.emit(SessionEvent::FileCancel(message)),
            pb::envelope::Msg::FileList(_)
            | pb::envelope::Msg::FileRequest(_)
            | pb::envelope::Msg::FileQuery(_) => {
                tracing::debug!("ignoring unexpected Host file request");
            }
            pb::envelope::Msg::Error(error) => {
                let code = error_code(error.code).unwrap_or(ErrorCode::Other);
                self.last_error = Some(code);
                self.emit_error(
                    code,
                    error.message,
                    error.detail.into_iter().collect(),
                    error.retry_after_s,
                );
            }
            pb::envelope::Msg::Goodbye(goodbye) => {
                let reason = pb::goodbye::Reason::try_from(goodbye.reason)
                    .unwrap_or(pb::goodbye::Reason::Unspecified);
                if reason == pb::goodbye::Reason::Replaced && self.primary_replaced_by_upgrade() {
                    return Ok(None);
                }
                return Ok(Some(EndReason::Goodbye(reason)));
            }
            _ => {}
        }
        Ok(None)
    }
    fn require_streaming(&self) -> Result<(), SessionError> {
        if self.state() == SessionStateKind::Streaming {
            Ok(())
        } else {
            Err(SessionError::Protocol(
                "message before authorization".into(),
            ))
        }
    }
    async fn tick(
        &mut self,
        last_reassembly_tick_us: &mut u64,
    ) -> Result<Option<EndReason>, SessionError> {
        if self.approval.is_some_and(|due| Instant::now() >= due) {
            self.emit_error(
                ErrorCode::ApprovalDenied,
                "approval timeout".into(),
                BTreeMap::from([("reason".into(), "timeout".into())]),
                0,
            );
            let _ = self
                .send(pb::Goodbye {
                    reason: pb::goodbye::Reason::UserClosed as i32,
                    message: "approval timeout".into(),
                })
                .await;
            self.drain().await;
            return Ok(Some(EndReason::Error(ErrorCode::ApprovalDenied)));
        }
        if self.state() == SessionStateKind::Authenticating
            && self.started.elapsed() >= Duration::from_secs(5)
        {
            return Ok(Some(EndReason::Protocol));
        }
        if self.ping_due.is_some_and(|due| Instant::now() >= due) {
            let t1_us = unix_micros();
            self.send(pb::Ping { t1_us }).await?;
            if let Some(driver) = &mut self.driver {
                driver.on_ping_sent(t1_us, Instant::now());
            }
            self.ping_schedule.on_sent(self.now_us());
            lock(&self.shared.stats).pings_sent += 1;
            let delay_us = self
                .ping_schedule
                .next_due(self.now_us())
                .saturating_sub(self.now_us());
            self.ping_due = Some(Instant::now() + Duration::from_micros(delay_us));
        }
        if self.feedback_due.is_some_and(|due| Instant::now() >= due) {
            if self.server_features & features::RATE_FEEDBACK != 0 {
                let now_us = self.now_us();
                let mut feedback = self.collector.feedback_with_low_power(
                    now_us,
                    self.epoch,
                    self.decode_queue,
                    self.gradient.gradient_us_per_s(now_us),
                    *lock(&self.shared.low_power),
                );
                (feedback.shards_received, feedback.shards_lost) = self.reception.counts(now_us);
                self.send(feedback).await?;
            }
            self.feedback_due = Some(Instant::now() + Duration::from_millis(200));
        }
        let now_us = self.now_us();
        let previous_tick_us = std::mem::replace(last_reassembly_tick_us, now_us);
        if self.state() == SessionStateKind::Streaming {
            if self.server_features & features::VIEWPORT != 0
                && self.options.features & features::VIEWPORT != 0
            {
                if let Some(message) = self.viewport.poll(now_us) {
                    self.send(message).await?;
                }
            }
            if now_us.saturating_sub(previous_tick_us) > 3 * 20_000 {
                // A missed 20 ms poll is not evidence of missing packets. Let
                // the media driver and receive loop resume before expiring slots.
                self.reassembler.pause_deadlines(previous_tick_us, now_us);
            }
            if let Some(hud) = self.collector.snapshot(self.now_us()) {
                let snapshot = {
                    let mut stats = lock(&self.shared.stats);
                    stats.hud = Some(hud);
                    stats.recv_kbps = hud.recv_kbps;
                    stats.clone()
                };
                self.emit(SessionEvent::Stats(snapshot));
            }
        }
        Ok(None)
    }
    async fn tick_reassembly(&mut self, now_us: u64) {
        if self.state() == SessionStateKind::Streaming {
            let events = self.reassembler.tick(now_us);
            self.video_events(events).await;
        }
    }
    async fn video_events(&mut self, events: Vec<ReassemblyEvent>) {
        for event in events {
            match event {
                ReassemblyEvent::Frame(frame) => {
                    if let Some(driver) = &mut self.driver {
                        if matches!(
                            driver.frame_verdict(self.current_conn_id, frame.keyframe),
                            crate::recovery::failover::FrameVerdict::Drop
                        ) {
                            continue;
                        }
                    }
                    let received_us = unix_micros();
                    let capture_local_us = self.clock.host_to_local(frame.capture_ts_us);
                    if let Some(local) = capture_local_us {
                        self.gradient.push(
                            self.now_us(),
                            (i128::from(received_us) - i128::from(local))
                                .clamp(i128::from(i64::MIN), i128::from(i64::MAX))
                                as i64,
                        );
                    }
                    let recovered =
                        self.reception
                            .finish(Some(frame.epoch), frame.frame_id, self.now_us());
                    self.collector.on_frame_complete(
                        self.now_us(),
                        frame.data.len(),
                        0, // Filled from the packet reception window below.
                        0,
                        recovered,
                    );
                    {
                        let mut stats = lock(&self.shared.stats);
                        stats.frames_received += 1;
                        stats.frames_recovered = self.reassembler.stats.frames_recovered;
                    }
                    self.last_good_frame = frame.frame_id;
                    self.emit(SessionEvent::Frame(ReceivedFrame {
                        frame,
                        received_us,
                        capture_local_us,
                    }));
                }
                ReassemblyEvent::Lost { frame_id } => {
                    self.reception.finish(None, frame_id, self.now_us());
                    lock(&self.shared.stats).frames_lost += 1;
                    self.collector.on_frame_lost(self.now_us());
                }
                ReassemblyEvent::NeedKeyframe {
                    reason,
                    last_good_frame,
                } => {
                    let reason = match reason {
                        KeyframeReason::Loss => pb::keyframe_request::Reason::Loss,
                        KeyframeReason::Backlog => pb::keyframe_request::Reason::Backlog,
                    };
                    let _ = self.keyframe(reason, last_good_frame).await;
                }
            }
        }
    }
    async fn keyframe(
        &mut self,
        reason: pb::keyframe_request::Reason,
        last_good_frame: u32,
    ) -> Result<(), SessionError> {
        if self.driver.as_ref().is_some_and(PathDriver::is_switching) {
            return Ok(());
        }
        if self
            .last_key
            .is_some_and(|at| at.elapsed() < Duration::from_millis(300))
        {
            return Ok(());
        }
        self.send(pb::KeyframeRequest {
            reason: reason as i32,
            last_good_frame,
        })
        .await?;
        self.last_key = Some(Instant::now());
        lock(&self.shared.stats).keyframe_requests += 1;
        Ok(())
    }
}
impl Drop for Runner {
    fn drop(&mut self) {
        self.redial_task.take();
        self.backup_task.take();
        self.conn.close(CloseCode::Normal, b"session task dropped");
        if let Some(backup) = self.backup.take() {
            backup.close(CloseCode::Normal, b"backup dropped");
        }
        if let Some((_, pending, _)) = self.pending_conn.take() {
            pending.close(CloseCode::Normal, b"pending path dropped");
        }
        self.upgrade_probe.take();
        self.upgrade_resume.take();
        if let Some((_, candidate, _)) = self.upgrade_conn.take() {
            candidate.close(CloseCode::Normal, b"upgrade candidate dropped");
        }
        if let Some(reader) = self.uni_reader.take() {
            reader.abort();
        }
    }
}

async fn uni_reader(
    conn: Conn,
    events: mpsc::UnboundedSender<SessionEvent>,
    shared: Arc<Shared>,
    accept_files: bool,
) {
    // JoinSet spawns independent cursor tasks, reaps completed ones, and aborts
    // remaining readers when this connection's accept task is stopped.
    let mut cursors = tokio::task::JoinSet::new();
    loop {
        while cursors.try_join_next().is_some() {}
        let Ok((kind, mut recv)) = conn.accept_uni().await else {
            return;
        };
        match kind {
            StreamKind::Cursor => {
                let events = events.clone();
                cursors.spawn(async move {
                    while let Ok(Some(payload)) = recv.recv_record().await {
                        let Ok(message) = pb::CursorMsg::decode(payload) else {
                            break;
                        };
                        if events.send(SessionEvent::CursorMessage(message)).is_err() {
                            return;
                        }
                    }
                });
            }
            StreamKind::File if accept_files => {
                if events
                    .send(SessionEvent::FileStream(IncomingFile(recv)))
                    .is_err()
                {
                    return;
                }
            }
            StreamKind::File => {
                let _ = recv.stop(crate::files::STREAM_NOT_ALLOWED);
            }
            StreamKind::Clipboard => {
                if shared.local_features & features::CLIPBOARD_TEXT == 0 {
                    tracing::debug!("clipboard text feature is disabled locally");
                    let _ = recv.stop(crate::files::STREAM_NOT_ALLOWED);
                    continue;
                }
                let events = events.clone();
                let shared = Arc::clone(&shared);
                cursors.spawn(async move {
                    let policy = ClipboardPolicy {
                        peer_features: shared.peer_features.load(Ordering::Acquire),
                        role: *lock(&shared.role),
                        to_phone: true,
                        from_phone: true,
                    };
                    match crate::clipboard::receive(&mut recv, &policy).await {
                        Ok(item) if item.kind == rsp_core::clipboard::ClipboardKind::Text => {
                            let _ = events.send(SessionEvent::Clipboard(item));
                        }
                        Ok(item) => {
                            tracing::debug!(
                                kind = %item.kind,
                                len = item.data.len(),
                                "discarding non-text clipboard item"
                            );
                        }
                        Err(error) => {
                            tracing::debug!(%error, "clipboard stream rejected");
                        }
                    }
                });
            }
            _ => {}
        }
    }
}
fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}
fn error_code(value: u32) -> Option<ErrorCode> {
    u16::try_from(value).ok().and_then(ErrorCode::from_u16)
}

/// Single protocol-ending table. A missing ErrorCode represents a normal UI
/// ending, retaining the protocol reason instead of inventing a numeric code.
#[derive(Debug, Clone, Copy)]
pub struct EndMapping {
    pub close_code: u64,
    pub reason: EndReason,
    pub error: Option<ErrorCode>,
}
pub const CLOSE_MAPPINGS: [EndMapping; 7] = [
    EndMapping {
        close_code: 0,
        reason: EndReason::Goodbye(pb::goodbye::Reason::UserClosed),
        error: None,
    },
    EndMapping {
        close_code: 1,
        reason: EndReason::Protocol,
        error: Some(ErrorCode::Other),
    },
    EndMapping {
        close_code: 2,
        reason: EndReason::Error(ErrorCode::PolicyDenied),
        error: Some(ErrorCode::PolicyDenied),
    },
    EndMapping {
        close_code: 3,
        reason: EndReason::Goodbye(pb::goodbye::Reason::Replaced),
        error: None,
    },
    EndMapping {
        close_code: 4,
        reason: EndReason::Goodbye(pb::goodbye::Reason::HostShutdown),
        error: None,
    },
    EndMapping {
        close_code: 5,
        reason: EndReason::Goodbye(pb::goodbye::Reason::IdleTimeout),
        error: None,
    },
    EndMapping {
        close_code: 6,
        reason: EndReason::Goodbye(pb::goodbye::Reason::Revoked),
        error: Some(ErrorCode::DeviceRevoked),
    },
];
pub fn close_code_to_error(code: u64) -> Option<ErrorCode> {
    CLOSE_MAPPINGS
        .iter()
        .find(|row| row.close_code == code)
        .map_or(Some(ErrorCode::Other), |row| row.error)
}
pub fn goodbye_reason_error(reason: pb::goodbye::Reason) -> Option<ErrorCode> {
    GOODBYE_MAPPINGS
        .iter()
        .find(|(value, _)| *value == reason)
        .and_then(|(_, error)| *error)
}

pub const GOODBYE_MAPPINGS: [(pb::goodbye::Reason, Option<ErrorCode>); 10] = [
    (pb::goodbye::Reason::Unspecified, None),
    (pb::goodbye::Reason::UserClosed, None),
    (pb::goodbye::Reason::HostClosedByLocalUser, None),
    (pb::goodbye::Reason::Replaced, None),
    (pb::goodbye::Reason::IdleTimeout, None),
    (pb::goodbye::Reason::Background, None),
    (pb::goodbye::Reason::HostShutdown, None),
    (pb::goodbye::Reason::HostUpdating, None),
    (pb::goodbye::Reason::Revoked, Some(ErrorCode::DeviceRevoked)),
    (
        pb::goodbye::Reason::PermissionLost,
        Some(ErrorCode::MissingPermissions),
    ),
];
