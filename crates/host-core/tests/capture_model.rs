use std::{sync::Arc, time::Duration};

use host_core::platform::CursorUpdate;
use host_core::{capture_model::*, cursor_shape::PointerKind};
use rsp_core::pb;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}
fn ready() -> Recovery {
    let mut r = Recovery::new();
    assert_eq!(r.step(ms(0)), Step::CreateDevice);
    r.device_result(ms(0), Ok(()));
    assert_eq!(r.step(ms(0)), Step::CreateDuplication);
    r.duplication_result(ms(0), Ok(()));
    assert_eq!(r.step(ms(0)), Step::Acquire);
    r
}

#[test]
fn every_hresult() {
    for (hr, class) in [
        (0, HrClass::Ok),
        (1, HrClass::Ok),
        (DXGI_ERROR_WAIT_TIMEOUT, HrClass::Timeout),
        (DXGI_ERROR_ACCESS_LOST, HrClass::AccessLost),
        (DXGI_ERROR_ACCESS_DENIED, HrClass::AccessDenied),
        (E_ACCESSDENIED, HrClass::AccessDenied),
        (DXGI_ERROR_DEVICE_REMOVED, HrClass::DeviceLost),
        (DXGI_ERROR_DEVICE_HUNG, HrClass::DeviceLost),
        (DXGI_ERROR_DEVICE_RESET, HrClass::DeviceLost),
        (DXGI_ERROR_DRIVER_INTERNAL_ERROR, HrClass::DeviceLost),
        (DXGI_ERROR_INVALID_CALL, HrClass::InvalidCall),
        (E_INVALIDARG, HrClass::InvalidArg),
        (DXGI_ERROR_NOT_CURRENTLY_AVAILABLE, HrClass::NotAvailable),
        (
            DXGI_ERROR_SESSION_DISCONNECTED,
            HrClass::SessionDisconnected,
        ),
        (DXGI_ERROR_UNSUPPORTED, HrClass::Unsupported),
        (DXGI_ERROR_NOT_FOUND, HrClass::NotFound),
        (0x80004005_u32 as i32, HrClass::Other),
    ] {
        assert_eq!(classify_hr(hr), class, "{hr:x}");
    }
}
#[test]
fn bounded_backoff() {
    for (n, delay) in [
        (0, 0),
        (1, 50),
        (2, 100),
        (5, 800),
        (6, 1000),
        (40, 1000),
        (u32::MAX, 1000),
    ] {
        assert_eq!(backoff(n), ms(delay));
    }
}
#[test]
fn recovery_normal_and_access_lost() {
    let mut r = ready();
    r.acquire_result(ms(5), HrClass::Timeout);
    assert_eq!(r.step(ms(5)), Step::Acquire);
    r.acquire_result(ms(1000), HrClass::AccessLost);
    assert_eq!(r.step(ms(1000)), Step::CreateDuplication);
    let mut t = ms(1000);
    for delay in [50, 100, 200, 400, 800, 1000, 1000] {
        r.duplication_result(t, Err(HrClass::AccessDenied));
        assert_eq!(r.step(t), Step::Wait(ms(delay)));
        t += ms(delay);
        assert_eq!(r.step(t), Step::CreateDuplication);
    }
    r.duplication_result(t, Ok(()));
    assert_eq!(r.step(t), Step::Acquire);
    assert_eq!(r.failures(), 0);
    r.acquire_result(t, HrClass::AccessLost);
    r.duplication_result(t, Err(HrClass::AccessDenied));
    assert_eq!(r.step(t), Step::Wait(ms(50)));
}
#[test]
fn recovery_acquire_classes() {
    for class in [HrClass::DeviceLost, HrClass::NotFound] {
        let mut r = ready();
        r.acquire_result(ms(0), class);
        assert_eq!(r.step(ms(0)), Step::CreateDevice);
    }
    for class in [
        HrClass::AccessLost,
        HrClass::AccessDenied,
        HrClass::InvalidCall,
    ] {
        let mut r = ready();
        r.acquire_result(ms(0), class);
        assert_eq!(r.step(ms(0)), Step::CreateDuplication);
    }
    let mut r = ready();
    r.acquire_result(ms(0), HrClass::NotAvailable);
    assert_eq!(r.step(ms(0)), Step::Wait(ms(50)));
    assert_eq!(r.step(ms(50)), Step::CreateDuplication);
}
#[test]
fn output_gone_grace_and_reset() {
    let mut r = Recovery::new();
    for t in [0, 1000, 2999] {
        r.device_result(ms(t), Err(HrClass::NotFound));
        assert_ne!(r.step(ms(t)), Step::GiveUp);
    }
    r.device_result(ms(3000), Err(HrClass::NotFound));
    assert_eq!(r.step(ms(3000)), Step::GiveUp);
    let mut r = Recovery::new();
    r.device_result(ms(0), Err(HrClass::NotFound));
    r.device_result(ms(1000), Ok(()));
    for t in [2000, 4000] {
        r.device_result(ms(t), Err(HrClass::NotFound));
        assert_ne!(r.step(ms(t)), Step::GiveUp);
    }
    r.device_result(ms(5000), Err(HrClass::NotFound));
    assert_eq!(r.step(ms(5000)), Step::GiveUp);
}
#[test]
fn hard_failure_limits_and_success_reset() {
    for class in [HrClass::Unsupported, HrClass::InvalidArg, HrClass::Other] {
        let mut r = ready();
        for _ in 0..9 {
            r.duplication_result(ms(0), Err(class));
            assert_ne!(r.step(ms(0)), Step::GiveUp);
        }
        r.duplication_result(ms(0), Err(class));
        assert_eq!(r.step(ms(0)), Step::GiveUp);
        let mut r = ready();
        for _ in 0..9 {
            r.duplication_result(ms(0), Err(class));
        }
        r.duplication_result(ms(0), Ok(()));
        for _ in 0..9 {
            r.duplication_result(ms(0), Err(class));
            assert_ne!(r.step(ms(0)), Step::GiveUp);
        }
    }
    let mut r = Recovery::new();
    for _ in 0..4 {
        r.device_result(ms(0), Err(HrClass::Other));
        assert_ne!(r.step(ms(0)), Step::GiveUp);
    }
    r.device_result(ms(0), Err(HrClass::Other));
    assert_eq!(r.step(ms(0)), Step::GiveUp);
}
#[test]
fn transient_duplication_errors_never_give_up() {
    for class in [
        HrClass::AccessDenied,
        HrClass::AccessLost,
        HrClass::NotAvailable,
        HrClass::SessionDisconnected,
        HrClass::InvalidCall,
        HrClass::Timeout,
    ] {
        let mut r = ready();
        for t in 0..1000 {
            r.duplication_result(ms(t * 1000), Err(class));
            assert_ne!(r.step(ms(t * 1000)), Step::GiveUp);
        }
    }
    for class in [HrClass::DeviceLost, HrClass::NotFound] {
        let mut r = ready();
        r.duplication_result(ms(0), Err(class));
        assert_eq!(r.step(ms(0)), Step::Wait(ms(50)));
        assert_eq!(r.step(ms(50)), Step::CreateDevice);
    }
}
#[test]
fn desktop_recovery() {
    let mut r = ready();
    r.desktop_switched();
    assert_eq!(r.step(ms(0)), Step::CreateDuplication);
    r.acquire_result(ms(0), HrClass::DeviceLost);
    r.desktop_switched();
    assert_eq!(r.step(ms(0)), Step::CreateDevice);
    let mut r = ready();
    r.desktop_attach_failed(ms(10));
    assert_eq!(r.step(ms(10)), Step::Wait(ms(50)));
    assert_eq!(r.step(ms(60)), Step::CreateDevice);
    r.desktop_switched();
    assert_eq!(r.step(ms(10)), Step::Wait(ms(50)));
}
#[test]
fn pacer_preserves_cadence_and_retries_full() {
    let mut p = Pacer::new(50);
    assert_eq!(p.interval(), ms(20));
    assert_eq!(p.until_due(ms(0)), ms(0));
    p.on_sent(ms(0));
    assert_eq!(p.until_due(ms(5)), ms(15));
    assert_eq!(p.until_due(ms(20)), ms(0));
    for (t, wait) in [(23, 17), (41, 19), (500, 20)] {
        p.on_sent(ms(t));
        assert_eq!(p.until_due(ms(t)), ms(wait));
    }
    p.on_full(ms(520));
    assert_eq!(p.until_due(ms(520)), ms(5));
    p.on_sent(ms(526));
    assert_eq!(p.until_due(ms(526)), ms(19));
    p.reset();
    assert_eq!(p.until_due(ms(526)), ms(0));
    assert_eq!(Pacer::new(0).interval(), ms(1000));
    assert_eq!(Pacer::new(1000).interval(), Duration::from_nanos(8_333_333));
}
#[test]
fn fanout_latest_backpressure_and_lifecycle() {
    let mut f = Fanout::<u32, &str>::new();
    let mut calls = Vec::new();
    macro_rules! poll {
        ($t:expr) => {{
            calls.clear();
            f.poll(ms($t), |s, v| {
                calls.push((*s, v));
                SendResult::Sent
            })
        }};
    }
    f.add(1, 50, "a");
    poll!(0);
    assert!(calls.is_empty());
    assert_eq!(f.wait_hint(ms(0), ms(50)), ms(50));
    f.publish(10);
    assert_eq!(poll!(0).sent, 1);
    assert_eq!(calls, [("a", 10)]);
    f.publish(11);
    poll!(5);
    assert!(calls.is_empty());
    assert_eq!(f.wait_hint(ms(5), ms(50)), ms(15));
    f.add(2, 10, "b");
    poll!(6);
    assert_eq!(calls, [("b", 11)]);
    poll!(20);
    assert_eq!(calls, [("a", 11)]);
    poll!(25);
    assert!(calls.is_empty());
    assert_eq!(f.wait_hint(ms(25), ms(50)), ms(50));
    f.publish(12);
    calls.clear();
    assert_eq!(
        f.poll(ms(40), |s, v| {
            calls.push((*s, v));
            SendResult::Full(v)
        }),
        PollReport {
            full: 1,
            ..Default::default()
        }
    );
    assert_eq!(calls, [("a", 12)]);
    assert_eq!(f.wait_hint(ms(40), ms(50)), ms(5));
    poll!(45);
    assert_eq!(calls, [("a", 12)]);
    f.publish(13);
    assert_eq!(f.poll(ms(65), |_, _| SendResult::Closed).closed, 1);
    assert_eq!(f.len(), 1);
    poll!(106);
    assert_eq!(calls, [("b", 13)]);
    f.clear();
    f.publish(14);
    poll!(107);
    assert_eq!(calls, [("b", 14)]);
    assert_eq!(f.remove(2), Some("b"));
    assert!(f.is_empty());
    assert_eq!(f.remove(99), None);
    f.add(3, 30, "c");
    f.add(4, 30, "d");
    assert_eq!(f.drain(), ["c", "d"]);
    assert!(f.is_empty());
}
#[test]
fn full_sink_gets_newest_frame_on_retry() {
    let mut f = Fanout::new();
    f.add(1, 30, ());
    f.publish(1);
    f.poll(ms(0), |_, v| SendResult::Full(v));
    f.publish(2);
    assert_eq!(f.poll(ms(4), |_, _| panic!("too early")).sent, 0);
    assert_eq!(
        f.poll(ms(5), |_, v| {
            assert_eq!(v, 2);
            SendResult::Sent
        })
        .sent,
        1
    );
}
#[test]
fn pool_only_reuses_unshared_slots() {
    let mut pool = FramePool::new(3);
    let mut allocs = 0;
    let mut slots: Vec<_> = (10..13)
        .map(|v| {
            pool.checkout(|| {
                allocs += 1;
                Ok::<_, &str>(v)
            })
            .unwrap()
            .unwrap()
        })
        .collect();
    assert_eq!(allocs, 3);
    assert!(pool.checkout::<()>(|| panic!("full")).unwrap().is_none());
    let ptr = Arc::as_ptr(&slots[1]);
    drop(slots.remove(1));
    let reused = pool
        .checkout::<()>(|| panic!("must reuse"))
        .unwrap()
        .unwrap();
    assert_eq!(Arc::as_ptr(&reused), ptr);
    assert_eq!(*reused, 11);
    drop(reused);
    drop(slots);
    let first = pool
        .checkout::<()>(|| panic!("must reuse slot 0"))
        .unwrap()
        .unwrap();
    assert_eq!(*first, 10);
    pool.clear();
    assert!(pool.is_empty());
    assert_eq!(*first, 10);
    assert_eq!(pool.checkout(|| Err::<u32, _>("oom")), Err("oom"));
    assert_eq!(pool.len(), 0);
    assert!(FramePool::<u32>::new(0)
        .checkout::<()>(|| panic!("zero capacity"))
        .unwrap()
        .is_none());
}
fn shape(id: u64) -> pb::CursorShape {
    pb::CursorShape {
        id,
        width: 32,
        height: 32,
        hot_x: if id == 1 { 2 } else { 0 },
        hot_y: if id == 1 { 3 } else { 0 },
        scale: 1.0,
        png: vec![1],
    }
}
fn report(d: u32, t: i64, vis: bool, x: i32, y: i32, w: u32, h: u32) -> PointerReport {
    PointerReport {
        display_id: d,
        update_time: t,
        visible: vis,
        x,
        y,
        width: w,
        height: h,
    }
}
fn pos(display_id: u32, seq: u32, x: f32, y: f32, visible: bool) -> CursorUpdate {
    CursorUpdate::Position {
        display_id,
        seq,
        x,
        y,
        visible,
    }
}
#[test]
fn cursor_shapes_ownership_position_and_snapshot() {
    let mut c = CursorModel::new();
    let a = shape(1);
    let b = shape(2);
    assert_eq!(c.on_shape(a.clone()), Some(CursorUpdate::Shape(a.clone())));
    assert_eq!(c.on_shape(a.clone()), None);
    assert_eq!(c.on_shape(b.clone()), Some(CursorUpdate::Shape(b.clone())));
    assert_eq!(c.on_shape(a.clone()), Some(CursorUpdate::Use(1)));
    assert_eq!(
        c.on_report(&report(1, 10, true, 98, 47, 200, 100)),
        [pos(1, 0, 0.5, 0.5, true), CursorUpdate::Hidden(false)]
    );
    assert!(c
        .on_report(&report(1, 11, true, 98, 47, 200, 100))
        .is_empty());
    assert!(c.on_report(&report(1, 0, true, 0, 0, 200, 100)).is_empty());
    assert!(c.on_report(&report(2, 12, false, 0, 0, 40, 20)).is_empty());
    assert_eq!(
        c.on_report(&report(2, 13, true, 18, 7, 40, 20)),
        [pos(2, 1, 0.5, 0.5, true)]
    );
    assert!(c.on_report(&report(1, 12, true, 0, 0, 200, 100)).is_empty());
    assert_eq!(
        c.on_report(&report(2, 14, false, 18, 7, 40, 20)),
        [pos(2, 2, 0.5, 0.5, false), CursorUpdate::Hidden(true)]
    );
    assert_eq!(
        c.on_report(&report(2, 15, true, 1000, -50, 40, 20)),
        [pos(2, 3, 1.0, 0.0, true), CursorUpdate::Hidden(false)]
    );
    assert_eq!(
        c.on_report(&report(2, 16, true, 5, 5, 0, 0)),
        [pos(2, 4, 0.5, 0.5, true)]
    );
    c.display_stopped(2);
    assert!(c
        .on_report(&report(1, 17, false, 0, 0, 200, 100))
        .is_empty());
    assert_eq!(
        c.on_report(&report(1, 18, true, 98, 47, 200, 100)),
        [pos(1, 5, 0.5, 0.5, true)]
    );
    c.forget_shape(1);
    assert_eq!(c.on_shape(a.clone()), Some(CursorUpdate::Shape(a.clone())));
    c.reset_shapes();
    assert_eq!(c.on_shape(b.clone()), Some(CursorUpdate::Shape(b)));
    assert_eq!(c.on_shape(a.clone()), Some(CursorUpdate::Use(1)));
    assert_eq!(
        c.snapshot(),
        [CursorUpdate::Shape(a.clone()), pos(1, 6, 0.5, 0.5, true)]
    );
    c.on_report(&report(1, 19, false, 98, 47, 200, 100));
    assert_eq!(
        c.snapshot(),
        [
            CursorUpdate::Shape(a),
            pos(1, 8, 0.5, 0.5, false),
            CursorUpdate::Hidden(true)
        ]
    );
}
#[test]
fn cursor_cache_bound_and_initial_hidden() {
    let mut c = CursorModel::new();
    assert!(c.on_report(&report(1, 5, false, 0, 0, 10, 10)).is_empty());
    assert!(c.snapshot().is_empty());
    for id in 1..=257 {
        assert_eq!(c.on_shape(shape(id)), Some(CursorUpdate::Shape(shape(id))));
    }
    assert_eq!(c.on_shape(shape(1)), Some(CursorUpdate::Shape(shape(1))));
}
#[test]
fn pointer_kinds_and_rotation() {
    for (n, k) in [
        (1, Some(PointerKind::Monochrome)),
        (2, Some(PointerKind::Color)),
        (4, Some(PointerKind::MaskedColor)),
        (0, None),
        (3, None),
    ] {
        assert_eq!(pointer_kind(n), k);
    }
    for (n, d) in [(0, 0), (1, 0), (2, 90), (3, 180), (4, 270), (9, 0)] {
        assert_eq!(dxgi_rotation_degrees(n), d);
    }
}
#[test]
fn output_names_and_identity() {
    let name = r"\\.\DISPLAY1";
    assert_eq!(
        device_name_from_wide(&name.encode_utf16().chain([0, 0, 0]).collect::<Vec<_>>()),
        name
    );
    assert_eq!(
        device_name_from_wide(&name.encode_utf16().collect::<Vec<_>>()),
        name
    );
    assert_eq!(device_name_from_wide(&[]), "");
    assert_eq!(device_name_from_wide(&[0xd800]), "\u{fffd}");
    let outputs = [
        (r"\\.\DISPLAY2".into(), true),
        (name.into(), false),
        (name.into(), true),
    ];
    assert_eq!(find_output(&outputs, 1), Some(2));
    assert_eq!(find_output(&outputs, 2), Some(0));
    assert_eq!(find_output(&outputs, 3), None);
}
#[test]
fn present_time_conversion() {
    assert_eq!(
        present_to_unix_us(49_990_000, 50_000_000, 10_000_000, 2_000_000_000),
        1_999_999_000
    );
    for (last, now, freq) in [(0, 100, 10), (101, 100, 10), (1, 100, 0), (-1, 100, 10)] {
        assert_eq!(present_to_unix_us(last, now, freq, 123), 123);
    }
    assert_eq!(
        present_to_unix_us(49_990_000, 50_000_000, 10_000_000, 500),
        0
    );
    assert_eq!(present_to_unix_us(1, i64::MAX, 1, 1), 0);
}
#[test]
fn bgra_rows_padding_opaque_alpha_and_bounds() {
    let src = [
        1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 9, 10, 11, 12, 13, 14, 15, 16,
    ];
    assert_eq!(
        bgra_to_rgba(&src, 2, 2, 12),
        Some(vec![
            3, 2, 1, 255, 7, 6, 5, 255, 11, 10, 9, 255, 15, 14, 13, 255
        ])
    );
    assert_eq!(bgra_to_rgba(&src, 2, 2, 7), None);
    assert_eq!(bgra_to_rgba(&src[..19], 2, 2, 12), None);
    assert_eq!(bgra_to_rgba(&[], 0, 2, 0), Some(vec![]));
    assert_eq!(bgra_to_rgba(&[], 2, 0, 0), Some(vec![]));
    assert_eq!(bgra_to_rgba(&[], 2, 3, usize::MAX), None);
}
#[test]
fn periodic_explicit_clock() {
    let mut p = Periodic::new(ms(100));
    for (t, due) in [
        (0, true),
        (50, false),
        (100, true),
        (150, false),
        (260, true),
    ] {
        assert_eq!(p.due(ms(t)), due);
    }
}
