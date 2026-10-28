use host_core::inject::scroll_gesture::{
    GestureState, MacScrollEvent, ScrollGesture, ScrollStats, GESTURE_IDLE_END_US,
    MOMENTUM_IDLE_END_US,
};
use rsp_core::hid::{InputMsg, ScrollPhase, ScrollUnit};

const BASE_US: u64 = 1_000_000;
const FRAME_US: u64 = 16_000;

fn g(phase: ScrollPhase, dy: f32) -> InputMsg {
    scroll(0.0, dy, phase, ScrollPhase::None, ScrollUnit::Pixel)
}

fn m(phase: ScrollPhase, dy: f32) -> InputMsg {
    scroll(0.0, dy, ScrollPhase::None, phase, ScrollUnit::Pixel)
}

fn w(dy: f32) -> InputMsg {
    scroll(
        0.0,
        dy,
        ScrollPhase::None,
        ScrollPhase::None,
        ScrollUnit::Line,
    )
}

fn wheel_pixels(dy: f32) -> InputMsg {
    scroll(
        0.0,
        dy,
        ScrollPhase::None,
        ScrollPhase::None,
        ScrollUnit::Pixel,
    )
}

fn scroll(
    dx: f32,
    dy: f32,
    phase: ScrollPhase,
    momentum: ScrollPhase,
    unit: ScrollUnit,
) -> InputMsg {
    InputMsg::Scroll {
        dx,
        dy,
        phase,
        momentum,
        unit,
    }
}

#[test]
fn complete_trackpad_gesture_and_inertia_pass_through_unchanged() {
    let mut gesture = ScrollGesture::new(10.0);
    let mut inputs = vec![g(ScrollPhase::Began, 1.0)];
    inputs.extend((0..5).map(|_| g(ScrollPhase::Changed, 1.0)));
    inputs.push(g(ScrollPhase::Ended, 1.0));
    inputs.push(m(ScrollPhase::Began, 1.0));
    inputs.extend((0..10).map(|_| m(ScrollPhase::Changed, 1.0)));
    inputs.push(m(ScrollPhase::Ended, 1.0));

    let mut output = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        output.extend(gesture.on_scroll(input, BASE_US + index as u64 * FRAME_US));
    }

    assert_eq!(output.len(), inputs.len());
    let expected_phases = [(1, 0)]
        .into_iter()
        .chain((0..5).map(|_| (2, 0)))
        .chain([(4, 0), (0, 1)])
        .chain((0..10).map(|_| (0, 2)))
        .chain([(0, 3)])
        .collect::<Vec<_>>();
    for (event, (scroll_phase, momentum_phase)) in output.iter().zip(expected_phases) {
        assert_eq!(event.scroll.scroll_phase, scroll_phase);
        assert_eq!(event.scroll.momentum_phase, momentum_phase);
        assert!(!event.synthetic);
        assert!(event.continuous);
        assert_eq!(event.scroll.dy, 1);
    }
    assert_eq!(gesture.state(), GestureState::Idle);
    assert_eq!(gesture.stats(), ScrollStats::default());
    assert_eq!(gesture.next_deadline(), None);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExpectedEvent {
    scroll_phase: u32,
    momentum_phase: u32,
    synthetic: bool,
    continuous: bool,
    dy: i32,
}

const fn synthetic(scroll_phase: u32, momentum_phase: u32) -> ExpectedEvent {
    ExpectedEvent {
        scroll_phase,
        momentum_phase,
        synthetic: true,
        continuous: true,
        dy: 0,
    }
}

const fn real(scroll_phase: u32, momentum_phase: u32, continuous: bool, dy: i32) -> ExpectedEvent {
    ExpectedEvent {
        scroll_phase,
        momentum_phase,
        synthetic: false,
        continuous,
        dy,
    }
}

const SG_BEGAN: ExpectedEvent = synthetic(1, 0);
const SG_ENDED: ExpectedEvent = synthetic(4, 0);
const SM_BEGIN: ExpectedEvent = synthetic(0, 1);
const SM_END: ExpectedEvent = synthetic(0, 3);

