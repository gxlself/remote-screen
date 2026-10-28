//! Encoded frames over QUIC shards or the TCP latest-frame slot.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rsp_core::{
    stats::HostStatsCollector,
    video::{self, FrameMeta, FLAG_KEYFRAME, FLAG_STILL},
};
use rsp_transport::{conn::SendDatagramError, Conn, ConnError, DatagramKind, TransportKind};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

use super::backlog::{wait_for_room, watermark_bytes, BacklogStats, Room, VIDEO_BACKLOG_MAX_WAIT};
use super::diagnostics::{self, Counter};
use crate::platform::EncodedFrame;

#[derive(Default)]
struct SendStats {
    frames: u64,
    keyframes: u64,
    bytes: u64,
    max_frame: usize,
    encode_lat_sum_us: u64,
    encode_lat_max_us: u64,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    conn: Conn,
    mut rx: mpsc::Receiver<EncodedFrame>,
    dump: Option<PathBuf>,
    display_id: u8,
    epoch: u8,
    fec: watch::Receiver<f32>,
    encoded_keyframes: watch::Sender<Option<(tokio::time::Instant, u64)>>,
    repairs: watch::Sender<Option<u64>>,
    host_stats: Arc<Mutex<HostStatsCollector>>,
) {
    let mut dump_file = match dump {
        Some(path) => match tokio::fs::File::create(&path).await {
            Ok(file) => {
                info!(path = %path.display(), "dumping stream");
                Some(file)
            }
            Err(error) => {
                warn!(path = %path.display(), %error, "cannot create dump file");
                None
            }
        },
        None => None,
    };
    let mut pacer = super::backlog::VideoPacer::default();
    let mut pending = std::collections::VecDeque::new();
    let connection_id = conn.stable_id();
    let mut frame_id = 0u32;
    let mut needs_keyframe = false;
    let mut stats = SendStats::default();
    let mut backlog_stats = BacklogStats::default();
    let mut window_start = Instant::now();
    while let Some(frame) = match pending.pop_front() {
        Some(frame) => Some(frame),
        None => rx.recv().await,
    } {
        let send_started = Instant::now();
        diagnostics::max(
            Counter::StartAgeMaxUs,
            rsp_core::clock::unix_micros().saturating_sub(frame.capture_ts_us),
        );
        if needs_keyframe && !frame.keyframe {
            diagnostics::add(Counter::ChainDrops, 1);
            continue;
        }
        diagnostics::max(Counter::QueueMaxFrames, (rx.len() + pending.len()) as u64);
        if frame.keyframe {
            diagnostics::add(Counter::Keyframes, 1);
            diagnostics::add(Counter::KeyBytes, frame.data.len() as u64);
            diagnostics::max(Counter::KeyMaxBytes, frame.data.len() as u64);
        }
        let latency = frame.encoded_ts_us.saturating_sub(frame.capture_ts_us);
        if let Ok(mut stats) = host_stats.lock() {
            stats.on_encoded(latency);
        }
        let redundancy = *fec.borrow();
        if let Some(file) = dump_file.as_mut() {
            if let Err(error) = file.write_all(&frame.data).await {
                warn!(%error, "cannot write stream dump");
                dump_file = None;
            }
        }
        let tcp = conn.kind() == TransportKind::Tcp;
        let mut max = if tcp {
            0
        } else {
            match conn.max_datagram_size() {
                Some(max) if max + 1 > video::HEADER_LEN => max + 1,
                _ => {
                    warn!("peer does not support video datagrams");
                    return;
                }
            }
        };
        let flags = if frame.keyframe {
            FLAG_KEYFRAME
        } else if frame.still_refine {
            FLAG_STILL
        } else {
            0
        };
        let meta = FrameMeta {
            flags,
            display_id,
            epoch,
            frame_id,
            capture_ts_us: frame.capture_ts_us,
        };
        let mut sent = false;
        for attempt in 0..2 {
            let packets = if tcp {
                video::packetize_tcp(&frame.data, meta)
            } else {
                video::packetize(&frame.data, meta, max, redundancy)
            };
            let datagrams = match packets {
                Ok(datagrams) => datagrams,
                Err(error) => {
                    warn!(%error, frame_id, "cannot packetize video frame");
                    break;
                }
            };
            let mut retry = false;
            for datagram in datagrams {
                let len = datagram.len();
                if !tcp {
                    diagnostics::max(
                        Counter::BacklogMaxBytes,
                        conn.datagram_backlog_bytes() as u64,
                    );
                    let watermark = watermark_bytes();
                    if watermark > 0 {
                        if super::backlog::AudioPriority::active(&connection_id) {
                            if let Some(quic) = conn.as_quic() {
                                let path = quic.stats().path;
                                super::backlog::prefetch_while_waiting(
                                    pacer.wait(
                                        len + 64,
                                        watermark,
                                        path.rtt,
                                        path.min_rtt,
                                        path.cwnd,
                                    ),
                                    &mut rx,
                                    &mut pending,
                                )
                                .await;
                            }
                        }
                        diagnostics::max(
                            Counter::BacklogMaxBytes,
                            conn.datagram_backlog_bytes() as u64,
                        );
                        record_room(
                            wait_for_room(&conn, watermark, VIDEO_BACKLOG_MAX_WAIT).await,
                            &mut backlog_stats,
                        );
                    }
                }
                match conn
                    .send_datagram_wait(DatagramKind::Video, datagram.slice(1..))
                    .await
                {
                    Ok(()) => {
                        if let Ok(mut stats) = host_stats.lock() {
                            stats.on_sent(len);
                        }
                    }
                    Err(ConnError::SendDatagram(SendDatagramError::TooLarge)) if attempt == 0 => {
                        if let Some(new_max) = conn
                            .max_datagram_size()
                            .map(|n| n + 1)
                            .filter(|n| *n > video::HEADER_LEN)
                        {
                            max = new_max;
                            retry = true;
                        }
                        break;
                    }
                    Err(error) => {
                        info!(%error, "video stopped");
                        return;
                    }
                }
            }
            if retry {
                continue;
            }
            sent = true;
            break;
        }
        if !sent {
            needs_keyframe = true;
            repairs.send_replace(Some(frame.capture_ts_us));
            continue;
        }
        if frame.keyframe {
            needs_keyframe = false;
            encoded_keyframes
                .send_replace(Some((tokio::time::Instant::now(), frame.capture_ts_us)));
        }
        diagnostics::max(
            Counter::SendAgeMaxUs,
            rsp_core::clock::unix_micros().saturating_sub(frame.capture_ts_us),
        );
        diagnostics::max(
            Counter::SendDurationMaxUs,
            send_started.elapsed().as_micros() as u64,
        );
        frame_id = frame_id.wrapping_add(1);
        stats.frames += 1;
        stats.keyframes += u64::from(frame.keyframe);
        stats.bytes += frame.data.len() as u64;
        stats.max_frame = stats.max_frame.max(frame.data.len());
        stats.encode_lat_sum_us += latency;
        stats.encode_lat_max_us = stats.encode_lat_max_us.max(latency);
        let elapsed = window_start.elapsed();
        if elapsed >= Duration::from_secs(2) {
            let seconds = elapsed.as_secs_f64();
            info!(
                backlog_waits = backlog_stats.waits,
                backlog_timeouts = backlog_stats.timeouts,
                backlog_waited_us = backlog_stats.waited_us,
                "video {:.1} fps | {:.2} Mbps | {} key | capture->encoded avg {:.1} ms max {:.1} ms | max frame {} KB | rtt {:.1} ms | datagram <={} B",
                stats.frames as f64 / seconds,
                stats.bytes as f64 * 8.0 / seconds / 1e6,
                stats.keyframes,
                stats.encode_lat_sum_us as f64 / stats.frames.max(1) as f64 / 1000.0,
                stats.encode_lat_max_us as f64 / 1000.0,
                stats.max_frame / 1024,
                conn.as_quic().map_or(0.0, |quic| quic.rtt().as_secs_f64() * 1000.0),
                max,
            );
            stats = SendStats::default();
            backlog_stats = BacklogStats::default();
            window_start = Instant::now();
        }
    }
}

fn record_room(room: Room, stats: &mut BacklogStats) {
    match room {
        Room::Ready => {}
        Room::Waited(elapsed) => {
            diagnostics::add(Counter::Waits, 1);
            stats.waits += 1;
            stats.waited_us += elapsed.as_micros() as u64;
        }
        Room::TimedOut => {
            diagnostics::add(Counter::Waits, 1);
            diagnostics::add(Counter::Timeouts, 1);
            stats.waits += 1;
            stats.timeouts += 1;
            stats.waited_us += VIDEO_BACKLOG_MAX_WAIT.as_micros() as u64;
        }
    }
}
