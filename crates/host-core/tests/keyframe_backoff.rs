use host_core::pipeline::policy::{
    KeyframeGate, KEYFRAME_MAX_INTERVAL_US, KEYFRAME_MIN_INTERVAL_US, KEYFRAME_RECOVERY_US,
};

// Explicit microsecond timestamps are a fake clock: no wall-clock scheduling
// participates in these recovery-policy assertions.
#[test]
fn repeated_recovery_grows_to_two_seconds_and_quiet_resets_it() {
    let mut gate = KeyframeGate::with_backoff();
    assert!(gate.request(0));
    gate.mark_in_flight();
    gate.on_response(0);
    let mut now = 0;
    for interval in [300_000, 600_000, 1_200_000, 2_000_000, 2_000_000] {
        assert_eq!(gate.interval_us(), interval);
        let due = now + interval;
        // Keep requesting through each recovery window, as a still-frozen
        // receiver would. All these requests must share one response.
        for request in (now + 100_000..due).step_by(100_000) {
            assert!(!gate.request(request));
        }
        assert_eq!(gate.deadline(), Some(due));
        assert!(!gate.poll(due - 1));
        assert!(gate.poll(due));
        gate.mark_in_flight();
        gate.on_response(due);
        assert!(!gate.poll(due + 1));
        now = due;
    }
    assert_eq!(gate.interval_us(), KEYFRAME_MAX_INTERVAL_US);
    now += KEYFRAME_RECOVERY_US;
    assert!(gate.request(now));
    assert_eq!(gate.interval_us(), KEYFRAME_MIN_INTERVAL_US);
}

#[test]
fn slow_encoder_and_queued_keyframe_do_not_get_duplicate_forces() {
    let mut gate = KeyframeGate::with_backoff();
    assert!(gate.request(0));
    gate.mark_in_flight();
    for now in (100_000..=3_000_000).step_by(100_000) {
        assert!(!gate.request(now));
        assert!(!gate.poll(now));
        assert_eq!(gate.deadline(), None, "do not spin on an expired timer");
    }
    gate.on_response(3_000_000);
    assert_eq!(gate.deadline(), Some(3_300_000));
    assert!(gate.poll(3_300_000));
    assert!(!gate.poll(6_000_000));
}

#[test]
fn single_loss_after_an_opening_or_periodic_frame_keeps_300ms_recovery() {
    for request_at in [0, 1, 100_000, 299_999, 300_000, 900_000] {
        let mut gate = KeyframeGate::with_backoff();
        gate.on_keyframe(0);
        if !gate.request(request_at) {
            let due = gate.deadline().unwrap();
            assert!(due - request_at <= KEYFRAME_MIN_INTERVAL_US);
            assert!(gate.poll(due));
        }
        assert_eq!(gate.interval_us(), KEYFRAME_MIN_INTERVAL_US);
    }
}

#[test]
fn broken_reference_chain_merges_repairs_until_backlog_drains() {
    let mut gate = KeyframeGate::with_backoff();
    gate.on_keyframe(0);
    for now in [10_000, 20_000, 30_000] {
        gate.encode_failed(now);
    }
    // The encoder does not poll the repair gate until earlier references have
    // drained. At that point only one forced keyframe is needed.
    assert!(gate.pending());
    assert!(gate.poll(300_000));
    gate.mark_in_flight();
    assert!(!gate.poll(900_000));
    gate.on_response(900_000);
    assert!(!gate.pending());
    assert!(!gate.poll(2_000_000));
}

#[test]
fn failed_forced_encode_releases_inflight_but_keeps_backoff() {
    let mut gate = KeyframeGate::with_backoff();
    assert!(gate.request(0));
    gate.mark_in_flight();
    gate.encode_failed(10_000);
    assert!(!gate.poll(299_999));
    assert!(gate.poll(300_000));
    gate.mark_in_flight();
    gate.encode_failed(310_000);
    assert_eq!(gate.deadline(), Some(900_000));
    assert!(gate.poll(900_000));
    gate.on_response(900_000);
    gate.encode_failed(900_000 + KEYFRAME_RECOVERY_US);
    assert_eq!(gate.interval_us(), KEYFRAME_MIN_INTERVAL_US);
    assert!(gate.poll(900_000 + KEYFRAME_RECOVERY_US));
}

#[test]
fn rebuild_clears_backoff_and_pending_requests() {
    let mut gate = KeyframeGate::with_backoff();
    assert!(gate.request(0));
    assert!(gate.request(300_000));
    assert!(gate.interval_us() > KEYFRAME_MIN_INTERVAL_US);
    gate.mark_in_flight();
    gate.defer();
    gate.reset();
    assert_eq!(gate.deadline(), None);
    assert_eq!(gate.interval_us(), KEYFRAME_MIN_INTERVAL_US);
    assert!(gate.request(300_001));
}

#[test]
fn remote_loss_keeps_300ms_even_after_a_local_repair_burst() {
    let mut gate = KeyframeGate::with_backoff();
    assert!(gate.request(0));
    assert!(gate.request(300_000));
    assert_eq!(gate.interval_us(), 600_000);
    gate.on_response(300_000);
    // The wire path has no local repair outstanding. Continuing remote loss
    // must not inherit a two-second interval and starve a decodable picture.
    gate.set_backoff(false);
    assert!(gate.request(600_000));
    assert_eq!(gate.interval_us(), KEYFRAME_MIN_INTERVAL_US);
    assert!(gate.request(900_000));
    assert!(gate.request(1_200_000));
}

#[test]
fn time_spent_waiting_for_a_failed_response_is_not_network_recovery() {
    let mut gate = KeyframeGate::default();
    gate.encode_failed(0);
    let mut now = 0;
    for interval in [300_000, 600_000, 1_200_000, 2_000_000, 2_000_000, 2_000_000] {
        assert!(gate.poll(now));
        gate.mark_in_flight();
        assert_eq!(gate.interval_us(), interval);
        now += interval;
        gate.encode_failed(now);
    }
    assert_eq!(gate.interval_us(), KEYFRAME_MAX_INTERVAL_US);
}
