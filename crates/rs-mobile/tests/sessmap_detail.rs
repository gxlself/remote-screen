mod support;

use std::{collections::BTreeMap, time::Duration};

use host_fake::harness::{TestHost, TestHostOptions};
use rs_mobile::rscore::{
    api_sessions,
    dto_core::{OpenIntent, OpenOpts},
    sessmap::SessionMapper,
    store::PairedHostRow,
    tables::SessionTable,
};
use rsp_client::{EndReason, SessionEvent, SessionStateKind};
use rsp_core::{errors::ErrorCode, ids::fp_b64url};
use serde_json::{json, Value};
use tokio::time::{sleep, timeout};

fn error(code: ErrorCode) -> SessionEvent {
    SessionEvent::Error {
        code,
        message: "denied".into(),
    }
}

fn timeout_detail(retry_after_s: Option<u32>) -> SessionEvent {
    SessionEvent::ErrorDetail {
        code: ErrorCode::ApprovalDenied,
        detail: BTreeMap::from([("reason".into(), "timeout".into())]),
        retry_after_s,
    }
}

fn map_json(mapper: &mut SessionMapper, event: SessionEvent) -> Value {
    serde_json::to_value(mapper.map(&event)).expect("JSON events")
}

#[test]
fn matching_error_detail_is_included_in_single_session_error() {
    let mut mapper = SessionMapper::new(7, "host-1");
    assert!(mapper.map(&timeout_detail(None)).is_empty());
    assert_eq!(
        map_json(&mut mapper, error(ErrorCode::ApprovalDenied)),
        json!([{
            "type": "session.error", "sessionId": "7", "hostId": "host-1",
            "code": 140, "message": "denied", "detail": {"reason": "timeout"}
        }])
    );
    let terminal = map_json(
        &mut mapper,
        SessionEvent::Ended {
            code: Some(ErrorCode::ApprovalDenied),
            reason: "denied".into(),
            kind: EndReason::Error(ErrorCode::ApprovalDenied),
        },
    );
    assert!(terminal
        .as_array()
        .unwrap()
        .iter()
        .all(|event| event["type"] != "session.error"));
}

#[test]
fn error_without_detail_omits_optional_fields() {
    let mut mapper = SessionMapper::new(7, "host-1");
    let events = map_json(&mut mapper, error(ErrorCode::ApprovalDenied));
    assert_eq!(events.as_array().unwrap().len(), 1);
    assert_eq!(events[0]["code"], 140);
    assert!(events[0].get("detail").is_none());
    assert!(events[0].get("retryAfterS").is_none());
}

#[test]
fn different_error_code_discards_pending_detail_and_retry() {
    let mut mapper = SessionMapper::new(7, "host-1");
    assert!(mapper.map(&timeout_detail(Some(12))).is_empty());
    let events = map_json(&mut mapper, error(ErrorCode::TakeoverDenied));
    assert_eq!(events.as_array().unwrap().len(), 1);
    assert_eq!(events[0]["code"], 141);
    assert!(events[0].get("detail").is_none());
    assert!(events[0].get("retryAfterS").is_none());
    let subsequent = map_json(&mut mapper, error(ErrorCode::ApprovalDenied));
    assert!(subsequent[0].get("detail").is_none());
    assert!(subsequent[0].get("retryAfterS").is_none());
}

#[test]
fn retry_after_is_preserved_with_or_without_detail() {
    for detail in [
        BTreeMap::new(),
        BTreeMap::from([("reason".into(), "timeout".into())]),
    ] {
        for retry in [0, 17, u32::MAX] {
            let mut mapper = SessionMapper::new(7, "host-1");
            assert!(mapper
                .map(&SessionEvent::ErrorDetail {
                    code: ErrorCode::ApprovalDenied,
                    detail: detail.clone(),
                    retry_after_s: Some(retry),
                })
                .is_empty());
            let events = map_json(&mut mapper, error(ErrorCode::ApprovalDenied));
            assert_eq!(events[0]["retryAfterS"], retry);
            if detail.is_empty() {
                assert!(events[0].get("detail").is_none());
            } else {
                assert_eq!(events[0]["detail"], json!({"reason": "timeout"}));
            }
        }
    }
}

#[test]
fn consumed_detail_is_not_reused_and_error_deduplication_is_preserved() {
    let mut mapper = SessionMapper::new(7, "host-1");
    mapper.map(&timeout_detail(Some(4)));
    assert_eq!(mapper.map(&error(ErrorCode::ApprovalDenied)).len(), 1);
    mapper.map(&timeout_detail(Some(5)));
    assert!(mapper.map(&error(ErrorCode::ApprovalDenied)).is_empty());
    let events = map_json(
        &mut mapper,
        SessionEvent::Error {
            code: ErrorCode::ApprovalDenied,
            message: "another denial".into(),
        },
    );
    assert_eq!(events.as_array().unwrap().len(), 1);
    assert!(events[0].get("detail").is_none());
    assert!(events[0].get("retryAfterS").is_none());
}

#[test]
fn intervening_event_discards_context() {
    let mut mapper = SessionMapper::new(7, "host-1");
    mapper.map(&timeout_detail(Some(3)));
    mapper.map(&SessionEvent::StateChanged(
        SessionStateKind::Authenticating,
    ));
    let events = map_json(&mut mapper, error(ErrorCode::ApprovalDenied));
    assert!(events[0].get("detail").is_none());
    assert!(events[0].get("retryAfterS").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opening_with_wrong_pin_publishes_fingerprint_address_and_path() {
    let core = support::test_core_with(|_, options| options.worker_threads = 2);
    support::with_identity(&core, "Error detail test");
    let events = support::EventCollector::new();
    core.events().subscribe(events.clone());
    let host = TestHost::start_with(TestHostOptions {
        tcp: false,
        ..Default::default()
    })
    .await;
    core.store()
        .upsert_paired_host(&PairedHostRow {
            host_id: host.host_id.clone(),
            fp: [0; 32],
            name: "Reinstalled host".into(),
            addresses: vec![host.quic_addr],
            paired_at_us: core.now_us(),
            permission: Some("control".into()),
        })
        .expect("paired host with old pin");
    let id = api_sessions::open(
        &core,
        &host.host_id,
        OpenOpts {
            intent: OpenIntent::View,
            display_id: None,
            quality: None,
            mode: None,
            audio: None,
            max_fps: None,
            max_width: None,
            codec: None,
        },
    )
    .expect("session ID");
    let session_id = id.to_string();
    timeout(Duration::from_secs(10), async {
        loop {
            if core.state::<SessionTable>().is_empty()
                && events.of_type("session.state").iter().any(|event| {
                    event["sessionId"].as_str() == Some(session_id.as_str())
                        && event["phase"] == "ended"
                })
            {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("failed open deadline");
    assert!(core.events().flush(Duration::from_secs(1)));
    let errors = events.of_type("session.error");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["code"], 150);
    assert_eq!(errors[0]["sessionId"], session_id);
    assert_eq!(
        errors[0]["detail"],
        json!({
            "fp": fp_b64url(&host.fp),
            "addr": host.quic_addr.ip().to_string(),
            "path": "manual"
        })
    );
    assert!(errors[0].get("retryAfterS").is_none());
    core.shutdown();
}