const REAL_G_BEGAN: ExpectedEvent = real(1, 0, true, 2);
const REAL_G_CHANGED: ExpectedEvent = real(2, 0, true, 2);
const REAL_G_ENDED: ExpectedEvent = real(4, 0, true, 2);
const REAL_M_BEGAN: ExpectedEvent = real(0, 1, true, 2);
const REAL_M_CHANGED: ExpectedEvent = real(0, 2, true, 2);
const REAL_M_ENDED: ExpectedEvent = real(0, 3, true, 2);
const REAL_WHEEL: ExpectedEvent = real(0, 0, false, 20);

fn machine_in(state: GestureState) -> ScrollGesture {
    let mut gesture = ScrollGesture::new(10.0);
    match state {
        GestureState::Idle => {}
        GestureState::Scrolling => {
            let output = gesture.on_scroll(&g(ScrollPhase::Began, 0.0), BASE_US);
            assert_eq!(output.len(), 1);
            assert!(!output[0].synthetic);
        }
        GestureState::Momentum => {
            let output = gesture.on_scroll(&m(ScrollPhase::Began, 0.0), BASE_US);
            assert_eq!(output.len(), 1);
            assert!(!output[0].synthetic);
        }
    }
    assert_eq!(gesture.state(), state);
    assert_eq!(gesture.stats(), ScrollStats::default());
    gesture
}

fn assert_transition(
    current: GestureState,
    input: InputMsg,
    expected: &[ExpectedEvent],
    next: GestureState,
    dropped: u64,
) {
    let mut gesture = machine_in(current);
    let output = gesture.on_scroll(&input, BASE_US + FRAME_US);

    assert_eq!(output.len(), expected.len(), "output: {output:#?}");
    for (event, expected) in output.iter().zip(expected) {
        assert_eq!(
            event.scroll.scroll_phase, expected.scroll_phase,
            "event: {event:#?}"
        );
        assert_eq!(
            event.scroll.momentum_phase, expected.momentum_phase,
            "event: {event:#?}"
        );
        assert_eq!(event.synthetic, expected.synthetic, "event: {event:#?}");
        assert_eq!(event.continuous, expected.continuous, "event: {event:#?}");
        assert!(event.scroll.units_pixel);
        assert_eq!(event.scroll.dx, 0, "event: {event:#?}");
        assert_eq!(event.scroll.dy, expected.dy, "event: {event:#?}");
    }

    assert_eq!(gesture.state(), next);
    let synthesized = expected.iter().filter(|event| event.synthetic).count() as u64;
    assert_eq!(
        gesture.stats(),
        ScrollStats {
            dropped,
            synthesized,
        }
    );
}

macro_rules! transition_test {
    ($name:ident, $current:expr, $input:expr, [$($expected:expr),* $(,)?], $next:expr, $dropped:expr) => {
        #[test]
        fn $name() {
            assert_transition(
                $current,
                $input,
                &[$($expected),*],
                $next,
                $dropped,
            );
        }
    };
}

transition_test!(
    idle_wheel,
    GestureState::Idle,
    w(2.0),
    [REAL_WHEEL],
    GestureState::Idle,
    0
);
transition_test!(
    idle_gesture_began,
    GestureState::Idle,
    g(ScrollPhase::Began, 2.0),
    [REAL_G_BEGAN],
    GestureState::Scrolling,
    0
);
transition_test!(
    idle_gesture_changed,
    GestureState::Idle,
    g(ScrollPhase::Changed, 2.0),
    [SG_BEGAN, REAL_G_CHANGED],
    GestureState::Scrolling,
    0
);
transition_test!(
    idle_gesture_ended,
    GestureState::Idle,
    g(ScrollPhase::Ended, 2.0),
    [],
    GestureState::Idle,
    1
);
transition_test!(
    idle_momentum_began,
    GestureState::Idle,
    m(ScrollPhase::Began, 2.0),
    [REAL_M_BEGAN],
    GestureState::Momentum,
    0
);
transition_test!(
    idle_momentum_changed,
    GestureState::Idle,
    m(ScrollPhase::Changed, 2.0),
    [SM_BEGIN, REAL_M_CHANGED],
    GestureState::Momentum,
    0
);
transition_test!(
    idle_momentum_ended,
    GestureState::Idle,
    m(ScrollPhase::Ended, 2.0),
    [],
    GestureState::Idle,
    1
);

