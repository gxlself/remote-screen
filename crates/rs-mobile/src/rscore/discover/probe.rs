//! Raw TCP and system protocol probes.

use std::{io, time::Duration};

use rsp_transport::{NetDialer, NetStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{timeout_at, Instant};

use super::super::dto_core::{DiscoverOutcome, DiscoverProbe};
use super::super::error::CoreError;

// TPKT, X.224 CR TPDU, then RDP_NEG_REQ (SSL | HYBRID, little endian).
const RDP_CONNECTION_REQUEST: [u8; 19] = [
    0x03, 0x00, 0x00, 0x13, 0x0e, 0xe0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x08, 0x00, 0x03,
    0x00, 0x00, 0x00,
];

/// Make one TCP connection on the selected network, without authenticating.
/// Dialing and protocol inspection share a timeout clamped to 200–5000 ms.
pub async fn probe(
    net: &dyn NetDialer,
    addr: &str,
    port: u16,
    kind: &str,
    timeout: Duration,
) -> Result<DiscoverProbe, CoreError> {
    if !matches!(kind, "vnc" | "rdp" | "tcp") {
        return Err(CoreError::internal("unknown probe kind"));
    }

    let started = Instant::now();
    let deadline = started + timeout.clamp(Duration::from_millis(200), Duration::from_secs(5));
    let (outcome, banner) = match timeout_at(deadline, net.dial_tcp_host(addr, port)).await {
        Ok(Ok(mut stream)) => {
            // Dropping the stream also closes silent or mismatched peers. VNC
            // never receives a client version, so no login is attempted.
            match timeout_at(deadline, check_protocol(&mut stream, kind)).await {
                Ok(Ok(banner)) => (DiscoverOutcome::Open, banner),
                _ => (DiscoverOutcome::Mismatch, None),
            }
        }
        Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionRefused => {
            (DiscoverOutcome::Refused, None)
        }
        _ => (DiscoverOutcome::Timeout, None),
    };
    let ms = started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
    tracing::debug!(addr, port, kind, ?outcome, ms, "discovery probe finished");
    Ok(DiscoverProbe {
        addr: addr.to_owned(),
        port,
        outcome,
        ms,
        banner,
    })
}

async fn check_protocol(stream: &mut NetStream, kind: &str) -> io::Result<Option<String>> {
    match kind {
        "vnc" => {
            let mut version = [0; 12];
            stream.read_exact(&mut version).await?;
            if !version.starts_with(b"RFB ") {
                return Err(io::ErrorKind::InvalidData.into());
            }
            Ok(Some(
                String::from_utf8_lossy(&version)
                    .trim_end_matches('\n')
                    .chars()
                    .take(11)
                    .collect(),
            ))
        }
        "rdp" => {
            stream.write_all(&RDP_CONNECTION_REQUEST).await?;
            stream.flush().await?;
            let mut header = [0; 4];
            stream.read_exact(&mut header).await?;
            let length = usize::from(u16::from_be_bytes([header[2], header[3]]));
            // A Connection Confirm needs at least the seven-byte X.224 TPDU.
            // Negotiation replies are small; bound allocation to 4 KiB.
            if header[..2] != [0x03, 0x00] || !(11..=4096).contains(&length) {
                return Err(io::ErrorKind::InvalidData.into());
            }
            let mut body = vec![0; length - header.len()];
            stream.read_exact(&mut body).await?;
            Ok(None)
        }
        _ => Ok(None), // "tcp" was validated before dialing.
    }
}
