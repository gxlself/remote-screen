use host_core::pipeline::policy::{
    codec_order, CodecFallback, KeyframeGate, StillRefiner, KEYFRAME_MIN_INTERVAL_US, STILL_IDLE_US,
};
use rsp_core::pb;

#[test]
fn still_refiner_refines_once_after_idle() {
    let mut refiner = StillRefiner::new(true);
    refiner.on_frame(0);
    refiner.on_frame(16_000);
    refiner.on_frame(33_000);

    assert!(!refiner.poll(332_999));
    assert!(refiner.poll(333_000));
    assert!(!refiner.poll(400_000));

    let mut refinements = 1;
    for now_us in (333_010..=10_000_000).step_by(10_000) {
        if refiner.poll(now_us) {
            refinements += 1;
        }
    }
    assert_eq!(refinements, 1);
    assert_eq!(refiner.deadline(), None);
}

#[test]
fn still_refiner_waits_for_cooldown_after_motion_resumes() {
    let mut refiner = StillRefiner::new(true);
    refiner.on_frame(0);
    refiner.on_frame(16_000);
    refiner.on_frame(33_000);
    assert!(refiner.poll(333_000));

    refiner.on_frame(500_000);
    assert!(!refiner.poll(800_000));
    assert_eq!(refiner.deadline(), Some(1_333_000));
    assert!(!refiner.poll(1_332_999));
    assert!(refiner.poll(1_333_000));
    assert!(!refiner.poll(1_333_001));
}

#[test]
fn still_refiner_never_refines_while_frames_keep_arriving() {
    let mut refiner = StillRefiner::new(true);
    for step in 0..=125 {
        let now_us = step * 16_000;
        refiner.on_frame(now_us);
        assert!(!refiner.poll(now_us));
    }
}

#[test]
fn still_refiner_honours_disable_and_reset() {
    let mut refiner = StillRefiner::new(true);
    refiner.on_frame(0);
    refiner.set_enabled(false);
    for now_us in (0..=2_000_000).step_by(10_000) {
        assert!(!refiner.poll(now_us));
    }
    assert_eq!(refiner.deadline(), None);

    refiner.set_enabled(true);
    assert!(refiner.poll(300_000));
    refiner.reset();
    assert_eq!(refiner.deadline(), None);
    assert!(!refiner.poll(10_000_000));
}

#[test]
fn still_refiner_without_frames_has_no_deadline() {
    let refiner = StillRefiner::new(true);
    assert_eq!(refiner.deadline(), None);

    let mut refiner = refiner;
    assert!(!refiner.poll(STILL_IDLE_US));
}

#[test]
fn keyframe_gate_immediately_responds_then_coalesces_requests() {
    let mut gate = KeyframeGate::default();
    assert!(gate.request(0));
    assert!(!gate.request(100_000));
    assert!(!gate.request(200_000));
    assert_eq!(gate.deadline(), Some(KEYFRAME_MIN_INTERVAL_US));
    assert!(!gate.poll(299_999));
    assert!(gate.poll(300_000));
    assert_eq!(gate.deadline(), None);
    assert!(!gate.poll(300_001));
}

#[test]
fn keyframe_gate_allows_request_after_window() {
    let mut gate = KeyframeGate::default();
    assert!(gate.request(0));
    assert!(!gate.request(100_000));
    assert!(gate.poll(300_000));

    assert!(gate.request(700_000));
}

#[test]
fn keyframe_gate_treats_external_keyframe_as_response() {
    let mut gate = KeyframeGate::default();
    gate.on_keyframe(50_000);
    assert!(!gate.request(100_000));
    assert_eq!(gate.deadline(), Some(350_000));
    assert!(gate.poll(350_000));
}

#[test]
fn keyframe_gate_external_keyframe_clears_pending_request() {
    let mut gate = KeyframeGate::default();
    assert!(gate.request(0));
    assert!(!gate.request(100_000));

    gate.on_keyframe(200_000);
    assert_eq!(gate.deadline(), None);
    assert!(!gate.poll(500_000));
}

#[test]
fn keyframe_gate_reset_clears_response_and_pending_state() {
    let mut gate = KeyframeGate::default();
    assert!(gate.request(0));
    assert!(!gate.request(100_000));

    gate.reset();
    assert_eq!(gate.deadline(), None);
    assert!(gate.request(100_000));
}

#[test]
fn codec_order_keeps_app_preference_and_defaults_to_hevc() {
    assert_eq!(
        codec_order(&[
            pb::Codec::H264,
            pb::Codec::Hevc,
            pb::Codec::H264,
            pb::Codec::Unspecified,
        ]),
        vec![pb::Codec::H264, pb::Codec::Hevc]
    );
    assert_eq!(codec_order(&[]), vec![pb::Codec::Hevc]);
    assert_eq!(
        codec_order(&[pb::Codec::Unspecified]),
        vec![pb::Codec::Hevc]
    );
}

#[test]
fn codec_fallback_walks_app_order() {
    let mut fallback = CodecFallback::new(&[pb::Codec::Hevc, pb::Codec::H264], &[]);
    assert_eq!(fallback.current(), Some(pb::Codec::Hevc));
    assert_eq!(fallback.on_unsupported(), Some(pb::Codec::H264));
    assert_eq!(fallback.current(), Some(pb::Codec::H264));
    assert_eq!(fallback.on_unsupported(), None);
    assert_eq!(fallback.current(), None);
    assert_eq!(fallback.on_unsupported(), None);
}

#[test]
fn codec_fallback_skips_disabled_and_reports_no_choice() {
    let mut fallback = CodecFallback::new(&[pb::Codec::Hevc, pb::Codec::H264], &[pb::Codec::Hevc]);
    assert_eq!(fallback.current(), Some(pb::Codec::H264));
    assert_eq!(fallback.on_unsupported(), None);

    let fallback = CodecFallback::new(
        &[pb::Codec::Hevc, pb::Codec::H264],
        &[pb::Codec::Hevc, pb::Codec::H264],
    );
    assert_eq!(fallback.current(), None);
}
