mod support;

use std::future::Future;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use host_fake::harness::{Approval, TestHost, TestHostOptions};
use rs_mobile::rscore::api_pairing::{
    cancel, ended_outcome, forget, open_error, paired, pairing_parse_qr, parse_qr, result_outcome,
    start,
};
use rs_mobile::rscore::dto_core::{PairOutcome, PairStartArg, PairTarget};
use rs_mobile::rscore::error::CoreError;
use rs_mobile::rscore::runtime::Core;
use rsp_client::session::{EndReason, SessionError};
use rsp_core::errors::ErrorCode;
use rsp_core::ids::{fp_b64url, id_from_fp};
use rsp_core::pb;
use serde_json::{json, Value};
use tokio::time::{sleep, timeout, Instant};

use support::{test_core_with, with_identity, EventCollector};

pub async fn within(future: impl Future<Output = ()>) {
    timeout(Duration::from_secs(9), future)
        .await
        .expect("pairing test completes within nine seconds");
}

pub fn configured_core() -> Arc<Core> {
    let core = test_core_with(|_, opts| {
        opts.timeouts.pairing = Duration::from_secs(6);
        opts.clock = Some(Arc::new(|| 1_792_465_134_000_000));
    });
    with_identity(&core, "Pairing test phone");
    core
}

pub fn collect(core: &Arc<Core>) -> Arc<EventCollector> {
    let events = EventCollector::new();
    core.events().subscribe(events.clone());
    events
}

pub fn assert_done(core: &Arc<Core>, events: &EventCollector, outcome: &PairOutcome) {
    assert!(core.events().flush(Duration::from_secs(1)));
    let done = events.of_type("pairing.done");
    assert_eq!(done.len(), 1);
    assert_eq!(done[0]["outcome"], serde_json::to_value(outcome).unwrap());
    assert_eq!(events.events().last().unwrap().1["type"], "pairing.done");
}