transition_test!(
    scrolling_gesture_changed,
    GestureState::Scrolling,
    g(ScrollPhase::Changed, 2.0),
    [REAL_G_CHANGED],
    GestureState::Scrolling,
    0
);
transition_test!(
    scrolling_gesture_ended,
    GestureState::Scrolling,
    g(ScrollPhase::Ended, 2.0),
    [REAL_G_ENDED],
    GestureState::Idle,
    0
);
transition_test!(
    scrolling_gesture_began,
    GestureState::Scrolling,
    g(ScrollPhase::Began, 2.0),
    [SG_ENDED, REAL_G_BEGAN],
    GestureState::Scrolling,
    0
);
transition_test!(
    scrolling_wheel,
    GestureState::Scrolling,
    w(2.0),
    [SG_ENDED, REAL_WHEEL],
    GestureState::Idle,
    0
);
transition_test!(
    scrolling_momentum_began,
    GestureState::Scrolling,
    m(ScrollPhase::Began, 2.0),
    [SG_ENDED, REAL_M_BEGAN],
    GestureState::Momentum,
    0
);
transition_test!(
    scrolling_momentum_changed,
    GestureState::Scrolling,
    m(ScrollPhase::Changed, 2.0),
    [SG_ENDED, SM_BEGIN, REAL_M_CHANGED],
    GestureState::Momentum,
    0
);
transition_test!(
    scrolling_momentum_ended,
    GestureState::Scrolling,
    m(ScrollPhase::Ended, 2.0),
    [SG_ENDED],
    GestureState::Idle,
    1
);

transition_test!(
    momentum_momentum_changed,
    GestureState::Momentum,
    m(ScrollPhase::Changed, 2.0),
    [REAL_M_CHANGED],
    GestureState::Momentum,
    0
);
transition_test!(
    momentum_momentum_ended,
    GestureState::Momentum,
    m(ScrollPhase::Ended, 2.0),
    [REAL_M_ENDED],
    GestureState::Idle,
    0
);
transition_test!(
    momentum_momentum_began,
    GestureState::Momentum,
    m(ScrollPhase::Began, 2.0),
    [SM_END, REAL_M_BEGAN],
    GestureState::Momentum,
    0
);
transition_test!(
    momentum_gesture_began,
    GestureState::Momentum,
    g(ScrollPhase::Began, 2.0),
    [SM_END, REAL_G_BEGAN],
    GestureState::Scrolling,
    0
);
transition_test!(
    momentum_gesture_changed,
    GestureState::Momentum,
    g(ScrollPhase::Changed, 2.0),
    [SM_END, SG_BEGAN, REAL_G_CHANGED],
    GestureState::Scrolling,
    0
);
transition_test!(
    momentum_gesture_ended,
    GestureState::Momentum,
    g(ScrollPhase::Ended, 2.0),
    [SM_END],
    GestureState::Idle,
    1
);
transition_test!(
    momentum_wheel,
    GestureState::Momentum,
    w(2.0),
    [SM_END, REAL_WHEEL],
    GestureState::Idle,
    0
);

#[test]
fn gesture_timeout_closes_only_at_the_deadline_and_clock_rollback_counts_as_zero() {
    let mut gesture = ScrollGesture::new(10.0);
    let output = gesture.on_scroll(&g(ScrollPhase::Began, 1.0), BASE_US);
    assert_eq!(output.len(), 1);
    assert_eq!(gesture.next_deadline(), Some(BASE_US + GESTURE_IDLE_END_US));

    assert!(gesture.poll(BASE_US - 1).is_empty());
    assert!(gesture.poll(BASE_US + 299_000).is_empty());
    assert_eq!(gesture.state(), GestureState::Scrolling);

    let changed_at = BASE_US + 299_001;
    let output = gesture.on_scroll(&g(ScrollPhase::Changed, 1.0), changed_at);
    assert_eq!(output.len(), 1);
    assert_eq!(
        gesture.next_deadline(),
        Some(changed_at + GESTURE_IDLE_END_US)
    );
    assert!(gesture.poll(changed_at + 299_999).is_empty());

    let output = gesture.poll(changed_at + GESTURE_IDLE_END_US);
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].scroll.scroll_phase, 4);
    assert_eq!(output[0].scroll.momentum_phase, 0);
    assert!(output[0].synthetic);
    assert_eq!(gesture.state(), GestureState::Idle);
    assert_eq!(gesture.next_deadline(), None);
    assert_eq!(
        gesture.stats(),
        ScrollStats {
            dropped: 0,
            synthesized: 1,
        }
    );
}

