//! Bounded native-frame handoff and keyframe requests.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rsp_core::stats::HostStatsCollector;
use tokio::sync::{mpsc, watch};
use tokio::time::{sleep_until, Instant};
use tracing::warn;

use super::diagnostics::{self, Counter};
use super::policy::{KeyframeGate, StillRefiner};
use crate::platform::{CapturedFrame, EncodeOpts, Encoder};

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    mut encoder: Box<dyn Encoder>,
    output: mpsc::Sender<crate::platform::EncodedFrame>,
    mut frames: mpsc::Receiver<CapturedFrame>,
    mut keyframes: watch::Receiver<u64>,
    mut rates: mpsc::Receiver<(u32, u32)>,
    mut encoded_keyframes: watch::Receiver<Option<(Instant, u64)>>,
    mut repairs: watch::Receiver<Option<u64>>,
    mut roi: watch::Receiver<Vec<crate::platform::RoiRegion>>,
    hq_refresh: bool,
    stats: Arc<Mutex<HostStatsCollector>>,
) {
    let origin = Instant::now();
    let micros = |at: Instant| {
        at.saturating_duration_since(origin)
            .as_micros()
            .min(u128::from(u64::MAX)) as u64
    };
    // Pause/rebuild drops this task; each new pipeline starts with fresh policies.
    let mut refiner = StillRefiner::new(hq_refresh);
    let mut gate = KeyframeGate::default();
    let mut last_frame: Option<CapturedFrame> = None;
    let mut seen_requests = 0u64;
    let mut requested_keyframe = false;
    let mut repair_pending = false;
    let mut repair_after: Option<u64> = None;
    let mut last_key_capture: Option<u64> = None;
    let mut roi_open = true;
    loop {
        let deadline = refiner
            .deadline()
            .into_iter()
            .chain((!repair_pending).then(|| gate.deadline()).flatten())
            .min();
        tokio::select! {
            biased;
            Ok(()) = encoded_keyframes.changed() => {
                let encoded_at = *encoded_keyframes.borrow_and_update();
                if let Some((encoded_at, capture_us)) = encoded_at {
                    let now = micros(encoded_at);
                    // Requests received after force_keyframe still need their one
                    // merged response, even if output arrives asynchronously.
                    let repaired = repair_after.is_some_and(|after| capture_us >= after);
                    last_key_capture = Some(last_key_capture.unwrap_or(0).max(capture_us));
                    if requested_keyframe && !repaired {
                        gate.on_response(now);
                    } else {
                        gate.on_keyframe(now);
                    }
                    requested_keyframe = false;
                    if repaired {
                        repair_after = None;
                        repair_pending = false;
                    } else if repair_pending {
                        gate.defer();
                    }
                }
            }
            Ok(()) = repairs.changed() => {
                let failed_capture = *repairs.borrow_and_update();
                if let Some(after) = failed_capture.filter(|after| {
                    last_key_capture.is_none_or(|key| key < *after)
                        && repair_after.is_none_or(|pending| pending < *after)
                }) {
                    gate.encode_failed(micros(Instant::now()));
                    repair_after = Some(after);
                    repair_pending = true;
                }
            }
            roi_changed = roi.changed(), if roi_open => {
                match roi_changed {
                    Ok(()) => {
                        let regions = roi.borrow_and_update().clone();
                        if let Err(error) = encoder.set_roi(&regions) {
                            warn!(%error, "setting encoder ROI failed");
                        }
                    }
                    Err(_) => {
                        warn!("encoder ROI update channel closed");
                        roi_open = false;
                    }
                }
            }
            Ok(()) = keyframes.changed() => {
                let count = *keyframes.borrow_and_update();
                diagnostics::add(Counter::Requests, count.wrapping_sub(seen_requests));
                let now = micros(Instant::now());
                if repair_after.is_none() {
                    gate.set_backoff(false);
                }
                if repair_pending {
                    // The repair already covers this request once references drain.
                    gate.defer();
                } else if gate.request(now) {
                    diagnostics::add(Counter::ForceRequest, 1);
                    encoder.force_keyframe();
                    gate.mark_in_flight();
                    requested_keyframe = true;
                }
                if !repair_pending && count.wrapping_sub(seen_requests) > 1 {
                    gate.request(now);
                }
                seen_requests = count;
            }
            Some((kbps, fps)) = rates.recv() => encoder.set_rate(kbps, fps),
            frame = frames.recv() => {
                let Some(frame) = frame else {
                    refiner.reset();
                    gate.reset();
                    return;
                };
                refiner.on_frame(micros(Instant::now()));
                last_frame = Some(frame.clone());
                if let Ok(mut stats) = stats.lock() {
                    stats.on_captured(frame.capture_ts_us);
                }
                diagnostics::max(Counter::QueueMaxFrames, (output.max_capacity() - output.capacity()) as u64);
                // Leave callback headroom and discard only unencoded input:
                // no decoder reference is lost, so no replacement IDR is needed.
                if !super::backlog::source_ready(
                    output.max_capacity() - output.capacity(),
                    frame.capture_ts_us,
                    rsp_core::clock::unix_micros(),
                ) {
                    diagnostics::add(Counter::SourceDrops, 1);
                    continue;
                }
                if repair_pending {
                    if !gate.poll(micros(Instant::now())) {
                        diagnostics::add(Counter::SourceDrops, 1);
                        continue;
                    }
                    diagnostics::add(Counter::ForceRepair, 1);
                    encoder.force_keyframe();
                    gate.mark_in_flight();
                    requested_keyframe = true;
                    repair_pending = false;
                }
                diagnostics::add(Counter::EncodeFull, u64::from(output.capacity() == 0));
                let capture_us = frame.capture_ts_us;
                if let Err(error) = encoder.encode(frame, EncodeOpts::default()) {
                    diagnostics::add(Counter::ForceEncodeError, 1);
                    warn!(%error, "encode failed");
                    gate.encode_failed(micros(Instant::now()));
                    repair_after = Some(capture_us);
                    repair_pending = true;
                }
            }
            _ = async {
                match deadline {
                    Some(at) => sleep_until(origin + Duration::from_micros(at)).await,
                    None => std::future::pending().await,
                }
            } => {
                let now = micros(Instant::now());
                if !repair_pending && gate.poll(now) {
                    diagnostics::add(Counter::ForceDeadline, 1);
                    encoder.force_keyframe();
                    gate.mark_in_flight();
                    requested_keyframe = true;
                }
                if refiner.poll(now) {
                    if let Some(frame) = last_frame.clone().filter(|_| output.capacity() == output.max_capacity() && !repair_pending) {
                        // The unchanged picture retains its original screen timestamp.
                        let capture_us = frame.capture_ts_us;
                        if let Err(error) = encoder.encode(frame, EncodeOpts {
                            quality_boost: true,
                            ..EncodeOpts::default()
                        }) {
                            diagnostics::add(Counter::ForceRefineError, 1);
                            warn!(%error, "still refinement encode failed");
                            gate.encode_failed(micros(Instant::now()));
                            repair_after = Some(capture_us);
                            repair_pending = true;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{EncodedFrame, Result, RoiRegion};
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    struct TestEncoder {
        output: mpsc::Sender<EncodedFrame>,
        forced: Arc<AtomicU64>,
        rois: Arc<Mutex<Vec<Vec<RoiRegion>>>>,
        keyframe: bool,
    }

    impl Encoder for TestEncoder {
        fn encode(&mut self, frame: CapturedFrame, _: EncodeOpts) -> Result<()> {
            self.output
                .try_send(EncodedFrame {
                    data: vec![0],
                    keyframe: std::mem::take(&mut self.keyframe),
                    still_refine: false,
                    capture_ts_us: frame.capture_ts_us,
                    encoded_ts_us: frame.capture_ts_us,
                })
                .expect("source admission must preserve callback capacity");
            Ok(())
        }
        fn force_keyframe(&mut self) {
            self.forced.fetch_add(1, Relaxed);
            self.keyframe = true;
        }
        fn set_rate(&mut self, _: u32, _: u32) {}
        fn set_quality_boost(&mut self, _: bool) {}
        fn set_roi(&mut self, regions: &[RoiRegion]) -> Result<()> {
            self.rois
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(regions.to_vec());
            Ok(())
        }
    }

    struct Fixture {
        frames: mpsc::Sender<CapturedFrame>,
        output: mpsc::Receiver<EncodedFrame>,
        requests: watch::Sender<u64>,
        sent: watch::Sender<Option<(Instant, u64)>>,
        repairs: watch::Sender<Option<u64>>,
        roi: watch::Sender<Vec<RoiRegion>>,
        rois: Arc<Mutex<Vec<Vec<RoiRegion>>>>,
        forced: Arc<AtomicU64>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Fixture {
        fn new() -> Self {
            let (frames, frame_rx) = mpsc::channel(4);
            let (output_tx, output) = mpsc::channel(4);
            let (requests, request_rx) = watch::channel(0);
            let (_, rates) = mpsc::channel(4);
            let (sent, sent_rx) = watch::channel(None);
            let (repairs, repair_rx) = watch::channel(None);
            let (roi, roi_rx) = watch::channel(Vec::new());
            let forced = Arc::new(AtomicU64::new(0));
            let rois = Arc::new(Mutex::new(Vec::new()));
            let encoder = TestEncoder {
                output: output_tx.clone(),
                forced: forced.clone(),
                rois: Arc::clone(&rois),
                keyframe: false,
            };
            let task = tokio::spawn(run(
                Box::new(encoder),
                output_tx,
                frame_rx,
                request_rx,
                rates,
                sent_rx,
                repair_rx,
                roi_rx,
                false,
                Arc::new(Mutex::new(HostStatsCollector::starting_at(0))),
            ));
            Self {
                frames,
                output,
                requests,
                sent,
                repairs,
                roi,
                rois,
                forced,
                task,
            }
        }

        async fn capture(&self) {
            self.frames
                .send(CapturedFrame {
                    display_id: 0,
                    width: 1,
                    height: 1,
                    capture_ts_us: rsp_core::clock::unix_micros(),
                    native: Arc::new(()),
                })
                .await
                .unwrap();
            settle().await;
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn settle() {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn source_overload_resumes_with_delta_and_preserves_queued_reference() {
        let mut test = Fixture::new();
        test.capture().await;
        for _ in 0..10 {
            test.capture().await;
        }
        assert_eq!(test.output.len(), 1);
        let reference = test.output.recv().await.unwrap();
        assert!(!reference.keyframe);
        test.capture().await;
        assert!(!test.output.recv().await.unwrap().keyframe);
        assert_eq!(test.forced.load(Relaxed), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn repair_drains_old_references_then_emits_exactly_one_keyframe() {
        let mut test = Fixture::new();
        test.capture().await;
        let failed_capture = rsp_core::clock::unix_micros();
        test.repairs.send_replace(Some(failed_capture));
        test.capture().await;
        assert_eq!(test.forced.load(Relaxed), 0);
        assert!(!test.output.recv().await.unwrap().keyframe);
        test.capture().await;
        let repair = test.output.recv().await.unwrap();
        assert!(repair.keyframe);
        assert_eq!(test.forced.load(Relaxed), 1);
        // A repeated notification for the same damaged reference must not
        // release the in-flight guard or schedule another repair.
        test.repairs.send_replace(Some(failed_capture));
        settle().await;
        test.sent
            .send_replace(Some((Instant::now(), repair.capture_ts_us)));
        settle().await;
        test.capture().await;
        assert!(!test.output.recv().await.unwrap().keyframe);
        assert_eq!(test.forced.load(Relaxed), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn queued_response_prevents_forcing_again_until_packetizer_completes() {
        let mut test = Fixture::new();
        test.requests.send_replace(1);
        test.capture().await;
        assert_eq!(test.forced.load(Relaxed), 1);
        tokio::time::advance(Duration::from_millis(500)).await;
        test.requests.send_replace(10);
        settle().await;
        assert_eq!(test.forced.load(Relaxed), 1);
        let key = test.output.recv().await.unwrap();
        assert!(key.keyframe);
        test.sent
            .send_replace(Some((Instant::now(), key.capture_ts_us)));
        settle().await;
        tokio::time::advance(Duration::from_millis(299)).await;
        settle().await;
        assert_eq!(test.forced.load(Relaxed), 1);
        tokio::time::advance(Duration::from_millis(1)).await;
        settle().await;
        assert_eq!(test.forced.load(Relaxed), 2);
        test.capture().await;
        assert!(test.output.recv().await.unwrap().keyframe);
    }

    #[tokio::test(start_paused = true)]
    async fn roi_updates_are_forwarded_in_order() {
        let test = Fixture::new();
        let first = RoiRegion {
            rect: crate::platform::PixelRect {
                x: 0,
                y: 0,
                w: 64,
                h: 64,
            },
            qp_delta: -6,
        };
        test.roi.send_replace(vec![first]);
        settle().await;
        test.roi.send_replace(Vec::new());
        settle().await;
        assert_eq!(
            *test.rois.lock().unwrap_or_else(|p| p.into_inner()),
            vec![vec![first], Vec::new()]
        );
    }
}
