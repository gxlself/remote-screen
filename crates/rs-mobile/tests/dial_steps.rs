mod support;

use std::{
    net::{IpAddr, SocketAddr, UdpSocket},
    sync::Arc,
    time::Duration,
};

use host_fake::{
    harness::{TestHost, TestHostOptions},
    FakePlatform,
};
use rs_mobile::rscore::{
    api_sessions as sessions,
    dto_core::{DialAttempt, DialState, NetPath, OpenIntent, OpenOpts, SessionPhase},
    runtime::Core,
    sessmap::{dial_from_plan, dial_from_report, SessionMapper},
    store::PairedHostRow,
    tables::SessionTable,
};
use rsp_client::{session::DialReport, EndReason, SessionEvent, SessionStateKind};
use rsp_core::trust::Permission;
use rsp_transport::{
    dial::{AttemptLog, AttemptResult, Candidate, CandidateSource, DialPlan, PathKind, Skipped},
    TransportKind,
};
use serde_json::{json, Value};
use tokio::time::{sleep, timeout, Instant};

const SECOND: Duration = Duration::from_secs(1);
const TWO_SECONDS: Duration = Duration::from_secs(2);

fn candidate(addr: &str, kind: PathKind) -> Candidate {
    Candidate {
        addr: addr.parse().unwrap(),
        kind,
        source: CandidateSource::Cache,
    }
}

fn log(
    candidate: &Candidate,
    transport: Option<TransportKind>,
    result: AttemptResult,
    started_at_us: Option<u64>,
    finished_at_us: Option<u64>,
) -> AttemptLog {
    AttemptLog {
        candidate: candidate.clone(),
        transport,
        result,
        started_at_us,
        finished_at_us,
    }
}

fn attempt(
    path: NetPath,
    addr: &str,
    state: DialState,
    ms: Option<u32>,
    reason: Option<&str>,
) -> DialAttempt {
    DialAttempt {
        path,
        addr: addr.into(),
        state,
        ms,
        reason: reason.map(Into::into),
    }
}

#[test]
fn plan_lists_tried_candidates_then_skipped_ones_by_ip() {
    let plan = DialPlan {
        candidates: vec![
            candidate(
                "10.1.2.3:7000",
                PathKind::LastGood(Box::new(PathKind::Intranet)),
            ),
            candidate("203.0.113.5:7001", PathKind::Manual),
        ],
        skipped: vec![
            (
                candidate("192.168.1.20:7000", PathKind::Lan),
                Skipped::NotOnLan,
            ),
            (
                candidate("10.20.0.4:7000", PathKind::Intranet),
                Skipped::NeedsVpn,
            ),
        ],
        derp_only: false,
    };
    let dial = dial_from_plan(&plan);
    assert_eq!(
        dial,
        vec![
            attempt(NetPath::Intranet, "10.1.2.3", DialState::Trying, None, None),
            attempt(
                NetPath::Manual,
                "203.0.113.5",
                DialState::Trying,
                None,
                None
            ),
            attempt(
                NetPath::Lan,
                "192.168.1.20",
                DialState::Skipped,
                None,
                Some("NotOnLan")
            ),
            attempt(
                NetPath::Intranet,
                "10.20.0.4",
                DialState::Skipped,
                None,
                Some("NeedsVpn")
            ),
        ]
    );
    assert!(dial
        .iter()
        .all(|attempt| attempt.addr.parse::<IpAddr>().is_ok()));
    assert_eq!(
        serde_json::to_value(&dial[2]).unwrap(),
        json!({"path": "lan", "addr": "192.168.1.20", "state": "skipped", "reason": "NotOnLan"})
    );
    assert!(dial_from_plan(&DialPlan::default()).is_empty());
}

