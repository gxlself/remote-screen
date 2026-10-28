//! Minimal standalone IronRDP client for size measurement and testkit probes.
//! Usage: size_probe ADDRESS:PORT [USER PASSWORD [DOMAIN]].
//! Test/probe only: accepts self-signed certificates; no app client is used.

#[path = "size_baseline.rs"]
mod tls;

use std::io;
use std::net::SocketAddr;

use ironrdp_connector::{ClientConnector, Config, ConnectorError, Credentials, DesktopSize};
use ironrdp_graphics::image_processing::PixelFormat;
use ironrdp_input::{Database, Operation, Scancode};
use ironrdp_pdu::gcc::{ConnectionType, KeyboardType};
use ironrdp_pdu::input::fast_path::FastPathInputEvent;
use ironrdp_pdu::rdp::capability_sets::{MajorPlatformType, RailSupportLevel};
use ironrdp_session::image::DecodedImage;
use ironrdp_session::{ActiveStage, ActiveStageBuilder, ActiveStageOutput};
use ironrdp_tokio::{FramedWrite as _, NetworkClient, TokioFramed};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("connect_begin: {0}")]
    Begin(#[source] ConnectorError),
    #[error("connect_finalize: {0}")]
    Finalize(#[source] ConnectorError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// NTLM needs no KDC/HTTP access. Fail closed if SSPI unexpectedly asks for it.
struct NoNetwork;

impl NetworkClient for NoNetwork {
    async fn send(
        &mut self,
        _: &ironrdp_connector::sspi::generator::NetworkRequest,
    ) -> ironrdp_connector::ConnectorResult<Vec<u8>> {
        Err(ironrdp_connector::general_err!(
            "probe forbids external authentication traffic"
        ))
    }
}

pub fn config(user: &str, password: &str, domain: Option<String>, credssp: bool) -> Config {
    Config {
        desktop_size: DesktopSize {
            width: 1024,
            height: 768,
        },
        monitor_layout: None,
        desktop_scale_factor: 100,
        enable_tls: true,
        enable_credssp: credssp,
        enable_standard_rdp_security: false,
        credentials: Credentials::UsernamePassword {
            username: user.into(),
            password: password.into(),
        },
        domain,
        client_build: 0,
        client_name: "rdp-probe".into(),
        keyboard_type: KeyboardType::IBM_ENHANCED,
        keyboard_subtype: 0,
        keyboard_functional_keys_count: 12,
        keyboard_layout: 0,
        connection_type: ConnectionType::Lan,
        ime_file_name: String::new(),
        bitmap: None,
        dig_product_id: String::new(),
        client_dir: String::new(),
        alternate_shell: String::new(),
        work_dir: String::new(),
        remote_application_mode: false,
        rail_support_level: RailSupportLevel::empty(),
        platform: MajorPlatformType::UNSPECIFIED,
        hardware_id: None,
        request_data: None,
        autologon: false,
        enable_audio_playback: false,
        enable_audio_capture: false,
        performance_flags: Default::default(),
        license_cache: None,
        timezone_info: Default::default(),
        compression_type: None,
        enable_server_pointer: true,
        pointer_software_rendering: false,
        multitransport_flags: None,
        support_dyn_vc_gfx_protocol: false,
    }
}

pub struct ProbeClient {
    pub framed: TokioFramed<TlsStream<TcpStream>>,
    pub stage: ActiveStage,
    pub image: DecodedImage,
    pub leaf_der: Vec<u8>,
}

impl ProbeClient {
    pub async fn connect(addr: SocketAddr, config: Config) -> Result<Self, ProbeError> {
        let stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;
        let mut connector = ClientConnector::new(config, stream.local_addr()?);
        let mut framed = TokioFramed::new(stream);
        let should_upgrade = ironrdp_tokio::connect_begin(&mut framed, &mut connector)
            .await
            .map_err(ProbeError::Begin)?;
        let (stream, leftover) = framed.into_inner();
        if !leftover.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected bytes before TLS upgrade",
            )
            .into());
        }
        // TLS must wrap the original byte stream, not the RDP framing layer.
        let stream = tls::tls_connect(stream).await?;
        let leaf_der = stream
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|chain| chain.first())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "missing TLS leaf certificate")
            })?
            .as_ref()
            .to_vec();
        let (_, cert) = x509_parser::parse_x509_certificate(&leaf_der)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        let public_key = cert.public_key().subject_public_key.data.to_vec();
        let upgraded = ironrdp_tokio::mark_as_upgraded(should_upgrade, &mut connector);
        let mut framed = TokioFramed::new(stream);
        let result = ironrdp_tokio::connect_finalize(
            upgraded,
            connector,
            &mut framed,
            &mut NoNetwork,
            "fake-rdp".into(),
            public_key,
            None,
        )
        .await
        .map_err(ProbeError::Finalize)?;
        let image = DecodedImage::new(
            PixelFormat::RgbA32,
            result.desktop_size.width,
            result.desktop_size.height,
        );
        let stage = ActiveStageBuilder {
            static_channels: result.static_channels,
            user_channel_id: result.user_channel_id,
            io_channel_id: result.io_channel_id,
            message_channel_id: result.message_channel_id,
            share_id: result.share_id,
            compression_type: result.compression_type,
            enable_server_pointer: true,
            pointer_software_rendering: false,
        }
        .build();
        Ok(Self {
            framed,
            stage,
            image,
            leaf_der,
        })
    }

    pub async fn next(&mut self) -> Result<Vec<ActiveStageOutput>, Box<dyn std::error::Error>> {
        let (action, frame) = self.framed.read_pdu().await?;
        let outputs =
            self.stage
                .process(&mut self.image, action, &frame, self.framed.last_read_at())?;
        self.respond(&outputs).await?;
        Ok(outputs)
    }

    pub async fn send(
        &mut self,
        events: &[FastPathInputEvent],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let outputs = self.stage.process_fastpath_input(&mut self.image, events)?;
        self.respond(&outputs).await?;
        Ok(())
    }

    async fn respond(&mut self, outputs: &[ActiveStageOutput]) -> io::Result<()> {
        for output in outputs {
            if let ActiveStageOutput::ResponseFrame(frame) = output {
                self.framed.write_all(frame).await?;
            }
        }
        Ok(())
    }
}

// Also included in tests/probe.rs: the test client is independent of client.rs.
#[allow(dead_code)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let addr: SocketAddr = args
        .next()
        .ok_or("usage: size_probe ADDRESS:PORT [USER PASSWORD [DOMAIN]]")?
        .parse()?;
    let user = args.next();
    let password = args.next().unwrap_or_default();
    let cfg = config(
        user.as_deref().unwrap_or_default(),
        &password,
        args.next(),
        user.is_some(),
    );
    let mut client = ProbeClient::connect(addr, cfg).await?;
    let mut input = Database::new();
    let events = input.apply([
        Operation::KeyPressed(Scancode::from_u8(false, 0x1e)),
        Operation::KeyReleased(Scancode::from_u8(false, 0x1e)),
    ]);
    client.send(&events).await?;
    loop {
        for output in client.next().await? {
            match output {
                ActiveStageOutput::GraphicsUpdate(rect) => {
                    // Keep decoded pixel data observable in optimized builds.
                    println!("graphics {rect:?}, {} bytes", client.image.data().len());
                }
                ActiveStageOutput::Terminate(reason) => {
                    println!("server closed: {reason}");
                    return Ok(());
                }
                _ => {}
            }
        }
    }
}
