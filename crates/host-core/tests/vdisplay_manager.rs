use host_core::platform::DisplayInfo;
use host_core::session::roster::ConnKey;
use host_core::vdisplay::manager::{Begin, Limits, Ticket, VdEvent, VdManager, VdStatus};
use host_core::vdisplay::{VdCreated, VdError, VdMode};

const K1: ConnKey = ConnKey(1);
const K2: ConnKey = ConnKey(2);

fn disp(id: u32, name: &str) -> DisplayInfo {
    DisplayInfo {
        id,
        name: name.to_owned(),
        width_px: 1920,
        height_px: 1080,
        scale: 1.0,
        x: 0,
        y: 0,
        refresh_hz: 60,
        primary: false,
        rotation: 0,
    }
}

fn created(token: u64, platform_id: Option<u32>, match_name: Option<&str>) -> VdCreated {
    VdCreated {
        token,
        platform_id,
        match_name: match_name.map(str::to_owned),
        mode: VdMode {
            width_px: 1920,
            height_px: 1080,
            scale: 1.0,
            refresh_hz: 60,
        },
    }
}

fn begin(mgr: &mut VdManager, owner: &str, conn: ConnKey, request_id: u32) -> Ticket {
    match mgr.begin(owner, "L", conn, request_id, true) {
        Begin::Create(ticket) => ticket,
        other => panic!("expected create, got {other:?}"),
    }
}

fn make_active(
    mgr: &mut VdManager,
    owner: &str,
    conn: ConnKey,
    request_id: u32,
    token: u64,
    id: u32,
) {
    let ticket = begin(mgr, owner, conn, request_id);
    assert_eq!(
        mgr.created(
            ticket,
            Ok(created(token, Some(id), None)),
            &[disp(id, "Virtual")],
            0
        ),
        vec![VdEvent::Ready {
            conn,
            request_id,
            select: true,
            platform_id: id,
        }]
    );
}

#[test]
fn create_then_claim_immediately() {
    let mut mgr = VdManager::new(Limits::default());
    assert_eq!(mgr.begin("s1", "L", K1, 7, true), Begin::Create(Ticket(1)));
    assert_eq!(
        mgr.created(
            Ticket(1),
            Ok(created(11, Some(42), None)),
            &[disp(42, "Virtual")],
            0
        ),
        vec![VdEvent::Ready {
            conn: K1,
            request_id: 7,
            select: true,
            platform_id: 42,
        }]
    );
    assert_eq!(mgr.virtual_names(), vec![(42, "L".to_owned())]);
    assert_eq!(mgr.owner_of(42), Some("s1"));
}

#[test]
fn claim_later_by_display_change() {
    let mut mgr = VdManager::new(Limits::default());
    let ticket = begin(&mut mgr, "s1", K1, 7);
    assert!(mgr
        .created(ticket, Ok(created(11, Some(42), None)), &[], 0)
        .is_empty());
    assert_eq!(
        mgr.on_displays(&[disp(42, "Virtual")], 1),
        vec![VdEvent::Ready {
            conn: K1,
            request_id: 7,
            select: true,
            platform_id: 42,
        }]
    );
}

#[test]
fn claim_by_name_skips_occupied_same_name() {
    let mut mgr = VdManager::new(Limits {
        total: 3,
        ..Limits::default()
    });
    make_active(&mut mgr, "s1", K1, 1, 11, 5);
    let ticket = begin(&mut mgr, "s2", K2, 2);
    assert_eq!(
        mgr.created(
            ticket,
            Ok(created(12, None, Some("RS Virtual 01"))),
            &[disp(5, "RS Virtual 01"), disp(6, "RS Virtual 01")],
            0,
        ),
        vec![VdEvent::Ready {
            conn: K2,
            request_id: 2,
            select: true,
            platform_id: 6,
        }]
    );
}

#[test]
fn existing_is_idempotent() {
    let mut mgr = VdManager::new(Limits::default());
    make_active(&mut mgr, "s1", K1, 7, 11, 42);
    assert_eq!(
        mgr.begin("s1", "other", K2, 8, false),
        Begin::Existing { platform_id: 42 }
    );
    assert_eq!(mgr.len(), 1);
}

#[test]
fn second_while_creating_is_limit() {
    let mut mgr = VdManager::new(Limits::default());
    assert!(matches!(
        mgr.begin("s1", "L", K1, 1, false),
        Begin::Create(_)
    ));
    assert_eq!(
        mgr.begin("s1", "L", K1, 2, false),
        Begin::Reject {
            status: VdStatus::Limit,
            message: "a virtual display is already being created".to_owned(),
        }
    );
}

