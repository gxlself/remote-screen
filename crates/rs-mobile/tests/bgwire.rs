use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use rs_mobile::rscore::bgwire::{BgClock, BgDriver, BgFlow, BgTarget};
use rsp_client::recovery::background::BgState;
use tokio::time::Instant;

const FP: [u8; 32] = [3; 32];
const GRACE: Option<Duration> = Some(Duration::from_secs(30));

struct Target {
    calls: RefCell<Vec<&'static str>>,
    token: Option<Vec<u8>>,
}

impl Default for Target {
    fn default() -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            token: Some(vec![7; 16]),
        }
    }
}

impl Target {
    fn take(&self) -> Vec<&'static str> {
        self.calls.take()
    }
}

impl BgTarget for Target {
    fn release_all(&self) {
        self.calls.borrow_mut().push("release_all");
    }
    fn pause(&self) {
        self.calls.borrow_mut().push("pause");
    }
    fn resume(&self) {
        self.calls.borrow_mut().push("resume");
    }
    fn request_resume_keyframe(&self) {
        self.calls.borrow_mut().push("request_resume_keyframe");
    }
    fn close_background(&self) {
        self.calls.borrow_mut().push("close_background");
    }
    fn resume_token(&self) -> Option<Vec<u8>> {
        self.token.clone()
    }
}

#[test]
fn goodbye_at_thirty_seconds_and_resume_inside_window() {
    let t0 = Instant::now();
    let target = Target::default();
    let mut driver = BgDriver::new(GRACE, FP);
    assert_eq!(
        driver.set_background(true, t0, Some(&target)),
        BgFlow::Continue
    );
    assert_eq!(target.take(), ["release_all", "pause"]);
    assert_eq!(
        driver.poll(t0 + Duration::from_secs(29), Some(&target)),
        BgFlow::Continue
    );
    assert!(target.take().is_empty());
    assert_eq!(
        driver.poll(t0 + Duration::from_secs(30), Some(&target)),
        BgFlow::Closed
    );
    assert_eq!(target.take(), ["close_background"]);
    assert_eq!(
        driver.state(),
        BgState::Closed {
            deadline: t0 + Duration::from_secs(90)
        }
    );
    assert_eq!(
        driver.set_background(false, t0 + Duration::from_secs(50), None),
        BgFlow::Resume { token: [7; 16] }
    );
    // Consuming a token cannot issue another recovery action.
    assert_eq!(
        driver.set_background(false, t0 + Duration::from_secs(51), None),
        BgFlow::Continue
    );
}

#[test]
fn expiry_waits_for_foreground_before_returning_to_devices() {
    let t0 = Instant::now();
    let target = Target::default();
    let mut driver = BgDriver::new(GRACE, FP);
    driver.set_background(true, t0, Some(&target));
    driver.poll(t0 + Duration::from_secs(30), Some(&target));
    assert_eq!(
        driver.poll(t0 + Duration::from_secs(90), None),
        BgFlow::Continue
    );
    assert_eq!(
        driver.set_background(false, t0 + Duration::from_secs(91), None),
        BgFlow::ReturnToDevices
    );
    assert_eq!(target.take(), ["release_all", "pause", "close_background"]);
}

#[test]
fn late_foreground_wake_closes_once_and_immediately_resumes() {
    let t0 = Instant::now();
    let target = Target::default();
    let mut driver = BgDriver::new(GRACE, FP);
    driver.set_background(true, t0, Some(&target));
    assert_eq!(
        driver.set_background(false, t0 + Duration::from_secs(45), Some(&target)),
        BgFlow::Resume { token: [7; 16] }
    );
    assert_eq!(target.take(), ["release_all", "pause", "close_background"]);
}

#[test]
fn unexpected_loss_starts_a_new_sixty_second_window() {
    for (foreground_at, expected) in [
        (69, BgFlow::Resume { token: [7; 16] }),
        (71, BgFlow::ReturnToDevices),
    ] {
        let t0 = Instant::now();
        let target = Target::default();
        let mut driver = BgDriver::new(GRACE, FP);
        driver.set_background(true, t0, Some(&target));
        assert_eq!(
            driver.on_connection_lost(t0 + Duration::from_secs(10), Some(&target)),
            BgFlow::Closed
        );
        assert_eq!(
            driver.state(),
            BgState::Closed {
                deadline: t0 + Duration::from_secs(70)
            }
        );
        assert_eq!(
            driver.set_background(false, t0 + Duration::from_secs(foreground_at), None),
            expected
        );
        assert_eq!(target.take(), ["release_all", "pause"]);
    }
}

