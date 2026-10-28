//! RSP/1 client over QUIC.
//!
//! The control stream carries length-prefixed protobuf envelopes. Video and
//! input transport remain unchanged from the M0 implementation.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use rsp_core::control::Codec;
use rsp_core::errors::ErrorCode;
use rsp_core::input::InputEvent;
use rsp_core::pb;
use rsp_core::video::Frame;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct StartParams {
    pub codec: Codec,
    pub max_width: u32,
    pub fps: u16,
    pub bitrate_kbps: u32,
}

impl Default for StartParams {
    fn default() -> Self {
        Self {
            codec: Codec::Hevc,
            max_width: 1920,
            fps: 60,
            bitrate_kbps: 12_000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub addr: SocketAddr,
    pub client_name: String,
    pub start: StartParams,
    /// SHA-256 of the host certificate. `None` accepts any certificate (M0 only).
    pub pin_sha256: Option<[u8; 32]>,
}

#[derive(Debug)]
pub struct ReceivedFrame {
    pub frame: Frame,
    pub received_us: u64,
    pub capture_local_us: Option<i64>,
}

#[derive(Debug)]
pub enum ClientEvent {
    Connected {
        host_name: String,
        display_width: u32,
        display_height: u32,
    },
    StreamConfig {
        width: u32,
        height: u32,
        fps: u16,
        cursor_x: f32,
        cursor_y: f32,
    },
    Frame(ReceivedFrame),
    ClockSync {
        rtt_us: u64,
        offset_us: i64,
    },
    HostError {
        code: u16,
        message: String,
    },
    Closed {
        reason: String,
    },
}

#[derive(Clone)]
pub struct ClientHandle {
    conn: quinn::Connection,
    _endpoint: quinn::Endpoint,
    session: session::SessionHandle,
    fingerprint: Arc<Mutex<Option<[u8; 32]>>>,
}
impl ClientHandle {
    pub fn send_input(&self, ev: InputEvent) {
        self.session.send_legacy_input(ev);
    }
    pub fn request_keyframe(&self) {
        self.session.request_keyframe();
    }
    pub fn keyframe_request_count(&self) -> u64 {
        self.session.stats().keyframe_requests
    }
    pub fn clock_offset_us(&self) -> Option<i64> {
        self.session.stats().offset_us
    }
    pub fn rtt(&self) -> Duration {
        self.conn.rtt()
    }
    pub fn stats(&self) -> quinn::ConnectionStats {
        self.conn.stats()
    }
    pub fn server_fingerprint(&self) -> Option<[u8; 32]> {
        *self.fingerprint.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn close(&self) {
        self.session.close();
    }
}

pub fn fingerprint_hex(fp: &[u8; 32]) -> String {
    fp.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn parse_fingerprint(hex: &str) -> Option<[u8; 32]> {
    let hex: String = hex.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Maps a Host `Goodbye` reason to the error code shown by the app.
///
/// Normal user-initiated endings do not carry an application error code.
/// `HOST_UPDATING` retains its reason for the recovery layer. See 03 section
/// 4.6 and the ending-reason table in 10-appendix.
pub fn goodbye_reason_to_error(reason: pb::goodbye::Reason) -> Option<ErrorCode> {
    session::goodbye_reason_error(reason)
}

#[derive(Debug)]
struct PinVerifier {
    pin: Option<[u8; 32]>,
    seen: Arc<Mutex<Option<[u8; 32]>>>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let digest = ring::digest::digest(&ring::digest::SHA256, end_entity);
        let fp: [u8; 32] = digest.as_ref().try_into().expect("sha256 is 32 bytes");
        *self.seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(fp);
        match self.pin {
            Some(pin) if pin != fp => Err(rustls::Error::General(format!(
                "host certificate {} does not match the pinned fingerprint",
                fingerprint_hex(&fp)
            ))),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn transport_config() -> quinn::TransportConfig {
    let mut t = quinn::TransportConfig::default();
    t.congestion_controller_factory(Arc::new(rsp_transport::quic::MediaCongestion));
    // Jitter can reorder a whole keyframe burst. Avoid spurious loss-induced
    // window collapse, with time-based loss detection still bounded to 2 RTTs.
    t.packet_threshold(128).time_threshold(2.0);
    t.max_idle_timeout(Some(
        Duration::from_secs(10)
            .try_into()
            .expect("valid idle timeout"),
    ))
    .keep_alive_interval(Some(Duration::from_secs(2)))
    .datagram_receive_buffer_size(Some(8 << 20))
    .datagram_send_buffer_size(rsp_transport::DATAGRAM_SEND_BUFFER_BYTES);
    t
}

fn make_endpoint(
    opts: &ClientOptions,
    seen: Arc<Mutex<Option<[u8; 32]>>>,
) -> Result<quinn::Endpoint> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = PinVerifier {
        pin: opts.pin_sha256,
        seen,
        provider: provider.clone(),
    };
    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    tls.alpn_protocols = vec![rsp_core::ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    config.transport_config(Arc::new(transport_config()));
    let bind: SocketAddr = if opts.addr.is_ipv4() && opts.addr.ip().is_loopback() {
        "127.0.0.1:0".parse()?
    } else if opts.addr.is_ipv6() {
        "[::]:0".parse()?
    } else {
        "0.0.0.0:0".parse()?
    };
    let mut endpoint = quinn::Endpoint::client(bind)?;
    endpoint.set_default_client_config(config);
    Ok(endpoint)
}

/// M0 compatibility entry point. Certificate pinning retains its historical
/// whole-certificate SHA-256 meaning; SessionParams uses an SPKI fingerprint.
pub async fn connect(
    opts: ClientOptions,
) -> Result<(ClientHandle, mpsc::UnboundedReceiver<ClientEvent>)> {
    let fingerprint = Arc::new(Mutex::new(None));
    let endpoint = make_endpoint(&opts, fingerprint.clone())?;
    let conn = endpoint
        .connect(opts.addr, rsp_core::SERVER_NAME)?
        .await
        .with_context(|| format!("cannot connect to {}", opts.addr))?;
    let (session, mut events) = session::open_legacy(conn.clone(), &opts).await?;
    let (tx, rx) = mpsc::unbounded_channel();
    let close_on_drop = session.clone();
    tokio::spawn(async move {
        let mut emitted_error = None;
        loop {
            let event = tokio::select! {
                event = events.recv() => match event { Some(e) => e, None => break },
                _ = tx.closed() => { close_on_drop.close(); break; },
            };
            let mapped = match event {
                SessionEvent::ServerHello(hello) => {
                    let display = hello
                        .displays
                        .iter()
                        .find(|d| d.primary)
                        .or_else(|| hello.displays.first());
                    let (display_width, display_height) = display
                        .map(|d| (d.width_px, d.height_px))
                        .unwrap_or_default();
                    Some(ClientEvent::Connected {
                        host_name: hello.host_name,
                        display_width,
                        display_height,
                    })
                }
                SessionEvent::StreamConfig(c) => Some(ClientEvent::StreamConfig {
                    width: c.width,
                    height: c.height,
                    fps: c.fps.min(u32::from(u16::MAX)) as u16,
                    cursor_x: c.cursor_x,
                    cursor_y: c.cursor_y,
                }),
                SessionEvent::Frame(frame) => Some(ClientEvent::Frame(frame)),
                SessionEvent::ClockSync { rtt_us, offset_us } => {
                    Some(ClientEvent::ClockSync { rtt_us, offset_us })
                }
                SessionEvent::Error { code, message } => {
                    emitted_error = Some(code);
                    Some(ClientEvent::HostError {
                        code: code.code(),
                        message,
                    })
                }
                SessionEvent::Ended { reason, code, .. } => {
                    if let Some(code) = code {
                        if emitted_error != Some(code) {
                            let _ = tx.send(ClientEvent::HostError {
                                code: code.code(),
                                message: reason.clone(),
                            });
                        }
                    }
                    let _ = tx.send(ClientEvent::Closed { reason });
                    break;
                }
                _ => None,
            };
            if let Some(event) = mapped {
                if tx.send(event).is_err() {
                    close_on_drop.close();
                    break;
                }
            }
        }
    });
    Ok((
        ClientHandle {
            conn,
            _endpoint: endpoint,
            session,
            fingerprint,
        },
        rx,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_hex_round_trip() {
        let fp = [0xabu8; 32];
        let hex = fingerprint_hex(&fp);
        assert_eq!(parse_fingerprint(&hex), Some(fp));
        assert_eq!(
            parse_fingerprint(&format!("{}:{}", &hex[..10], &hex[10..])),
            Some(fp)
        );
        assert_eq!(parse_fingerprint("abc"), None);
    }

    #[test]
    fn goodbye_reasons_map_to_the_appendix_error_codes() {
        use pb::goodbye::Reason;

        assert_eq!(
            goodbye_reason_to_error(Reason::Revoked),
            Some(ErrorCode::DeviceRevoked)
        );
        assert_eq!(
            goodbye_reason_to_error(Reason::PermissionLost),
            Some(ErrorCode::MissingPermissions)
        );
        for reason in [
            Reason::Unspecified,
            Reason::UserClosed,
            Reason::HostClosedByLocalUser,
            Reason::Replaced,
            Reason::IdleTimeout,
            Reason::Background,
            Reason::HostShutdown,
            Reason::HostUpdating,
        ] {
            assert_eq!(goodbye_reason_to_error(reason), None);
        }
    }
}

pub mod audio;
pub mod clipboard;
pub mod dial;
pub mod feedback;
pub mod prewarm;
pub mod reassembly;
pub mod recovery;
pub mod session;
pub mod stats;
pub mod viewport;

pub use prewarm::PrewarmPool;
pub use session::{
    close_code_to_error, goodbye_reason_error, probe, CursorEvent, EndReason, EventStream,
    HandshakeTimings, InvalidTransition, ProbeResult, ServerHelloSummary, Session, SessionError,
    SessionEvent, SessionHandle, SessionParams, SessionState, SessionStateKind, SessionStats,
    StateTransition,
};

pub mod files;
