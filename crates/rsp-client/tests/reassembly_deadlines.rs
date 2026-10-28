//! Exercise the production session's 20 ms receiver polls with a paused clock.

mod support;

use std::time::Duration;

use rsp_client::session::{EventStream, Session, SessionEvent, SessionHandle, SessionParams};
use rsp_core::{pb, video};
use rsp_transport::{
    identity::{CertKind, LocalIdentity},
    Conn, ControlStream, DatagramKind,
};
use tokio::time::Instant;

struct Receiver {
    _host: support::Host,
    conn: Conn,
    control: ControlStream,
    session: SessionHandle,
    events: EventStream,
}

impl Receiver {
    async fn start(fps: u32) -> Self {
        let host = support::Host::bind().await;
        let mut params = SessionParams::single(
            host.addr,
            host.fingerprint,
            LocalIdentity::generate_software(CertKind::App).expect("app identity"),
        );
        params.start.fps = fps as u16;
        let opening = tokio::spawn(Session::open(params));
        let (conn, control) = host.accept().await;
        support::hello(&control).await;
        support::auth(&control, pb::auth_result::Status::Ok).await;
        assert!(matches!(
            support::recv_command(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        support::send(
            &control,
            pb::StreamConfig {
                epoch: 1,
                fps,
                ..Default::default()
            },
        )
        .await;
        let (session, events) = opening.await.expect("open task").expect("session");
        let mut receiver = Self {
            _host: host,
            conn,
            control,
            session,
            events,
        };
        let key = packets(0, video::FLAG_KEYFRAME, 100, 0.0);
        receiver.queue(&key, &[0]);
        receiver.frame(0).await;
        receiver
    }

    fn queue(&self, packets: &[bytes::Bytes], indices: &[usize]) {
        for &index in indices {
            self.conn
                .send_datagram(DatagramKind::Video, packets[index].slice(1..))
                .expect("queue video datagram");
        }
    }

    async fn frame(&mut self, expected_id: u32) {
        loop {
            match self.events.recv().await.expect("session event") {
                SessionEvent::Frame(frame) => {
                    assert_eq!(frame.frame.frame_id, expected_id);
                    assert_eq!(
                        frame.frame.data,
                        vec![expected_id as u8; frame.frame.data.len()]
                    );
                    return;
                }
                SessionEvent::Ended { .. } | SessionEvent::Error { .. } => {
                    panic!("session ended before frame {expected_id}")
                }
                _ => {}
            }
        }
    }

    /// Transmit the queued datagrams before a control-stream round trip. With
    /// time frozen, the receiver drains its ready video branch before idling.
    async fn barrier(&self, datagrams: u64, marker: u64) -> Vec<pb::KeyframeRequest> {
        let quic = self.conn.as_quic().expect("QUIC test peer");
        while quic.stats().frame_tx.datagram < datagrams {
            tokio::task::yield_now().await;
        }
        support::send(&self.control, pb::Ping { t1_us: marker }).await;
        let mut losses = Vec::new();
        loop {
            match support::recv(&self.control).await {
                pb::envelope::Msg::Pong(pong) if pong.t1_us == marker => return losses,
                pb::envelope::Msg::Ping(ping) => {
                    support::send(
                        &self.control,
                        pb::Pong {
                            t1_us: ping.t1_us,
                            t2_us: rsp_core::clock::unix_micros(),
                        },
                    )
                    .await;
                }
                pb::envelope::Msg::KeyframeRequest(request) => losses.push(request),
                message => panic!("unexpected peer message: {message:?}"),
            }
        }
    }

    async fn settle_clock(&self) {
        // Establish normal 20 ms polling after real-I/O setup, whose duration
        // may itself include scheduler stalls on the shared test machine.
        for marker in [u64::MAX - 1, u64::MAX] {
            advance_clock(Duration::from_millis(20)).await;
            assert!(self.barrier(1, marker).await.is_empty());
        }
    }

    async fn finish(self) {
        support::finish(&self.conn, &self.control).await;
    }
}

// `advance` only yields; it does not wait for the timer driver (Tokio's
// time/clock.rs). A control-stream round trip can finish before that driver
// runs. Await a timer strictly inside the step, allowing for its 1 ms rounding,
// so each explicit step reaches the driver before the next I/O barrier.
async fn advance_clock(duration: Duration) {
    let timer = tokio::time::sleep(duration - Duration::from_millis(1));
    tokio::time::advance(duration).await;
    timer.await;
}

fn packets(id: u32, flags: u8, len: usize, redundancy: f32) -> Vec<bytes::Bytes> {
    video::packetize(
        &vec![id as u8; len],
        video::FrameMeta {
            flags,
            display_id: 1,
            epoch: 1,
            frame_id: id,
            capture_ts_us: rsp_core::clock::unix_micros(),
        },
        1150,
        redundancy,
    )
    .expect("packetize video")
}

#[tokio::test]
async fn paused_time_fixture_does_not_auto_advance_while_waiting_for_real_io() {
    tokio::time::pause();
    let started = Instant::now();
    let (sleep_ready_tx, sleep_ready_rx) = tokio::sync::oneshot::channel();
    let sleep = tokio::spawn(async move {
        let timer = tokio::time::sleep(Duration::from_millis(10));
        let _ = sleep_ready_tx.send(());
        timer.await;
    });

    support::without_time_advancing(async {
        sleep_ready_rx.await.expect("paused sleep registered");
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let waiter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            let _ = done_tx.send(());
        });
        done_rx.await.expect("real I/O completion");
        waiter.join().expect("real I/O thread");
    })
    .await;

    let elapsed = started.elapsed();
    let sleep_finished = sleep.is_finished();
    sleep.abort();
    tokio::time::resume();
    assert_eq!(elapsed, Duration::ZERO);
    assert!(!sleep_finished, "paused sleep was auto-advanced");
}

#[tokio::test]
async fn sparse_frame_loss_requests_keyframe_within_strict_deadline_and_one_poll() {
    for fps in [30, 60, 10] {
        let receiver = Receiver::start(fps).await;
        tokio::time::pause();
        support::without_time_advancing(async {
            receiver.settle_clock().await;
            // Seven data shards and two parity shards; only k-1 are delivered.
            // There are no subsequent video frames to advance the frame ids.
            let frame = packets(1, 0, 7 * 1122, 2.0 / 7.0);
            receiver.queue(&frame, &[0, 1, 2, 3, 4, 5]);
            assert!(receiver.barrier(7, 1).await.is_empty());
            let last_shard = Instant::now();
            let deadline = Duration::from_micros((2_000_000 / u64::from(fps)).max(50_000));
            let mut requests = Vec::new();
            // Include the poll exactly at the deadline. For fps=10 the 200 ms
            // deadline is a poll boundary; stopping at `< deadline` can leave
            // the request queued until the next tick. The frozen clock cannot
            // supply that missing poll implicitly.
            while last_shard.elapsed() <= deadline {
                advance_clock(Duration::from_millis(20)).await;
                requests.extend(receiver.barrier(7, 2).await);
                if last_shard.elapsed() < deadline {
                    assert!(requests.is_empty(), "premature LOSS at {fps} fps");
                    assert_eq!(receiver.session.stats().frames_lost, 0);
                }
            }
            assert!(last_shard.elapsed() <= deadline + Duration::from_millis(20));
            assert_eq!(requests.len(), 1);
            assert_eq!(
                requests[0].reason,
                pb::keyframe_request::Reason::Loss as i32
            );
            assert_eq!(requests[0].last_good_frame, 0);
            assert_eq!(receiver.session.stats().frames_lost, 1);
            assert_eq!(receiver.session.stats().keyframe_requests, 1);
        })
        .await;
        tokio::time::resume();
        receiver.finish().await;
    }
}

#[tokio::test]
async fn queued_fec_flight_survives_receiver_starvation() {
    for queue_before_resume in [true, false] {
        let mut receiver = Receiver::start(30).await;
        tokio::time::pause();
        support::without_time_advancing(async {
            receiver.settle_clock().await;
            let frame = packets(1, 0, 7 * 1122, 2.0 / 7.0);
            receiver.queue(&frame, &[0, 2, 3]);
            assert!(receiver.barrier(4, 1).await.is_empty());
            if queue_before_resume {
                receiver.queue(&frame, &[4, 5, 6, 7]);
            }
            // No session task can poll during this injected 200 ms gap. Test
            // both queued packets and a media driver resuming after the tick.
            advance_clock(Duration::from_millis(200)).await;
            assert!(receiver
                .barrier(if queue_before_resume { 8 } else { 4 }, 2)
                .await
                .is_empty());
            assert_eq!(receiver.session.stats().frames_lost, 0);
            assert_eq!(receiver.session.stats().keyframe_requests, 0);
            if !queue_before_resume {
                receiver.queue(&frame, &[4, 5, 6, 7]);
            }
            receiver.frame(1).await;
            assert_eq!(receiver.session.stats().frames_recovered, 1);
            for marker in 3..=8 {
                advance_clock(Duration::from_millis(20)).await;
                assert!(receiver.barrier(8, marker).await.is_empty());
            }
            assert_eq!(receiver.session.stats().frames_lost, 0);
            assert_eq!(receiver.session.stats().keyframe_requests, 0);
        })
        .await;
        tokio::time::resume();
        receiver.finish().await;
    }
}
