//! QUIC endpoint configuration, listener and dial function.

use std::any::Any;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use quinn::congestion::{Controller, ControllerFactory, ControllerMetrics, CubicConfig};
use quinn_proto::RttEstimator;
use rustls::{ClientConfig, ServerConfig};

use crate::conn::{Conn, ConnError};

fn check_alpn(alpn: &[Vec<u8>]) -> Result<(), ConnError> {
    if alpn.iter().any(|p| p.as_slice() == b"rsp/1") {
        Ok(())
    } else {
        Err(ConnError::MissingAlpn)
    }
}

/// Maximum queued outbound QUIC datagram bytes.
pub const DATAGRAM_SEND_BUFFER_BYTES: usize = 8 << 20;

fn transport_config() -> quinn::TransportConfig {
    let mut transport = quinn::TransportConfig::default();
    transport.congestion_controller_factory(Arc::new(MediaCongestion));
    // Independent jitter can reorder an entire keyframe burst. The default
    // three-packet / 9/8 RTT thresholds mistake that for congestion and split
    // subsequent frames across ACK rounds. Time-based loss detection remains
    // bounded to two RTTs, including when the path's RTT suddenly increases.
    transport.packet_threshold(128).time_threshold(2.0);
    transport.max_idle_timeout(Some(quinn::VarInt::from_u32(10_000).into()));
    // Conn supplies the mutable keep-alive schedule.
    transport.keep_alive_interval(None);
    transport.initial_mtu(1200);
    let mut mtu = quinn::MtuDiscoveryConfig::default();
    mtu.upper_bound(1452);
    transport.mtu_discovery_config(Some(mtu));
    transport.datagram_receive_buffer_size(Some(4 << 20));
    transport.datagram_send_buffer_size(DATAGRAM_SEND_BUFFER_BYTES);
    transport.max_concurrent_uni_streams(64u32.into());
    transport
}

pub struct QuicListener {
    endpoint: quinn::Endpoint,
}

pub struct QuicIncoming {
    incoming: quinn::Incoming,
    remote: SocketAddr,
}

impl QuicListener {
    pub async fn bind(addr: SocketAddr, server_cfg: ServerConfig) -> Result<Self, ConnError> {
        check_alpn(&server_cfg.alpn_protocols)?;
        let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(Arc::new(server_cfg))
            .map_err(|e| ConnError::Tls(e.to_string()))?;
        let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
        config.transport_config(Arc::new(transport_config()));
        let endpoint = quinn::Endpoint::server(config, addr).map_err(ConnError::from)?;
        Ok(Self { endpoint })
    }
    pub fn local_addr(&self) -> Result<SocketAddr, ConnError> {
        Ok(self.endpoint.local_addr()?)
    }
    pub async fn accept(&self) -> Result<QuicIncoming, ConnError> {
        let incoming = self.endpoint.accept().await.ok_or(ConnError::Closed)?;
        let remote = incoming.remote_address();
        Ok(QuicIncoming { incoming, remote })
    }
    pub fn endpoint(&self) -> &quinn::Endpoint {
        &self.endpoint
    }
}

impl QuicIncoming {
    pub fn remote_addr(&self) -> SocketAddr {
        self.remote
    }
    pub async fn accept(self) -> Result<Conn, ConnError> {
        let connection = self
            .incoming
            .accept()
            .map_err(|e| ConnError::Quic(e.to_string()))?
            .await
            .map_err(|e| ConnError::Quic(e.to_string()))?;
        verify_alpn(&connection)?;
        Ok(Conn::from_quic(connection, Duration::from_secs(15)))
    }
    pub fn ignore(self) {
        self.incoming.ignore();
    }
}

pub async fn connect_quic(
    endpoint: &quinn::Endpoint,
    addr: SocketAddr,
    cfg: Arc<ClientConfig>,
    prewarm: bool,
) -> Result<Conn, ConnError> {
    check_alpn(&cfg.alpn_protocols)?;
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(cfg)
        .map_err(|e| ConnError::Tls(e.to_string()))?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    config.transport_config(Arc::new(transport_config()));
    let connecting = endpoint
        .connect_with(config, addr, "rs-host")
        .map_err(|e| ConnError::Quic(e.to_string()))?;
    let connection = connecting
        .await
        .map_err(|e| ConnError::Quic(e.to_string()))?;
    verify_alpn(&connection)?;
    Ok(Conn::from_quic(
        connection,
        Duration::from_secs(if prewarm { 15 } else { 2 }),
    ))
}