#[test]
fn report_merges_attempts_per_candidate_in_first_seen_order() {
    let failed = candidate("10.1.2.3:7000", PathKind::Intranet);
    let primary = candidate(
        "192.168.1.20:7000",
        PathKind::LastGood(Box::new(PathKind::Lan)),
    );
    let backup = candidate("100.64.0.7:7000", PathKind::Tailnet);
    let mismatch = candidate("203.0.113.5:7000", PathKind::Manual);
    let late = candidate("203.0.113.6:7000", PathKind::Manual);
    let unstarted = candidate("203.0.113.7:7000", PathKind::Manual);
    let skipped = candidate("10.20.0.4:7000", PathKind::Intranet);
    let got = [0xab; 32];
    let report = DialReport {
        attempts: vec![
            log(
                &failed,
                Some(TransportKind::Quic),
                AttemptResult::TimedOut,
                Some(1_000),
                Some(3_001_000),
            ),
            log(
                &primary,
                Some(TransportKind::Quic),
                AttemptResult::Cancelled,
                Some(25_000),
                Some(1_300_000),
            ),
            log(
                &failed,
                Some(TransportKind::Tcp),
                AttemptResult::Refused,
                Some(250_000),
                Some(257_000),
            ),
            log(
                &primary,
                Some(TransportKind::Tcp),
                AttemptResult::Connected,
                Some(1_270_000),
                Some(1_275_000),
            ),
            log(
                &backup,
                Some(TransportKind::Quic),
                AttemptResult::Connected,
                Some(50_000),
                Some(1_290_000),
            ),
            log(
                &mismatch,
                Some(TransportKind::Quic),
                AttemptResult::FingerprintMismatch { got },
                Some(75_000),
                Some(90_000),
            ),
            log(
                &late,
                Some(TransportKind::Quic),
                AttemptResult::TimedOut,
                None,
                Some(3_000_000),
            ),
            log(
                &unstarted,
                Some(TransportKind::Quic),
                AttemptResult::NotStarted,
                None,
                None,
            ),
            log(
                &unstarted,
                Some(TransportKind::Tcp),
                AttemptResult::NotStarted,
                None,
                None,
            ),
            log(
                &skipped,
                None,
                AttemptResult::Skipped(Skipped::NeedsVpn),
                None,
                None,
            ),
        ],
        primary: Some(primary.clone()),
        transport: Some(TransportKind::Tcp),
    };
    let dial = dial_from_report(&report);
    assert_eq!(
        dial,
        vec![
            attempt(
                NetPath::Intranet,
                "10.1.2.3",
                DialState::Failed,
                Some(7),
                Some("Refused")
            ),
            attempt(NetPath::Lan, "192.168.1.20", DialState::Ok, Some(5), None),
            attempt(
                NetPath::TailnetDirect,
                "100.64.0.7",
                DialState::Skipped,
                None,
                Some("Backup")
            ),
            attempt(
                NetPath::Manual,
                "203.0.113.5",
                DialState::Failed,
                Some(15),
                Some("FingerprintMismatch")
            ),
            attempt(
                NetPath::Manual,
                "203.0.113.6",
                DialState::Failed,
                None,
                Some("TimedOut")
            ),
            attempt(
                NetPath::Manual,
                "203.0.113.7",
                DialState::Skipped,
                None,
                Some("NotStarted")
            ),
            attempt(
                NetPath::Intranet,
                "10.20.0.4",
                DialState::Skipped,
                None,
                Some("NeedsVpn")
            ),
        ]
    );
    let text = serde_json::to_string(&dial).unwrap();
    assert!(!text.contains(&rsp_core::ids::fp_b64url(&got)));
    assert!(!text.contains("got"));
    assert!(dial_from_report(&DialReport::default()).is_empty());
}

#[test]
fn report_marks_a_cancelled_only_candidate_skipped() {
    let primary = candidate("192.168.1.20:7000", PathKind::Lan);
    let slower = candidate("203.0.113.5:7000", PathKind::Manual);
    let report = DialReport {
        attempts: vec![
            log(
                &primary,
                Some(TransportKind::Quic),
                AttemptResult::Connected,
                Some(0),
                Some(5_999),
            ),
            log(
                &primary,
                Some(TransportKind::Tcp),
                AttemptResult::NotStarted,
                None,
                None,
            ),
            log(
                &slower,
                Some(TransportKind::Quic),
                AttemptResult::Cancelled,
                Some(25_000),
                Some(36_000),
            ),
            log(
                &slower,
                Some(TransportKind::Tcp),
                AttemptResult::NotStarted,
                None,
                None,
            ),
        ],
        primary: Some(primary.clone()),
        transport: Some(TransportKind::Quic),
    };
    assert_eq!(
        dial_from_report(&report),
        vec![
            attempt(NetPath::Lan, "192.168.1.20", DialState::Ok, Some(5), None),
            attempt(
                NetPath::Manual,
                "203.0.113.5",
                DialState::Skipped,
                None,
                Some("Cancelled")
            ),
        ]
    );
}