#[test]
fn momentum_timeout_closes_only_at_the_deadline() {
    let mut gesture = ScrollGesture::new(10.0);
    let began_at = BASE_US + 40_000;
    gesture.on_scroll(&m(ScrollPhase::Began, 1.0), began_at);
    assert_eq!(
        gesture.next_deadline(),
        Some(began_at + MOMENTUM_IDLE_END_US)
    );
    assert!(gesture.poll(began_at + 299_000).is_empty());

    let changed_at = began_at + 299_001;
    gesture.on_scroll(&m(ScrollPhase::Changed, 1.0), changed_at);
    assert_eq!(
        gesture.next_deadline(),
        Some(changed_at + MOMENTUM_IDLE_END_US)
    );
    assert!(gesture.poll(changed_at + 299_999).is_empty());

    let output = gesture.poll(changed_at + MOMENTUM_IDLE_END_US);
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].scroll.scroll_phase, 0);
    assert_eq!(output[0].scroll.momentum_phase, 3);
    assert!(output[0].synthetic);
    assert_eq!(gesture.state(), GestureState::Idle);
    assert_eq!(gesture.next_deadline(), None);
}

#[test]
fn reset_closes_each_active_phase() {
    let mut scrolling = ScrollGesture::new(10.0);
    scrolling.on_scroll(&g(ScrollPhase::Began, 1.0), BASE_US);
    let output = scrolling.reset();
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].scroll.scroll_phase, 4);
    assert_eq!(output[0].scroll.momentum_phase, 0);
    assert!(output[0].synthetic);
    assert_eq!(scrolling.state(), GestureState::Idle);
    assert_eq!(scrolling.next_deadline(), None);

    let mut momentum = ScrollGesture::new(10.0);
    momentum.on_scroll(&m(ScrollPhase::Began, 1.0), BASE_US);
    let output = momentum.reset();
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].scroll.scroll_phase, 0);
    assert_eq!(output[0].scroll.momentum_phase, 3);
    assert!(output[0].synthetic);
    assert_eq!(momentum.state(), GestureState::Idle);

    let mut idle = ScrollGesture::new(10.0);
    assert!(idle.reset().is_empty());
    assert_eq!(idle.state(), GestureState::Idle);
}

#[test]
fn reset_clears_fractional_pixels() {
    let mut reset_gesture = ScrollGesture::new(10.0);
    let began = reset_gesture.on_scroll(&g(ScrollPhase::Began, 0.4), BASE_US);
    let changed = reset_gesture.on_scroll(&g(ScrollPhase::Changed, 0.4), BASE_US + FRAME_US);
    assert_eq!((began[0].scroll.dy, changed[0].scroll.dy), (0, 0));
    let reset = reset_gesture.reset();
    assert_eq!(reset.len(), 1);
    assert_eq!(reset[0].scroll.scroll_phase, 4);

    let wheel = reset_gesture.on_scroll(&wheel_pixels(0.4), BASE_US + 2 * FRAME_US);
    assert_eq!(wheel.len(), 1);
    assert_eq!(wheel[0].scroll.dy, 0);

    let mut retained = ScrollGesture::new(10.0);
    retained.on_scroll(&g(ScrollPhase::Began, 0.4), BASE_US);
    retained.on_scroll(&g(ScrollPhase::Changed, 0.4), BASE_US + FRAME_US);
    let wheel = retained.on_scroll(&wheel_pixels(0.4), BASE_US + 2 * FRAME_US);
    assert_eq!(wheel.len(), 2);
    assert!(wheel[0].synthetic);
    assert_eq!(wheel[1].scroll.dy, 1);
}