#[test]
fn background_hold_never_says_goodbye_and_can_still_recover_after_loss() {
    let t0 = Instant::now();
    let target = Target::default();
    let mut driver = BgDriver::new(None, FP);
    driver.set_background(true, t0, Some(&target));
    assert_eq!(
        driver.poll(t0 + Duration::from_secs(600), Some(&target)),
        BgFlow::Continue
    );
    assert_eq!(
        driver.set_background(false, t0 + Duration::from_secs(600), Some(&target)),
        BgFlow::Continue
    );
    assert_eq!(target.take(), ["release_all", "pause", "resume"]);
    driver.set_background(true, t0 + Duration::from_secs(601), Some(&target));
    assert_eq!(
        driver.on_connection_lost(t0 + Duration::from_secs(602), Some(&target)),
        BgFlow::Closed
    );
    assert_eq!(
        driver.set_background(false, t0 + Duration::from_secs(610), None),
        BgFlow::Resume { token: [7; 16] }
    );
}

#[test]
fn changing_grace_to_hold_prevents_goodbye() {
    let t0 = Instant::now();
    let target = Target::default();
    let mut driver = BgDriver::new(GRACE, FP);
    driver.set_background(true, t0, Some(&target));
    driver.set_grace(None, t0 + Duration::from_secs(10));
    assert_eq!(
        driver.poll(t0 + Duration::from_secs(600), Some(&target)),
        BgFlow::Continue
    );
    assert_eq!(target.take(), ["release_all", "pause"]);
}

#[test]
fn unchanged_grace_does_not_restart_the_pause_timer() {
    let t0 = Instant::now();
    let target = Target::default();
    let mut driver = BgDriver::new(GRACE, FP);
    driver.set_background(true, t0, Some(&target));
    driver.set_grace(GRACE, t0 + Duration::from_secs(29));
    assert_eq!(
        driver.poll(t0 + Duration::from_secs(30), Some(&target)),
        BgFlow::Closed
    );
}

#[test]
fn keyframe_watchdog_requests_once_unless_a_keyframe_arrived() {
    for keyframe in [false, true] {
        let t0 = Instant::now();
        let target = Target::default();
        let mut driver = BgDriver::new(GRACE, FP);
        driver.set_background(true, t0, Some(&target));
        let foreground_at = t0 + Duration::from_secs(10);
        driver.set_background(false, foreground_at, Some(&target));
        assert_eq!(target.take(), ["release_all", "pause", "resume"]);
        if keyframe {
            driver.on_keyframe(foreground_at + Duration::from_millis(200));
        }
        driver.poll(foreground_at + Duration::from_millis(500), Some(&target));
        driver.poll(foreground_at + Duration::from_secs(1), Some(&target));
        assert_eq!(
            target.take(),
            if keyframe {
                vec![]
            } else {
                vec!["request_resume_keyframe"]
            }
        );
    }
}

#[test]
fn missing_or_malformed_token_returns_to_devices() {
    for token in [None, Some(vec![7; 15])] {
        let t0 = Instant::now();
        let target = Target {
            token,
            ..Default::default()
        };
        let mut driver = BgDriver::new(GRACE, FP);
        driver.set_background(true, t0, Some(&target));
        driver.poll(t0 + Duration::from_secs(30), Some(&target));
        assert_eq!(
            driver.set_background(false, t0 + Duration::from_secs(50), None),
            BgFlow::ReturnToDevices
        );
    }
}

#[test]
fn next_wake_caps_background_checks_and_tracks_the_keyframe_deadline() {
    let t0 = Instant::now();
    for grace in [GRACE, None] {
        let target = Target::default();
        let mut driver = BgDriver::new(grace, FP);
        assert_eq!(driver.next_wake(t0), None);
        driver.set_background(true, t0, Some(&target));
        assert!(driver.next_wake(t0).unwrap() <= t0 + Duration::from_secs(1));
        driver.set_background(false, t0 + Duration::from_secs(10), Some(&target));
        assert_eq!(
            driver.next_wake(t0 + Duration::from_secs(10)),
            Some(t0 + Duration::from_millis(10_500))
        );
        driver.on_keyframe(t0 + Duration::from_millis(10_100));
        assert_eq!(driver.next_wake(t0 + Duration::from_millis(10_100)), None);
    }
}

#[test]
fn clock_tracks_injected_wall_time_and_saturates_backwards_adjustments() {
    let micros = Arc::new(AtomicU64::new(100_000_000));
    let clock = BgClock::new({
        let micros = micros.clone();
        Arc::new(move || micros.load(Ordering::Relaxed))
    });
    let base = clock.now();
    micros.fetch_add(31_000_000, Ordering::Relaxed);
    assert_eq!(clock.now(), base + Duration::from_secs(31));
    micros.store(0, Ordering::Relaxed);
    assert_eq!(clock.now(), base);
}