fn dial_list() -> Vec<DialAttempt> {
    vec![
        attempt(NetPath::Lan, "192.168.1.20", DialState::Ok, Some(5), None),
        attempt(
            NetPath::Manual,
            "203.0.113.5",
            DialState::Skipped,
            None,
            Some("Cancelled"),
        ),
    ]
}

fn state(mapper: &mut SessionMapper, state: SessionStateKind) -> Value {
    let events = serde_json::to_value(mapper.map(&SessionEvent::StateChanged(state))).unwrap();
    assert_eq!(events.as_array().unwrap().len(), 1, "{events}");
    events[0].clone()
}

#[test]
fn mapper_attaches_dial_until_streaming() {
    let mut mapper = SessionMapper::new(7, "host-1");
    mapper.set_dial(dial_list());
    let expected = serde_json::to_value(dial_list()).unwrap();
    assert_eq!(
        state(&mut mapper, SessionStateKind::Authenticating)["dial"],
        expected
    );
    assert_eq!(
        state(&mut mapper, SessionStateKind::PendingApproval)["dial"],
        expected
    );
    let streaming = state(&mut mapper, SessionStateKind::Streaming);
    assert!(streaming.get("dial").is_none(), "{streaming}");
    let reconnecting = state(&mut mapper, SessionStateKind::Reconnecting);
    assert!(reconnecting.get("dial").is_none(), "{reconnecting}");
    let approval = state(&mut mapper, SessionStateKind::PendingApproval);
    assert!(approval.get("dial").is_none(), "{approval}");
}

#[test]
fn empty_dial_leaves_state_events_unchanged() {
    let mut mapper = SessionMapper::new(7, "host-1");
    mapper.set_dial(vec![]);
    assert_eq!(
        serde_json::to_value(mapper.map(&SessionEvent::StateChanged(
            SessionStateKind::Authenticating
        )))
        .unwrap(),
        json!([
            {"type": "session.state", "sessionId": "7", "hostId": "host-1", "phase": "authenticating"}
        ])
    );
}

#[test]
fn resume_and_terminal_states_drop_the_dial() {
    let mut mapper = SessionMapper::new(7, "host-1");
    mapper.set_dial(dial_list());
    let resume = serde_json::to_value(mapper.begin_resume()).unwrap();
    assert!(resume.get("dial").is_none(), "{resume}");
    let approval = state(&mut mapper, SessionStateKind::PendingApproval);
    assert!(approval.get("dial").is_none(), "{approval}");

    let mut streaming = SessionMapper::new(7, "host-1");
    state(&mut streaming, SessionStateKind::Authenticating);
    state(&mut streaming, SessionStateKind::Streaming);
    streaming.set_dial(dial_list());
    let approval = state(&mut streaming, SessionStateKind::PendingApproval);
    assert!(approval.get("dial").is_none(), "{approval}");

    let mut ending = SessionMapper::new(7, "host-1");
    ending.set_dial(dial_list());
    let ended = state(&mut ending, SessionStateKind::Ended);
    assert!(ended.get("dial").is_none(), "{ended}");
    ending.set_dial(dial_list());
    let after = state(&mut ending, SessionStateKind::Authenticating);
    assert!(after.get("dial").is_none(), "{after}");

    let mut closed = SessionMapper::new(7, "host-1");
    closed.map(&SessionEvent::Ended {
        code: None,
        reason: "connection lost".into(),
        kind: EndReason::ConnectionLost,
    });
    assert!(closed.snapshot().ended);
    closed.set_dial(dial_list());
    assert!(closed
        .map(&SessionEvent::StateChanged(
            SessionStateKind::Authenticating
        ))
        .is_empty());
}