#[test]
fn fractional_pixels_survive_gesture_to_momentum_handoff() {
    let mut gesture = ScrollGesture::new(10.0);
    let mut output = Vec::new();
    for (index, input) in [
        g(ScrollPhase::Began, 0.6),
        g(ScrollPhase::Changed, 0.6),
        g(ScrollPhase::Ended, 0.6),
        m(ScrollPhase::Began, 0.6),
    ]
    .iter()
    .enumerate()
    {
        output.extend(gesture.on_scroll(input, BASE_US + index as u64 * FRAME_US));
    }
    assert_eq!(output.len(), 4);
    assert_eq!(
        output
            .iter()
            .map(|event| event.scroll.dy)
            .collect::<Vec<_>>(),
        [0, 1, 0, 1]
    );
    assert!(output.iter().all(|event| !event.synthetic));
    assert_eq!(gesture.state(), GestureState::Momentum);
}

#[test]
fn synthetic_events_do_not_consume_fractional_pixels() {
    let mut gesture = ScrollGesture::new(10.0);
    gesture.on_scroll(&g(ScrollPhase::Began, 0.6), BASE_US);
    let output = gesture.on_scroll(&m(ScrollPhase::Began, 0.6), BASE_US + FRAME_US);

    assert_eq!(output.len(), 2);
    assert!(output[0].synthetic);
    assert_eq!(output[0].scroll.scroll_phase, 4);
    assert_eq!((output[0].scroll.dx, output[0].scroll.dy), (0, 0));
    assert!(!output[1].synthetic);
    assert_eq!(output[1].scroll.momentum_phase, 1);
    assert_eq!(output[1].scroll.dy, 1);
    assert_eq!(gesture.state(), GestureState::Momentum);
}

#[test]
fn line_wheel_scrolls_in_pixels_and_is_not_continuous() {
    let mut gesture = ScrollGesture::new(10.0);
    let output = gesture.on_scroll(&w(-3.0), BASE_US);

    assert_eq!(output.len(), 1);
    let event = output[0];
    assert_eq!(event.scroll.dy, -30);
    assert_eq!(event.scroll.dx, 0);
    assert!(event.scroll.units_pixel);
    assert_eq!(event.scroll.scroll_phase, 0);
    assert_eq!(event.scroll.momentum_phase, 0);
    assert!(!event.continuous);
    assert!(!event.synthetic);
    assert_eq!(gesture.state(), GestureState::Idle);
    assert_eq!(gesture.stats(), ScrollStats::default());
}

#[test]
fn invalid_inputs_are_dropped_without_changing_state_or_deadline() {
    let mut gesture = machine_in(GestureState::Scrolling);
    let deadline = gesture.next_deadline();
    let invalid = [
        scroll(
            0.0,
            1.0,
            ScrollPhase::Began,
            ScrollPhase::Changed,
            ScrollUnit::Pixel,
        ),
        InputMsg::ReleaseAll,
        g(ScrollPhase::Changed, f32::NAN),
        wheel_pixels(f32::INFINITY),
    ];

    for (index, input) in invalid.iter().enumerate() {
        assert!(gesture
            .on_scroll(input, BASE_US + (index as u64 + 1) * FRAME_US)
            .is_empty());
        assert_eq!(gesture.state(), GestureState::Scrolling);
        assert_eq!(gesture.next_deadline(), deadline);
    }
    assert_eq!(
        gesture.stats(),
        ScrollStats {
            dropped: invalid.len() as u64,
            synthesized: 0,
        }
    );
}

#[test]
fn disconnect_closes_the_old_gesture_and_reopens_a_missing_begin() {
    let mut gesture = ScrollGesture::new(10.0);
    let began_at = BASE_US;
    gesture.on_scroll(&g(ScrollPhase::Began, 1.0), began_at);
    gesture.on_scroll(&g(ScrollPhase::Changed, 1.0), began_at + FRAME_US);

    let output = gesture.poll(began_at + FRAME_US + GESTURE_IDLE_END_US);
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].scroll.scroll_phase, 4);
    assert!(output[0].synthetic);
    assert_eq!(gesture.state(), GestureState::Idle);

    let new_session_at = began_at + FRAME_US + GESTURE_IDLE_END_US + 1;
    let output = gesture.on_scroll(&g(ScrollPhase::Changed, 1.0), new_session_at);
    assert_eq!(output.len(), 2);
    assert!(output[0].synthetic);
    assert_eq!(output[0].scroll.scroll_phase, 1);
    assert!(!output[1].synthetic);
    assert_eq!(output[1].scroll.scroll_phase, 2);
    assert_eq!(output[1].scroll.dy, 1);
    assert_eq!(gesture.state(), GestureState::Scrolling);
}

