use std::time::Duration;

use host_core::activity_model::{
    keyboard_is_ours, mouse_is_ours, ActivityState, KeepAwakeLatch, LLKHF_INJECTED, LLMHF_INJECTED,
    RS_EXTRA_INFO,
};

#[test]
fn activity_state_throttles_and_expires() {
    let mut state = ActivityState::default();
    assert_eq!(state.on_event(Duration::ZERO), Some(true));
    assert_eq!(state.on_event(Duration::from_millis(100)), None);
    assert_eq!(state.on_event(Duration::from_millis(250)), Some(true));
    assert_eq!(state.on_tick(Duration::from_millis(2_200)), None);
    assert_eq!(state.on_tick(Duration::from_millis(2_300)), Some(false));
    assert_eq!(state.on_tick(Duration::from_millis(2_301)), None);
    assert_eq!(state.on_event(Duration::from_secs(3)), Some(true));
}

#[test]
fn injected_event_filter_requires_marker_and_flag() {
    assert_eq!(RS_EXTRA_INFO, 0x5253);
    assert_eq!(LLKHF_INJECTED, 0x10);
    assert_eq!(LLMHF_INJECTED, 0x01);
    assert!(keyboard_is_ours(0x10, 0x5253));
    assert!(!keyboard_is_ours(0x10, 0));
    assert!(!keyboard_is_ours(0x00, 0x5253));
    assert!(keyboard_is_ours(0x12, 0x5253));
    assert!(mouse_is_ours(0x01, 0x5253));
    assert!(!mouse_is_ours(0x02, 0x5253));
    assert!(!mouse_is_ours(0x01, 0));
}

#[test]
fn keep_awake_latch_only_reports_changes() {
    let mut latch = KeepAwakeLatch::default();
    assert_eq!(latch.set(false), None);
    assert_eq!(latch.set(true), Some(true));
    assert_eq!(latch.set(true), None);
    assert_eq!(latch.set(false), Some(false));
}

#[test]
fn activity_boundaries_use_the_most_recent_input() {
    let mut state = ActivityState::default();
    assert_eq!(state.on_tick(Duration::from_secs(10)), None);
    assert_eq!(state.on_event(Duration::ZERO), Some(true));
    assert_eq!(state.on_event(Duration::from_millis(199)), None);
    assert_eq!(state.on_event(Duration::from_millis(200)), Some(true));
    assert_eq!(state.on_event(Duration::from_millis(201)), None);
    assert_eq!(state.on_tick(Duration::from_millis(2_200)), None);
    assert_eq!(state.on_tick(Duration::from_millis(2_201)), Some(false));
}
