use host_core::audit::{AuditPath, HostAuditRecord};
use host_core::session::{handlers::roster, journal::JOURNAL_FILE, takeover::Seat};
use rsp_core::pb;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

mod wire_support;
use wire_support::{device_id, Dev, OffsetClock, WireHost, WireOptions};

struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rs-b11f-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn start(dir: &Path, clock: &Arc<OffsetClock>) -> WireHost {
    WireHost::start_with(WireOptions {
        audit: true,
        state_dir: Some(dir.to_path_buf()),
        clock: Some(clock.clone()),
        ..Default::default()
    })
    .await
}

async fn stop(host: WireHost) {
    let weak = Arc::downgrade(&host.mgr);
    let started = Instant::now();
    drop(host);
    tokio::time::timeout(Duration::from_secs(5), async {
        while weak.strong_count() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Host tasks still hold SessionManager after 5 seconds");
    eprintln!(
        "stop(): SessionManager released after {:?}",
        started.elapsed()
    );
}

fn journal(dir: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(dir.join(JOURNAL_FILE)).unwrap()).unwrap()
}

fn session_events(host: &WireHost) -> Vec<HostAuditRecord> {
    host.audit_events()
        .into_iter()
        .filter(|record| {
            matches!(
                record.event.as_str(),
                "session_start" | "session_end" | "role_change"
            )
        })
        .collect()
}

fn assert_ok(auth: &pb::AuthResult, role: pb::Role) {
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert_eq!(auth.role, role as i32);
}

fn assert_pair(events: &[HostAuditRecord]) {
    assert_eq!(events.len(), 2, "unexpected session audit: {events:?}");
    assert_eq!(events[0].event, "session_start");
    assert_eq!(events[1].event, "session_end");
}

#[tokio::test]
async fn lingering_session_is_ended_after_restart() {
    let dir = Dir::new();
    let clock = Arc::new(OffsetClock::manual());
    let h1 = start(&dir.0, &clock).await;
    let mut a = h1.connect().await;
    assert_ok(&a.hello().await.1, pb::Role::Controller);
    let since_us = h1.conns()[0].since_us;
    clock.skip(Duration::from_secs(5));
    a.goodbye(pb::goodbye::Reason::Background).await;
    h1.wait_until(|| {
        matches!(roster::seat(&h1.mgr), Seat::Lingering { .. }) && h1.conns().is_empty()
    })
    .await;
    clock.skip(Duration::from_secs(30));
    stop(h1).await;

    let h2 = start(&dir.0, &clock).await;
    // Recovery is synchronous with start: no polling before these assertions.
    let events = session_events(&h2);
    assert_pair(&events);
    let end = &events[1];
    assert_eq!(end.duration_s, Some(5));
    assert_eq!(
        end.detail,
        Some(json!({
            "role": "controller",
            "recovered": true,
            "ended_at": since_us + 5_000_000,
        }))
    );
    assert_eq!(end.path, Some(AuditPath::Lan));
    assert_eq!(end.device_id, Some(device_id(Dev::A)));
    assert_eq!(end.user, None);
    assert_eq!(end.ts_us, Some(events[0].ts_us.unwrap() + 35_000_000));
    assert!(since_us + 5_000_000 < end.ts_us.unwrap());
    assert_eq!(journal(&dir.0)["sessions"], json!([]));
    assert!(roster::summaries(&h2.mgr).is_empty());

    stop(h2).await;
    let h3 = start(&dir.0, &clock).await;
    let events = session_events(&h3);
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].detail, end.detail);
    stop(h3).await;
}

#[tokio::test]
async fn active_session_is_ended_with_last_alive_time() {
    let dir = Dir::new();
    let clock = Arc::new(OffsetClock::manual());
    let h1 = start(&dir.0, &clock).await;
    let mut a = h1.connect().await;
    assert_ok(&a.hello().await.1, pb::Role::Controller);
    let since_us = h1.conns()[0].since_us;
    clock.skip(Duration::from_secs(61));
    h1.wait_until(|| {
        journal(&dir.0)["sessions"][0]["alive_at_us"].as_u64() == Some(since_us + 61_000_000)
    })
    .await;
    stop(h1).await;
    clock.skip(Duration::from_secs(1));

    let h2 = start(&dir.0, &clock).await;
    let events = session_events(&h2);
    assert_pair(&events);
    assert_eq!(events[1].duration_s, Some(61));
    assert_eq!(
        events[1].detail,
        Some(json!({
            "role": "controller",
            "recovered": true,
            "ended_at": since_us + 61_000_000,
        }))
    );
    assert!(events[1].ts_us.unwrap() > since_us + 61_000_000);
    // Keep the client alive through recovery so dropping it cannot end H1 normally.
    drop(a);
    stop(h2).await;
}

