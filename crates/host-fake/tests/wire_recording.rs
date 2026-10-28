use host_core::audit::HostAuditRecord;
use host_core::session::handlers::roster;
use rsp_core::{features, pb};
use serde_json::json;
use std::{sync::Arc, time::Duration};

mod wire_support;
use wire_support::{client_hello, device_id, Dev, OffsetClock, WireClient, WireHost, WireOptions};

fn start() -> pb::StartStream {
    pb::StartStream {
        codec: pb::Codec::Hevc as i32,
        max_width: 320,
        fps: 10,
        bitrate_kbps: 1_000,
        ..Default::default()
    }
}

fn recording_events(host: &WireHost) -> Vec<HostAuditRecord> {
    host.audit_events()
        .into_iter()
        .filter(|event| event.event == "recording")
        .collect()
}

fn recording_state(host: &WireHost, device: Dev) -> Option<bool> {
    roster::summaries(&host.mgr)
        .into_iter()
        .find(|summary| summary.device_id == device_id(device))
        .map(|summary| summary.recording)
}

async fn barrier(client: &mut WireClient) {
    client.send(pb::Ping { t1_us: 7 }).await;
    client
        .next_matching(|message| match message {
            pb::envelope::Msg::Pong(pong) if pong.t1_us == 7 => Some(()),
            _ => None,
        })
        .await;
}

async fn audited_host(clock: Arc<OffsetClock>) -> WireHost {
    WireHost::start_with(WireOptions {
        audit: true,
        clock: Some(clock),
        ..Default::default()
    })
    .await
}

#[tokio::test]
async fn controller_recording_notice_tracks_state_and_duration() {
    let clock = Arc::new(OffsetClock::manual());
    let host = audited_host(clock.clone()).await;
    let mut client = host.connect().await;
    let (server, auth) = client.hello().await;
    assert_ne!(server.features & features::RECORDING_NOTICE, 0);
    assert_eq!(auth.role, pb::Role::Controller as i32);
    client.start_stream(start()).await;

    client.send(pb::RecordingNotice { active: false }).await;
    barrier(&mut client).await;
    assert!(recording_events(&host).is_empty());

    client.send(pb::RecordingNotice { active: true }).await;
    host.wait_until(|| recording_state(&host, Dev::A) == Some(true))
        .await;
    assert!(recording_events(&host).is_empty());

    client.send(pb::RecordingNotice { active: true }).await;
    barrier(&mut client).await;
    assert_eq!(recording_state(&host, Dev::A), Some(true));
    assert!(recording_events(&host).is_empty());

    clock.skip(Duration::from_secs(90));
    client.send(pb::RecordingNotice { active: false }).await;
    barrier(&mut client).await;
    host.wait_until(|| recording_state(&host, Dev::A) == Some(false))
        .await;

    let events = recording_events(&host);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].duration_s, Some(90));
    assert_eq!(events[0].detail, Some(json!({"role": "controller"})));
    assert_eq!(
        events[0].device_id.as_deref(),
        Some(device_id(Dev::A).as_str())
    );
}

#[tokio::test]
async fn disconnect_finishes_recording_and_clears_state() {
    let clock = Arc::new(OffsetClock::manual());
    let host = audited_host(clock.clone()).await;
    let mut client = host.connect().await;
    client.hello().await;
    client.start_stream(start()).await;
    client.send(pb::RecordingNotice { active: true }).await;
    host.wait_until(|| recording_state(&host, Dev::A) == Some(true))
        .await;
    clock.skip(Duration::from_secs(4));

    client.conn.close(0u32.into(), b"recording test disconnect");
    host.wait_until(|| host.conns().is_empty() && recording_events(&host).len() == 1)
        .await;
    let events = recording_events(&host);
    assert_eq!(events[0].duration_s, Some(4));
    assert_eq!(events[0].detail, Some(json!({"role": "controller"})));
}

#[tokio::test]
async fn resumed_connection_starts_without_recording_marker() {
    let clock = Arc::new(OffsetClock::manual());
    let host = audited_host(clock.clone()).await;
    let mut client = host.connect().await;
    let (_, auth) = client.hello().await;
    client.send(pb::RecordingNotice { active: true }).await;
    host.wait_until(|| recording_state(&host, Dev::A) == Some(true))
        .await;
    clock.skip(Duration::from_secs(3));
    client.goodbye(pb::goodbye::Reason::Background).await;
    host.wait_until(|| host.conns().is_empty() && recording_events(&host).len() == 1)
        .await;
    assert_eq!(recording_events(&host)[0].duration_s, Some(3));

    let mut resumed = host.connect().await;
    let (_, auth) = resumed
        .hello_with(pb::ClientHello {
            resume_token: auth.session_token,
            ..client_hello()
        })
        .await;
    assert!(auth.resumed);
    assert_eq!(recording_state(&host, Dev::A), Some(false));
    resumed.send(pb::RecordingNotice { active: true }).await;
    host.wait_until(|| recording_state(&host, Dev::A) == Some(true))
        .await;
    clock.skip(Duration::from_secs(2));
    resumed.send(pb::RecordingNotice { active: false }).await;
    host.wait_until(|| recording_events(&host).len() == 2).await;
    assert_eq!(recording_events(&host)[1].duration_s, Some(2));
    assert_eq!(recording_state(&host, Dev::A), Some(false));
}

#[tokio::test]
async fn viewer_recording_notice_audits_viewer_role() {
    let clock = Arc::new(OffsetClock::manual());
    let host = audited_host(clock.clone()).await;
    let mut controller = host.connect().await;
    controller.hello().await;
    let mut viewer = host.connect_as(Dev::B, 0).await;
    let (_, auth) = viewer.hello().await;
    assert_eq!(auth.role, pb::Role::Viewer as i32);

    viewer.send(pb::RecordingNotice { active: true }).await;
    host.wait_until(|| recording_state(&host, Dev::B) == Some(true))
        .await;
    clock.skip(Duration::from_secs(2));
    viewer.send(pb::RecordingNotice { active: false }).await;
    barrier(&mut viewer).await;
    host.wait_until(|| recording_state(&host, Dev::B) == Some(false))
        .await;

    let events = recording_events(&host);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].duration_s, Some(2));
    assert_eq!(events[0].detail, Some(json!({"role": "viewer"})));
    assert_eq!(
        events[0].device_id.as_deref(),
        Some(device_id(Dev::B).as_str())
    );
}

#[tokio::test]
async fn recording_notice_updates_state_without_audit_when_disabled() {
    let clock = Arc::new(OffsetClock::manual());
    let host = WireHost::start_with(WireOptions {
        clock: Some(clock),
        ..Default::default()
    })
    .await;
    let mut client = host.connect().await;
    client.hello().await;
    client.send(pb::RecordingNotice { active: true }).await;
    host.wait_until(|| recording_state(&host, Dev::A) == Some(true))
        .await;
    client.send(pb::RecordingNotice { active: false }).await;
    barrier(&mut client).await;
    host.wait_until(|| recording_state(&host, Dev::A) == Some(false))
        .await;
    assert!(host.audit_events().is_empty());
}