pub async fn wait_dialing(events: &EventCollector, count: usize) {
    timeout(Duration::from_secs(1), async {
        loop {
            if events
                .of_type("pairing.progress")
                .iter()
                .filter(|event| event["step"] == "dialing")
                .count()
                >= count
            {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("attempt reaches dialing");
}

pub fn silent_target() -> (UdpSocket, PairTarget) {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let fp = [7; 32];
    let target = PairTarget {
        v: 1,
        host_id: id_from_fp(&fp),
        fp: fp_b64url(&fp),
        code: "123456".to_owned(),
        name: "Silent Host".to_owned(),
        addrs: vec![socket.local_addr().unwrap().to_string()],
    };
    (socket, target)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn qr_parsing_uses_host_ticket_without_initializing_global_core() {
    within(async {
        let host = TestHost::start().await;
        let ticket = host.new_pairing_code();
        let target = parse_qr(&ticket.qr_url).unwrap();
        assert_eq!(target.host_id, host.host_id);
        assert_eq!(target.fp, fp_b64url(&host.fp));
        assert_eq!(target.code, ticket.code);
        assert_eq!(target.addrs, vec![host.quic_addr.to_string()]);
        let exported: Value =
            serde_json::from_str(&pairing_parse_qr(ticket.qr_url).unwrap()).unwrap();
        assert_eq!(exported, serde_json::to_value(&target).unwrap());
        assert!(parse_qr("rsp://pair?v=2").is_none());
        assert!(parse_qr("hello").is_none());
        assert!(pairing_parse_qr("hello".to_owned()).is_none());
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn qr_success_emits_ordered_events_persists_and_forgets() {
    within(async {
        let host = TestHost::start().await;
        let core = configured_core();
        let events = collect(&core);
        let extra = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut target = parse_qr(&host.new_pairing_code().qr_url).unwrap();
        target.addrs.push(host.quic_addr.to_string());
        target.addrs.push(extra.local_addr().unwrap().to_string());
        let outcome = start(&core, PairStartArg::Target(target)).await.unwrap();
        let value = serde_json::to_value(&outcome).unwrap();
        assert_eq!(value["status"], "ok");
        assert_eq!(value["hostId"], host.host_id);
        assert!(value["hostName"]
            .as_str()
            .is_some_and(|name| !name.is_empty()));
        assert!(value.get("permission").is_none());
        assert!(value["latencyMs"].is_u64());
        assert_done(&core, &events, &outcome);
        let progress = events.of_type("pairing.progress");
        // The fixture's immediate Allow approver still sends WAITING_LOCAL.
        assert_eq!(
            progress
                .iter()
                .map(|event| event["step"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["dialing", "verifying", "exchanging", "waitingLocal"]
        );
        assert_eq!(progress[1]["fp"], fp_b64url(&host.fp));
        assert_eq!(progress[1]["latencyMs"], value["latencyMs"]);
        assert_eq!(progress[2]["path"], value["path"]);
        // Pairing ServerHello deliberately withholds the Host name; the final
        // PairResult is the authoritative source of the persisted name.
        assert_eq!(progress[2]["hostName"], "");
        assert_eq!(progress[3]["timeoutS"], 30);
        assert!(progress[3]["hostName"].is_string());
        let hosts = paired(&core).unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].host_id, host.host_id);
        assert_eq!(hosts[0].fp, fp_b64url(&host.fp));
        assert_eq!(hosts[0].name, value["hostName"]);
        assert_eq!(hosts[0].paired_at, core.now_us());
        assert_eq!(
            hosts[0].addresses,
            vec![
                host.quic_addr.to_string(),
                extra.local_addr().unwrap().to_string()
            ]
        );
        assert!(core
            .store()
            .paired_host(&host.host_id)
            .unwrap()
            .unwrap()
            .permission
            .is_none());
        forget(&core, &host.host_id).unwrap();
        assert!(paired(&core).unwrap().is_empty());
        forget(&core, &host.host_id).unwrap();
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn wrong_code_reports_remaining_attempts() {
    within(async {
        let host = TestHost::start().await;
        let core = configured_core();
        let events = collect(&core);
        let mut target = parse_qr(&host.new_pairing_code().qr_url).unwrap();
        target.code = if target.code == "000000" {
            "000001"
        } else {
            "000000"
        }
        .to_owned();
        let outcome = start(&core, PairStartArg::Target(target)).await.unwrap();
        assert_eq!(
            serde_json::to_value(&outcome).unwrap(),
            json!({"status":"badCode", "attemptsLeft":4})
        );
        assert_done(&core, &events, &outcome);
        assert!(paired(&core).unwrap().is_empty());
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn local_denial_does_not_save_host() {
    within(async {
        let host = TestHost::start_with(TestHostOptions {
            pairing_approval: Some(Approval::Deny),
            ..TestHostOptions::default()
        })
        .await;
        let core = configured_core();
        let events = collect(&core);
        let outcome = start(
            &core,
            PairStartArg::Target(parse_qr(&host.new_pairing_code().qr_url).unwrap()),
        )
        .await
        .unwrap();
        assert_eq!(outcome, PairOutcome::Denied);
        assert_done(&core, &events, &outcome);
        assert!(paired(&core).unwrap().is_empty());
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn tls_fingerprint_mismatch_is_mitm() {
    within(async {
        let host = TestHost::start().await;
        let core = configured_core();
        let events = collect(&core);
        let mut target = parse_qr(&host.new_pairing_code().qr_url).unwrap();
        let fp = [9; 32];
        target.fp = fp_b64url(&fp);
        target.host_id = id_from_fp(&fp);
        let outcome = start(&core, PairStartArg::Target(target)).await.unwrap();
        assert_eq!(outcome, PairOutcome::Mitm);
        assert_done(&core, &events, &outcome);
        assert!(paired(&core).unwrap().is_empty());
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn invalid_targets_finish_without_dialing() {
    within(async {
        let (_socket, target) = silent_target();
        let mut invalid = Vec::new();
        let mut changed = target.clone();
        changed.host_id = id_from_fp(&[8; 32]);
        invalid.push(changed);
        let mut changed = target.clone();
        changed.v = 2;
        invalid.push(changed);
        let mut changed = target.clone();
        changed.fp = "invalid".to_owned();
        invalid.push(changed);
        for code in ["12345", "1234567", "12345a", "１２３４５６"] {
            let mut changed = target.clone();
            changed.code = code.to_owned();
            invalid.push(changed);
        }
        for target in invalid {
            let core = configured_core();
            let events = collect(&core);
            let outcome = start(&core, PairStartArg::Target(target)).await.unwrap();
            assert!(matches!(outcome, PairOutcome::Error { .. }));
            assert_done(&core, &events, &outcome);
            assert!(events.of_type("pairing.progress").is_empty());
        }
        let core = configured_core();
        let events = collect(&core);
        let mut target = target;
        target.addrs.clear();
        let outcome = start(&core, PairStartArg::Target(target)).await.unwrap();
        assert!(matches!(
            outcome,
            PairOutcome::Error {
                code: Some(110),
                ..
            }
        ));
        assert_done(&core, &events, &outcome);
        assert!(events.of_type("pairing.progress").is_empty());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn cancel_interrupts_silent_dial_within_half_a_second() {
    within(async {
        let (_socket, target) = silent_target();
        let core = configured_core();
        let events = collect(&core);
        let other = core.clone();
        let task = tokio::spawn(async move { start(&other, PairStartArg::Target(target)).await });
        wait_dialing(&events, 1).await;
        sleep(Duration::from_millis(200)).await;
        let cancelled_at = Instant::now();
        cancel(&core);
        let outcome = timeout(Duration::from_millis(500), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(cancelled_at.elapsed() < Duration::from_millis(500));
        assert_eq!(outcome, PairOutcome::Cancelled);
        assert_done(&core, &events, &outcome);
        cancel(&core);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn pairing_deadline_interrupts_silent_dial() {
    within(async {
        let (_socket, target) = silent_target();
        let core = test_core_with(|_, opts| opts.timeouts.pairing = Duration::from_secs(1));
        with_identity(&core, "Timeout phone");
        let events = collect(&core);
        let started = Instant::now();
        let outcome = start(&core, PairStartArg::Target(target)).await.unwrap();
        assert_eq!(outcome, PairOutcome::Timeout);
        assert!(started.elapsed() >= Duration::from_secs(1));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_done(&core, &events, &outcome);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn replacement_cancels_old_attempt_without_clearing_new_cancellation() {
    within(async {
        let (_socket, target) = silent_target();
        let core = configured_core();
        let events = collect(&core);
        let first_core = core.clone();
        let first_arg = PairStartArg::Target(target.clone());
        let first = tokio::spawn(async move { start(&first_core, first_arg).await });
        wait_dialing(&events, 1).await;
        let second_core = core.clone();
        let second =
            tokio::spawn(async move { start(&second_core, PairStartArg::Target(target)).await });
        wait_dialing(&events, 2).await;
        assert_eq!(
            timeout(Duration::from_millis(500), first)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            PairOutcome::Cancelled
        );
        cancel(&core);
        assert_eq!(
            timeout(Duration::from_millis(500), second)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            PairOutcome::Cancelled
        );
        assert!(core.events().flush(Duration::from_secs(1)));
        let done = events.of_type("pairing.done");
        assert_eq!(done.len(), 2);
        assert!(done
            .iter()
            .all(|event| event["outcome"]["status"] == "cancelled"));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn identity_failure_rejects_without_done() {
    within(async {
        let (_socket, target) = silent_target();
        let core = test_core_with(|_, _| {});
        let events = collect(&core);
        let error = start(&core, PairStartArg::Target(target))
            .await
            .unwrap_err();
        assert!(matches!(error, CoreError::Internal { .. }));
        assert!(core.events().flush(Duration::from_secs(1)));
        assert!(events.events().is_empty());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn expired_ticket_is_not_saved() {
    within(async {
        let now = Arc::new(AtomicU64::new(1_000_000));
        let clock = now.clone();
        let host = TestHost::start_with(TestHostOptions {
            pairing_clock: Some(Arc::new(move || clock.load(Ordering::Relaxed))),
            ..TestHostOptions::default()
        })
        .await;
        let ticket = host.new_pairing_code();
        now.store(ticket.expires_at, Ordering::Relaxed);
        host.security.pairing.code_state(ticket.expires_at).unwrap();
        let core = configured_core();
        let events = collect(&core);
        let outcome = start(
            &core,
            PairStartArg::Target(parse_qr(&ticket.qr_url).unwrap()),
        )
        .await
        .unwrap();
        assert_eq!(outcome, PairOutcome::Expired);
        assert_done(&core, &events, &outcome);
        assert!(paired(&core).unwrap().is_empty());
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn terminal_mapping_preserves_policy_results_and_protocol_mitm_default() {
    within(async {
        for (status, expected) in [
            (pb::pair_result::Status::Locked, PairOutcome::Locked { retry_after_s: None }),
            (pb::pair_result::Status::MethodDisabled, PairOutcome::MethodDisabled),
            (pb::pair_result::Status::DisabledOnPath, PairOutcome::DisabledOnPath),
        ] {
            assert_eq!(result_outcome(pb::PairResult { status: status as i32, ..Default::default() }, "host", None, 0), Some(expected));
        }
        assert!(matches!(result_outcome(pb::PairResult { status: pb::pair_result::Status::Ok as i32, host_id: "other".to_owned(), ..Default::default() }, "host", None, 0), Some(PairOutcome::Error { message, .. }) if message == "host id mismatch"));
        assert_eq!(ended_outcome(Some(ErrorCode::Other), EndReason::Protocol, "details are not exposed".to_owned()), PairOutcome::Mitm);
        assert_eq!(open_error(SessionError::Dial { code: ErrorCode::FingerprintMismatch, detail: Default::default() }), PairOutcome::Mitm);
        assert!(matches!(open_error(SessionError::Dial { code: ErrorCode::Offline, detail: Default::default() }), PairOutcome::Error { code: Some(110), .. }));
        assert_eq!(ended_outcome(Some(ErrorCode::PolicyDenied), EndReason::Error(ErrorCode::PolicyDenied), "policy reason".to_owned()), PairOutcome::Error { code: Some(ErrorCode::PolicyDenied as u32), message: "policy reason".to_owned() });
    }).await;
}