fn verify_alpn(connection: &quinn::Connection) -> Result<(), ConnError> {
    let valid = connection
        .handshake_data()
        .and_then(|data| data.downcast::<quinn::crypto::rustls::HandshakeData>().ok())
        .is_some_and(|data| data.protocol.as_deref() == Some(b"rsp/1"));
    if valid {
        Ok(())
    } else {
        connection.close(1u32.into(), b"ALPN mismatch");
        Err(ConnError::MissingAlpn)
    }
}

const INITIAL_WINDOW: u64 = 240_000;
// IT-07 produces 63,492-byte keyframes; 84 KiB covers their 25% parity
// and packet overhead at the configured 1200..1452-byte MTUs.
// Keep this below the measured 4 Mbps path's 200 ms queue budget (100 KB).
// Persistent congestion must still be able to reduce the flight to two MTUs.
const MEDIA_WINDOW: u64 = 84 * 1024;

/// Cubic with a minimum media flight, except during persistent congestion.
#[derive(Debug, Default)]
pub struct MediaCongestion;

impl ControllerFactory for MediaCongestion {
    fn build(self: Arc<Self>, now: Instant, mtu: u16) -> Box<dyn Controller> {
        let mut config = CubicConfig::default();
        config.initial_window(INITIAL_WINDOW);
        Box::new(MediaCubic {
            inner: Arc::new(config).build(now, mtu),
            persistent: false,
        })
    }
}

struct MediaCubic {
    inner: Box<dyn Controller>,
    persistent: bool,
}

impl Controller for MediaCubic {
    fn on_sent(&mut self, now: Instant, bytes: u64, last_packet_number: u64) {
        self.inner.on_sent(now, bytes, last_packet_number);
    }
    fn on_ack(
        &mut self,
        now: Instant,
        sent: Instant,
        bytes: u64,
        app_limited: bool,
        rtt: &RttEstimator,
    ) {
        self.inner.on_ack(now, sent, bytes, app_limited, rtt);
        // Persistent congestion retains QUIC's minimum window until Cubic has
        // recovered naturally. A single ACK must not restore a large flight.
        if self.inner.window() >= MEDIA_WINDOW {
            self.persistent = false;
        }
    }
    fn on_end_acks(
        &mut self,
        now: Instant,
        in_flight: u64,
        app_limited: bool,
        largest_packet_num_acked: Option<u64>,
    ) {
        self.inner
            .on_end_acks(now, in_flight, app_limited, largest_packet_num_acked);
    }
    fn on_congestion_event(
        &mut self,
        now: Instant,
        sent: Instant,
        persistent: bool,
        lost_bytes: u64,
    ) {
        self.persistent |= persistent;
        self.inner
            .on_congestion_event(now, sent, persistent, lost_bytes);
    }
    fn on_mtu_update(&mut self, mtu: u16) {
        self.inner.on_mtu_update(mtu);
    }
    fn window(&self) -> u64 {
        if self.persistent {
            self.inner.window()
        } else {
            self.inner.window().max(MEDIA_WINDOW)
        }
    }
    fn metrics(&self) -> ControllerMetrics {
        let mut metrics = self.inner.metrics();
        metrics.congestion_window = self.window();
        metrics
    }
    fn clone_box(&self) -> Box<dyn Controller> {
        Box::new(Self {
            inner: self.inner.clone_box(),
            persistent: self.persistent,
        })
    }
    fn initial_window(&self) -> u64 {
        self.inner.initial_window()
    }
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn loss_backs_off_but_a_media_frame_does_not_need_multiple_ack_rounds() {
        let mut now = Instant::now();
        let mut cc = Arc::new(MediaCongestion).build(now, 1200);
        assert_eq!(cc.window(), INITIAL_WINDOW);
        for _ in 0..32 {
            let sent = now + Duration::from_millis(1);
            now += Duration::from_millis(200);
            cc.on_congestion_event(now, sent, false, 1200);
        }
        assert_eq!(cc.window(), MEDIA_WINDOW);
        assert_eq!(cc.clone_box().window(), MEDIA_WINDOW);
        assert_eq!(cc.metrics().congestion_window, MEDIA_WINDOW);
        let sent = now + Duration::from_millis(1);
        now += Duration::from_millis(200);
        cc.on_congestion_event(now, sent, true, 1200);
        assert_eq!(cc.window(), 2400);
    }
}