#[derive(Debug, Default)]
struct SequenceChecker {
    gesture_active: bool,
    momentum_active: bool,
    synthetic: u64,
}

impl SequenceChecker {
    fn check(&mut self, event: &MacScrollEvent) {
        let scroll = event.scroll;
        assert!(scroll.units_pixel, "event: {event:#?}");
        assert!(!(self.gesture_active && self.momentum_active));
        if event.synthetic {
            self.synthetic += 1;
            assert!(event.continuous, "event: {event:#?}");
            assert_eq!((scroll.dx, scroll.dy), (0, 0), "event: {event:#?}");
        }
        assert!(
            scroll.scroll_phase == 0 || scroll.momentum_phase == 0,
            "event: {event:#?}"
        );

        match scroll.scroll_phase {
            0 => {}
            1 => {
                assert!(!self.gesture_active, "nested gesture begin: {event:#?}");
                assert!(
                    !self.momentum_active,
                    "gesture overlapped momentum: {event:#?}"
                );
                self.gesture_active = true;
            }
            2 => assert!(
                self.gesture_active,
                "gesture change before begin: {event:#?}"
            ),
            4 => {
                assert!(self.gesture_active, "gesture end before begin: {event:#?}");
                self.gesture_active = false;
            }
            other => panic!("unexpected scroll phase {other}: {event:#?}"),
        }

        match scroll.momentum_phase {
            0 => {}
            1 => {
                assert!(!self.momentum_active, "nested momentum begin: {event:#?}");
                assert!(
                    !self.gesture_active,
                    "momentum overlapped gesture: {event:#?}"
                );
                self.momentum_active = true;
            }
            2 => assert!(
                self.momentum_active,
                "momentum continue before begin: {event:#?}"
            ),
            3 => {
                assert!(
                    self.momentum_active,
                    "momentum end before begin: {event:#?}"
                );
                self.momentum_active = false;
            }
            other => panic!("unexpected momentum phase {other}: {event:#?}"),
        }

        assert!(!(self.gesture_active && self.momentum_active));
    }

    fn check_all(&mut self, events: &[MacScrollEvent]) {
        for event in events {
            self.check(event);
        }
    }
}

fn xorshift64(state: &mut u64) -> u64 {
    let mut value = *state;
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    *state = value;
    value
}

fn random_delta(random: &mut u64) -> f32 {
    ((xorshift64(random) % 25) as i32 - 12) as f32 / 4.0
}

fn random_message(random: &mut u64) -> InputMsg {
    let dy = random_delta(random);
    match xorshift64(random) % 10 {
        0 => w(dy),
        1 => g(ScrollPhase::Began, dy),
        2 => g(ScrollPhase::Changed, dy),
        3 => g(ScrollPhase::Ended, dy),
        4 => m(ScrollPhase::Began, dy),
        5 => m(ScrollPhase::Changed, dy),
        6 => m(ScrollPhase::Ended, dy),
        7 => scroll(
            0.0,
            dy,
            ScrollPhase::Changed,
            ScrollPhase::Began,
            ScrollUnit::Pixel,
        ),
        8 => InputMsg::ReleaseAll,
        _ => g(ScrollPhase::Changed, f32::NAN),
    }
}

#[test]
fn mixed_random_stream_always_forms_legal_sequences() {
    let mut gesture = ScrollGesture::new(10.0);
    let mut checker = SequenceChecker::default();
    let mut random = 0x9e37_79b9_7f4a_7c15_u64;
    let mut now_us = BASE_US;

    for _ in 0..5_000 {
        now_us += xorshift64(&mut random) % 400_001;
        checker.check_all(&gesture.poll(now_us));

        let input = random_message(&mut random);
        checker.check_all(&gesture.on_scroll(&input, now_us));
        assert!(!(checker.gesture_active && checker.momentum_active));
    }

    let output = gesture.reset();
    checker.check_all(&output);
    assert!(!checker.gesture_active);
    assert!(!checker.momentum_active);
    assert_eq!(gesture.state(), GestureState::Idle);
    assert_eq!(gesture.next_deadline(), None);
    assert_eq!(gesture.stats().synthesized, checker.synthetic);
    assert!(gesture.stats().dropped > 0);
    assert!(checker.synthetic > 0);
}