#[test]
fn total_limit() {
    let mut mgr = VdManager::new(Limits {
        total: 2,
        ..Limits::default()
    });
    assert!(matches!(
        mgr.begin("s1", "L", K1, 1, false),
        Begin::Create(_)
    ));
    assert!(matches!(
        mgr.begin("s2", "L", K2, 2, false),
        Begin::Create(_)
    ));
    assert_eq!(
        mgr.begin("s3", "L", ConnKey(3), 3, false),
        Begin::Reject {
            status: VdStatus::Limit,
            message: "virtual display limit reached".to_owned(),
        }
    );
    let mut one = VdManager::new(Limits {
        total: 1,
        ..Limits::default()
    });
    assert!(matches!(
        one.begin("s1", "L", K1, 1, false),
        Begin::Create(_)
    ));
    assert!(matches!(
        one.begin("s2", "L", K2, 2, false),
        Begin::Reject { .. }
    ));
}

#[test]
fn backend_error_maps_status() {
    for (error, status) in [
        (VdError::Limit, VdStatus::Limit),
        (VdError::Unsupported("x".to_owned()), VdStatus::Unsupported),
        (VdError::Failed("boom".to_owned()), VdStatus::Failed),
    ] {
        let mut mgr = VdManager::new(Limits::default());
        let ticket = begin(&mut mgr, "s1", K1, 7);
        let events = mgr.created(ticket, Err(error), &[], 0);
        assert!(matches!(
            events.as_slice(),
            [VdEvent::Failed {
                status: actual,
                message,
                ..
            }] if *actual == status && (status != VdStatus::Failed || message.contains("boom"))
        ));
        assert!(mgr.is_empty());
    }
}

#[test]
fn no_identity_fails() {
    let mut mgr = VdManager::new(Limits::default());
    let ticket = begin(&mut mgr, "s1", K1, 7);
    assert_eq!(
        mgr.created(ticket, Ok(created(11, None, None)), &[], 0),
        vec![
            VdEvent::Destroy { token: 11 },
            VdEvent::Failed {
                conn: K1,
                request_id: 7,
                status: VdStatus::Failed,
                message: "backend returned no display identity".to_owned(),
            },
        ]
    );
}

#[test]
fn appear_timeout() {
    let mut mgr = VdManager::new(Limits::default());
    let ticket = begin(&mut mgr, "s1", K1, 7);
    assert!(mgr
        .created(ticket, Ok(created(11, Some(42), None)), &[], 0)
        .is_empty());
    assert!(mgr.tick(&|_| true, 9_999_999).is_empty());
    assert_eq!(
        mgr.tick(&|_| true, 10_000_000),
        vec![
            VdEvent::Destroy { token: 11 },
            VdEvent::Failed {
                conn: K1,
                request_id: 7,
                status: VdStatus::Failed,
                message: "virtual display did not appear in time".to_owned(),
            },
        ]
    );
}

#[test]
fn owner_gone_while_creating_destroys_on_arrival() {
    let mut mgr = VdManager::new(Limits::default());
    let ticket = begin(&mut mgr, "s1", K1, 7);
    assert!(mgr.tick(&|_| false, 0).is_empty());
    assert_eq!(
        mgr.created(
            ticket,
            Ok(created(11, Some(42), None)),
            &[disp(42, "Virtual")],
            1
        ),
        vec![VdEvent::Destroy { token: 11 }]
    );
    assert!(mgr.is_empty());
}

#[test]
fn owner_gone_while_creating_and_backend_fails() {
    let mut mgr = VdManager::new(Limits::default());
    let ticket = begin(&mut mgr, "s1", K1, 7);
    assert!(mgr.tick(&|_| false, 0).is_empty());
    assert!(mgr
        .created(ticket, Err(VdError::Failed("boom".to_owned())), &[], 1)
        .is_empty());
    assert!(mgr.is_empty());
}

#[test]
fn owner_gone_active_destroys() {
    let mut mgr = VdManager::new(Limits::default());
    make_active(&mut mgr, "s1", K1, 7, 11, 42);
    assert_eq!(
        mgr.tick(&|owner| owner == "s2", 5),
        vec![VdEvent::Destroy { token: 11 }]
    );
    assert!(mgr.virtual_names().is_empty());
}