#[tokio::test]
async fn normal_end_is_not_repeated() {
    let dir = Dir::new();
    let clock = Arc::new(OffsetClock::manual());
    let h1 = start(&dir.0, &clock).await;
    let mut a = h1.connect().await;
    assert_ok(&a.hello().await.1, pb::Role::Controller);
    clock.skip(Duration::from_secs(5));
    a.goodbye(pb::goodbye::Reason::UserClosed).await;
    h1.wait_until(|| session_events(&h1).len() == 2).await;
    stop(h1).await;

    let h2 = start(&dir.0, &clock).await;
    let events = session_events(&h2);
    assert_pair(&events);
    assert_eq!(events[1].duration_s, Some(5));
    assert_eq!(events[1].detail, Some(json!({"role": "controller"})));
    assert_eq!(journal(&dir.0)["sessions"], json!([]));
    stop(h2).await;
}

#[tokio::test]
async fn two_open_sessions_get_distinct_receipts() {
    let dir = Dir::new();
    let clock = Arc::new(OffsetClock::manual());
    let h1 = start(&dir.0, &clock).await;
    let mut a = h1.connect().await;
    assert_ok(&a.hello().await.1, pb::Role::Controller);
    let mut b = h1.connect_as(Dev::B, 0).await;
    assert_ok(&b.hello().await.1, pb::Role::Viewer);
    let conns = h1.conns();
    assert_eq!(conns.len(), 2);
    let since_us = conns[0].since_us;
    assert_eq!(conns[1].since_us, since_us);

    clock.skip(Duration::from_secs(3));
    a.goodbye(pb::goodbye::Reason::Background).await;
    h1.wait_until(|| {
        matches!(roster::seat(&h1.mgr), Seat::Lingering { .. }) && h1.conns().len() == 1
    })
    .await;
    // At 61s the live session needs a touch, while A's linger lasts until 63s.
    clock.skip(Duration::from_secs(58));
    h1.wait_until(|| {
        journal(&dir.0)["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| {
                entry["device_id"].as_str() == Some(device_id(Dev::B).as_str())
                    && entry["alive_at_us"].as_u64() == Some(since_us + 61_000_000)
            })
    })
    .await;
    stop(h1).await;
    clock.skip(Duration::from_secs(1));

    let h2 = start(&dir.0, &clock).await;
    let events = session_events(&h2);
    assert_eq!(events.len(), 4, "unexpected session audit: {events:?}");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event == "session_start")
            .count(),
        2
    );
    let ends: Vec<_> = events
        .iter()
        .filter(|event| event.event == "session_end")
        .collect();
    assert_eq!(ends.len(), 2);
    for (dev, duration_s, role) in [(Dev::A, 3, "controller"), (Dev::B, 61, "viewer")] {
        let end = ends
            .iter()
            .find(|event| event.device_id == Some(device_id(dev)))
            .unwrap();
        assert_eq!(end.duration_s, Some(duration_s));
        let ended_at = match dev {
            Dev::A => since_us + 3_000_000,
            Dev::B => since_us + 61_000_000,
            _ => unreachable!("only A and B have recovered session ends"),
        };
        assert_eq!(
            end.detail,
            Some(json!({
                "role": role,
                "recovered": true,
                "ended_at": ended_at,
            }))
        );
        assert_eq!(end.ts_us, Some(since_us + 62_000_000));
        assert!(ended_at < end.ts_us.unwrap());
        assert!(end.seq.is_some());
    }
    assert_ne!((ends[0].ts_us, ends[0].seq), (ends[1].ts_us, ends[1].seq));
    assert_eq!(journal(&dir.0)["sessions"], json!([]));
    drop(b);
    stop(h2).await;
}
