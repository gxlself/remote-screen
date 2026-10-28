use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use host_core::platform::Platform;
use host_core::vdisplay::{VdCreated, VdError, VdMode, VirtualDisplayBackend};
use host_fake::vdisplay::{
    mac_like_caps, Appear, FakeVdOptions, FakeVirtualDisplays, VdCall, PLATFORM_ID_BASE,
};
use host_fake::FakePlatform;
use tokio::sync::mpsc;

fn mode(width_px: u32, height_px: u32) -> VdMode {
    VdMode {
        width_px,
        height_px,
        scale: 2.0,
        refresh_hz: 60,
    }
}

fn ids(platform: &FakePlatform) -> Vec<u32> {
    platform.displays().into_iter().map(|d| d.id).collect()
}

fn display_options(appear: Appear) -> FakeVdOptions {
    FakeVdOptions {
        appear,
        ..FakeVdOptions::default()
    }
}

#[test]
fn install_sets_platform_backend() {
    let platform = FakePlatform::new();
    FakeVirtualDisplays::install(&platform, FakeVdOptions::default());

    let backend = platform
        .virtual_displays()
        .expect("installed virtual display backend");
    assert_eq!(backend.caps(), Some(mac_like_caps()));
}

#[test]
fn create_appears_immediately() {
    let platform = FakePlatform::new();
    let backend = FakeVirtualDisplays::install(&platform, FakeVdOptions::default());
    let requested = mode(2732, 2048);

    let created = backend.create(requested, "L").expect("create succeeds");
    assert_eq!(
        created,
        VdCreated {
            token: 1,
            platform_id: Some(901),
            match_name: None,
            mode: requested,
        }
    );
    assert_eq!(ids(&platform), [1, 901]);

    let display = backend.display_of(1).expect("created display is live");
    assert_eq!(display.name, "L");
    assert_eq!(display.width_px, 2732);
    assert_eq!(display.height_px, 2048);
    assert_eq!(display.scale, 2.0);
}

#[test]
fn create_by_name() {
    let platform = FakePlatform::new();
    let backend = FakeVirtualDisplays::install(
        &platform,
        FakeVdOptions {
            identify_by_name: true,
            ..FakeVdOptions::default()
        },
    );

    let created = backend
        .create(mode(1920, 1080), "ignored")
        .expect("create succeeds");
    assert_eq!(created.platform_id, None);
    assert_eq!(created.match_name.as_deref(), Some("RS Virtual 01"));
    assert_eq!(
        backend.display_of(1).map(|display| display.name),
        Some("RS Virtual 01".into())
    );
}

#[test]
fn destroy_removes_and_is_idempotent() {
    let platform = FakePlatform::new();
    let backend = FakeVirtualDisplays::install(&platform, FakeVdOptions::default());
    backend
        .create(mode(2732, 2048), "L")
        .expect("create succeeds");

    backend.destroy(1).expect("first destroy succeeds");
    assert_eq!(ids(&platform), [1]);
    assert_eq!(backend.destroyed(), [1]);
    assert!(backend.live().is_empty());

    backend.destroy(1).expect("repeated destroy is idempotent");
    backend.destroy(42).expect("unknown destroy is idempotent");
    assert_eq!(backend.destroyed(), [1]);
}

#[test]
fn fail_next_once() {
    let platform = FakePlatform::new();
    let backend = FakeVirtualDisplays::install(&platform, FakeVdOptions::default());
    backend.fail_next(VdError::Failed("boom".into()));

    assert_eq!(
        backend.create(mode(1920, 1080), "first"),
        Err(VdError::Failed("boom".into()))
    );
    assert!(backend.create(mode(1920, 1080), "second").is_ok());
    assert_eq!(backend.create_count(), 2);
}

#[test]
fn limit_by_caps() {
    let platform = FakePlatform::new();
    let mut caps = mac_like_caps();
    caps.max_displays = 1;
    let backend = FakeVirtualDisplays::install(
        &platform,
        FakeVdOptions {
            caps: Some(caps),
            ..FakeVdOptions::default()
        },
    );

    backend
        .create(mode(1920, 1080), "first")
        .expect("first create succeeds");
    assert_eq!(
        backend.create(mode(1920, 1080), "second"),
        Err(VdError::Limit)
    );
}