#[test]
fn owner_gone_appearing_destroys_without_failed() {
    let mut mgr = VdManager::new(Limits::default());
    let ticket = begin(&mut mgr, "s1", K1, 7);
    assert!(mgr
        .created(ticket, Ok(created(11, Some(42), None)), &[], 0)
        .is_empty());
    assert_eq!(
        mgr.tick(&|_| false, 1),
        vec![VdEvent::Destroy { token: 11 }]
    );
}

#[test]
fn vanish_grace() {
    let mut mgr = VdManager::new(Limits::default());
    make_active(&mut mgr, "s1", K1, 7, 11, 42);
    assert!(mgr.on_displays(&[], 100).is_empty());
    assert!(mgr.tick(&|_| true, 100 + 2_999_999).is_empty());
    assert_eq!(
        mgr.tick(&|_| true, 100 + 3_000_000),
        vec![VdEvent::Destroy { token: 11 }]
    );

    let mut mgr = VdManager::new(Limits::default());
    make_active(&mut mgr, "s1", K1, 7, 11, 42);
    assert!(mgr.on_displays(&[], 100).is_empty());
    assert!(mgr.on_displays(&[disp(42, "Virtual")], 1_100).is_empty());
    assert!(mgr.on_displays(&[], 4_099).is_empty());
    assert!(mgr.tick(&|_| true, 4_100).is_empty());
}

#[test]
fn release_rules() {
    let mut mgr = VdManager::new(Limits::default());
    make_active(&mut mgr, "s1", K1, 7, 11, 42);
    assert_eq!(mgr.release("s2", 42), Err(VdStatus::NotAllowed));
    assert_eq!(mgr.release("s1", 99), Err(VdStatus::BadRequest));
    assert_eq!(
        mgr.release("s1", 42),
        Ok(vec![VdEvent::Destroy { token: 11 }])
    );
    assert_eq!(mgr.release("s1", 42), Err(VdStatus::BadRequest));
}

#[test]
fn drain_all() {
    let mut mgr = VdManager::new(Limits::default());
    make_active(&mut mgr, "s1", K1, 7, 11, 42);
    let ticket = begin(&mut mgr, "s2", K2, 8);
    assert_eq!(mgr.drain_all(), vec![11]);
    assert_eq!(mgr.len(), 1);
    assert_eq!(
        mgr.created(
            ticket,
            Ok(created(12, Some(43), None)),
            &[disp(43, "Virtual")],
            0
        ),
        vec![VdEvent::Destroy { token: 12 }]
    );
}

#[test]
fn unknown_ticket() {
    let mut mgr = VdManager::new(Limits::default());
    assert_eq!(
        mgr.created(Ticket(99), Ok(created(5, Some(42), None)), &[], 0),
        vec![VdEvent::Destroy { token: 5 }]
    );
    assert!(mgr
        .created(Ticket(99), Err(VdError::Failed("x".to_owned())), &[], 0)
        .is_empty());
}

#[test]
fn event_order() {
    let mut mgr = VdManager::new(Limits::default());
    let first = begin(&mut mgr, "s1", K1, 1);
    assert!(mgr
        .created(first, Ok(created(11, Some(41), None)), &[], 0)
        .is_empty());
    let second = begin(&mut mgr, "s2", K2, 2);
    assert!(mgr
        .created(second, Ok(created(12, Some(42), None)), &[], 0)
        .is_empty());
    assert_eq!(
        mgr.tick(&|_| true, 10_000_000),
        vec![
            VdEvent::Destroy { token: 11 },
            VdEvent::Failed {
                conn: K1,
                request_id: 1,
                status: VdStatus::Failed,
                message: "virtual display did not appear in time".to_owned(),
            },
            VdEvent::Destroy { token: 12 },
            VdEvent::Failed {
                conn: K2,
                request_id: 2,
                status: VdStatus::Failed,
                message: "virtual display did not appear in time".to_owned(),
            },
        ]
    );
}

#[test]
fn per_owner_after_release() {
    let mut mgr = VdManager::new(Limits::default());
    make_active(&mut mgr, "s1", K1, 1, 11, 42);
    assert_eq!(
        mgr.release("s1", 42),
        Ok(vec![VdEvent::Destroy { token: 11 }])
    );
    assert_eq!(mgr.begin("s1", "L", K1, 2, false), Begin::Create(Ticket(2)));
}