fn opts(intent: OpenIntent) -> OpenOpts {
    OpenOpts {
        intent,
        display_id: None,
        quality: None,
        mode: None,
        audio: None,
        max_fps: None,
        max_width: None,
        codec: None,
    }
}

async fn until(limit: Duration, predicate: impl Fn() -> bool) {
    timeout(limit, async {
        while !predicate() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition before deadline");
}

fn new_core() -> (Arc<Core>, Arc<support::EventCollector>) {
    let core = support::test_core_with(|_, options| options.worker_threads = 2);
    support::with_identity(&core, "Session test device");
    let events = support::EventCollector::new();
    core.events().subscribe(events.clone());
    (core, events)
}

fn pair_row(core: &Arc<Core>, host_id: &str, fp: [u8; 32], addresses: Vec<SocketAddr>) {
    core.store()
        .upsert_paired_host(&PairedHostRow {
            host_id: host_id.into(),
            fp,
            name: "Paired test host".into(),
            addresses,
            paired_at_us: core.now_us(),
            permission: Some("control".into()),
        })
        .unwrap();
}

struct Fixture {
    core: Arc<Core>,
    events: Arc<support::EventCollector>,
    host: TestHost,
    // Bound but silent: QUIC Initial packets to it are never answered.
    _silent: UdpSocket,
}

impl Fixture {
    async fn new() -> Self {
        let (core, events) = new_core();
        let host = TestHost::start_with_platform(
            FakePlatform::new(),
            TestHostOptions {
                tcp: false,
                ..Default::default()
            },
        )
        .await;
        host.pair(&core.local_identity().unwrap(), Permission::Control);
        let silent = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        pair_row(
            &core,
            &host.host_id,
            host.fp,
            vec![silent.local_addr().unwrap(), host.quic_addr],
        );
        Self {
            core,
            events,
            host,
            _silent: silent,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for entry in self.core.state::<SessionTable>().list() {
            sessions::close(&self.core, entry.id);
        }
        self.core.shutdown();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_states_show_planned_then_used_paths() {
    let fixture = Fixture::new().await;
    let started = Instant::now();
    let id = sessions::open(&fixture.core, &fixture.host.host_id, opts(OpenIntent::View)).unwrap();
    assert!(started.elapsed() < Duration::from_millis(100));
    until(TWO_SECONDS, || {
        let summaries = sessions::list(&fixture.core);
        summaries.len() == 1 && summaries[0].phase == SessionPhase::Streaming
    })
    .await;
    assert!(fixture.core.events().flush(SECOND));
    let session_id = id.to_string();
    let states: Vec<_> = fixture
        .events
        .of_type("session.state")
        .into_iter()
        .filter(|event| event["sessionId"] == session_id.as_str())
        .collect();
    let phases: Vec<_> = states
        .iter()
        .map(|event| event["phase"].as_str().unwrap())
        .collect();
    assert_eq!(phases, ["connecting", "authenticating", "streaming"]);

    let connecting = states[0]["dial"].as_array().expect("connecting dial");
    assert_eq!(connecting.len(), 2, "{connecting:?}");
    assert!(
        connecting.iter().all(|entry| entry["state"] == "trying"),
        "{connecting:?}"
    );
    assert!(connecting
        .iter()
        .all(|entry| entry["addr"] == "127.0.0.1" && entry["path"] == "manual"));

    let authenticating = states[1]["dial"].as_array().expect("authenticating dial");
    assert_eq!(authenticating.len(), 2, "{authenticating:?}");
    let ok: Vec<_> = authenticating
        .iter()
        .filter(|entry| entry["state"] == "ok")
        .collect();
    assert_eq!(ok.len(), 1, "{authenticating:?}");
    assert!(ok[0]["ms"].is_u64(), "{authenticating:?}");
    assert!(
        authenticating
            .iter()
            .all(|entry| entry["state"] != "trying"),
        "{authenticating:?}"
    );

    assert!(states[2].get("dial").is_none(), "{}", states[2]);

    sessions::close(&fixture.core, id);
    until(TWO_SECONDS, || {
        fixture.core.state::<SessionTable>().is_empty()
    })
    .await;
}