#[test]
fn unsupported_without_caps() {
    let platform = FakePlatform::new();
    let backend = FakeVirtualDisplays::install(
        &platform,
        FakeVdOptions {
            caps: None,
            ..FakeVdOptions::default()
        },
    );

    assert_eq!(backend.caps(), None);
    assert!(matches!(
        backend.create(mode(1920, 1080), "unsupported"),
        Err(VdError::Unsupported(_))
    ));
}

#[test]
fn manual_and_never() {
    let manual_platform = FakePlatform::new();
    let manual = FakeVirtualDisplays::install(&manual_platform, display_options(Appear::Manual));
    manual
        .create(mode(1920, 1080), "manual")
        .expect("manual create succeeds");
    assert!(!ids(&manual_platform).contains(&901));
    manual.show_pending();
    assert!(ids(&manual_platform).contains(&901));

    let never_platform = FakePlatform::new();
    let never = FakeVirtualDisplays::install(&never_platform, display_options(Appear::Never));
    never
        .create(mode(1920, 1080), "never")
        .expect("never create succeeds");
    never.show_pending();
    assert!(!ids(&never_platform).contains(&901));
}

#[test]
fn hold_blocks_until_release() {
    let platform = FakePlatform::new();
    let backend = FakeVirtualDisplays::install(&platform, FakeVdOptions::default());
    backend.hold_creates(true);

    let creating = Arc::clone(&backend);
    let handle = thread::spawn(move || creating.create(mode(2732, 2048), "held"));

    let deadline = Instant::now() + Duration::from_secs(2);
    while backend.waiting_creates() != 1 {
        assert!(Instant::now() < deadline, "create did not start waiting");
        thread::sleep(Duration::from_millis(5));
    }
    assert!(!ids(&platform).contains(&901));

    backend.release_creates();
    handle
        .join()
        .expect("create thread panicked")
        .expect("create succeeds");
    assert!(ids(&platform).contains(&901));
}

#[test]
fn vanish_and_reappear() {
    let platform = FakePlatform::new();
    let backend = FakeVirtualDisplays::install(&platform, FakeVdOptions::default());
    backend
        .create(mode(1920, 1080), "L")
        .expect("create succeeds");

    backend.vanish(1);
    assert!(!ids(&platform).contains(&901));
    assert_eq!(backend.live(), [1]);

    backend.reappear(1);
    assert!(ids(&platform).contains(&901));

    backend.destroy(1).expect("destroy succeeds");
    backend.reappear(1);
    assert_eq!(ids(&platform), [1]);
}

#[test]
fn watchers_notified() {
    let platform = FakePlatform::new();
    let backend = FakeVirtualDisplays::install(&platform, FakeVdOptions::default());
    let (tx, mut rx) = mpsc::channel(4);
    platform.watch_displays(tx);
    let initial = rx.try_recv().expect("initial display list");
    assert_eq!(
        initial.iter().map(|display| display.id).collect::<Vec<_>>(),
        [1]
    );

    backend
        .create(mode(1920, 1080), "watched")
        .expect("create succeeds");
    let updated = rx.try_recv().expect("display update");
    assert!(updated.iter().any(|display| display.id == 901));
}

#[test]
fn calls_recorded() {
    let platform = FakePlatform::new();
    let backend = FakeVirtualDisplays::install(
        &platform,
        FakeVdOptions {
            stale: 3,
            ..FakeVdOptions::default()
        },
    );
    let requested = mode(1920, 1080);

    backend.caps();
    backend.create(requested, "L").expect("create succeeds");
    backend.destroy(1).expect("destroy succeeds");
    assert_eq!(backend.cleanup_stale(), 3);
    assert_eq!(
        backend.calls(),
        [
            VdCall::Caps,
            VdCall::Create {
                mode: requested,
                label: "L".into(),
            },
            VdCall::Destroy { token: 1 },
            VdCall::CleanupStale,
        ]
    );
    assert_eq!(PLATFORM_ID_BASE, 900);
}
