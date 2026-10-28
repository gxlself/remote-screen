//! Pairing results are delivered to the Host UI event sink for code pairing.

use async_trait::async_trait;
use host_core::{
    ipc::Event,
    pairing::{PairApproval, PairApprover, PairingManager},
    HostRuntime, RuntimeOptions,
};
use rsp_core::{pairing::AppPairing, pb};
use std::sync::Arc;

mod wire_support;
use wire_support::{client_hello, identity, Dev, WireHost};

struct Allow;

#[async_trait]
impl PairApprover for Allow {
    async fn request(&self, _request: host_core::pairing::PairApprovalRequest) -> PairApproval {
        PairApproval {
            allow: true,
            permission: rsp_core::trust::Permission::Control,
        }
    }
}

struct RuntimeTask(tokio::task::JoinHandle<host_core::RuntimeResult<()>>);

impl Drop for RuntimeTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn start_pairing_runtime(host: &mut WireHost) -> (RuntimeTask, PairingManager) {
    let pairing = PairingManager::new(host.store.clone(), host.host_fp, "fake-host", Vec::new());
    let bound = HostRuntime::new(
        host.platform.clone(),
        RuntimeOptions {
            bind: Some("127.0.0.1:0".parse().expect("loopback")),
            store_dir: host.state_dir.join("store"),
            ..Default::default()
        },
    )
    .with_manager(host.mgr.clone())
    .with_store(host.store.clone())
    .with_pairing_manager(pairing)
    .with_pairing_approver(Arc::new(Allow))
    .bind()
    .await
    .expect("pairing listener");
    let addr = bound
        .listener
        .bound()
        .into_iter()
        .find(|(_, _, kind)| *kind == rsp_transport::TransportKind::Quic)
        .expect("quic listener")
        .0;
    host.addrs[0] = addr;
    let pairing = bound.security.pairing.clone();
    let task = RuntimeTask(tokio::spawn(bound.run()));
    (task, pairing)
}

async fn exchange(host: &mut WireHost, code: &str) -> pb::PairResult {
    let mut client = host.connect_as(Dev::Unpaired, 0).await;
    client
        .send(pb::ClientHello {
            intent: pb::Intent::Pair as i32,
            ..client_hello()
        })
        .await;
    client
        .next_matching(|message| matches!(message, pb::envelope::Msg::ServerHello(_)).then_some(()))
        .await;
    let mut app = AppPairing::new(
        code,
        identity(Dev::Unpaired).fingerprint(),
        host.host_fp,
        "pairing phone",
        pb::Platform::Ios,
    )
    .expect("app pairing");
    client.send(app.try_start().expect("pair start")).await;
    let spake = client
        .next_matching(|message| match message {
            pb::envelope::Msg::PairSpake(value) => Some(value.clone()),
            _ => None,
        })
        .await;
    let (spake, confirm) = app.on_spake(spake).expect("app spake");
    client.send(spake).await;
    client.send(confirm).await;
    let result = match client.next().await {
        pb::envelope::Msg::PairConfirm(confirm) => {
            app.on_confirm(confirm).expect("app confirm");
            let waiting = client.next().await;
            assert!(matches!(
                waiting,
                pb::envelope::Msg::PairResult(ref result)
                    if result.status == pb::pair_result::Status::WaitingLocal as i32
            ));
            client
                .next_matching(|message| match message {
                    pb::envelope::Msg::PairResult(value) => Some(value.clone()),
                    _ => None,
                })
                .await
        }
        pb::envelope::Msg::PairResult(result) => result,
        other => panic!("unexpected pairing response: {other:?}"),
    };
    let _ = client.close_code().await;
    result
}

fn pairing_events(host: &WireHost) -> Vec<serde_json::Value> {
    host.events
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::PairingResult(payload) => Some(payload),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn code_pairing_success_emits_one_ui_result() {
    let mut host = WireHost::start().await;
    let (task, manager) = start_pairing_runtime(&mut host).await;
    let ticket = manager
        .start(rsp_core::clock::unix_micros())
        .expect("pair ticket");
    let result = exchange(&mut host, &ticket.code).await;
    assert_eq!(result.status, pb::pair_result::Status::Ok as i32);
    let events = pairing_events(&host);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["status"], result.status);
    assert_eq!(events[0]["attempts_left"], result.attempts_left);
    assert_eq!(events[0]["method"], "code");
    assert_eq!(
        events[0]["device_fp"],
        rsp_core::ids::fp_b64url(&identity(Dev::Unpaired).fingerprint())
    );
    assert_eq!(events[0]["device_name"], "pairing phone");
    drop(task);
}

#[tokio::test]
async fn code_pairing_failure_emits_finish_result_with_attempts_left() {
    let mut host = WireHost::start().await;
    let (task, manager) = start_pairing_runtime(&mut host).await;
    let ticket = manager
        .start(rsp_core::clock::unix_micros())
        .expect("pair ticket");
    let wrong = if ticket.code == "000000" {
        "999999"
    } else {
        "000000"
    };
    let result = exchange(&mut host, wrong).await;
    assert_ne!(result.status, pb::pair_result::Status::Ok as i32);
    let events = pairing_events(&host);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["status"], result.status);
    assert_eq!(events[0]["attempts_left"], result.attempts_left);
    assert_eq!(events[0]["method"], "code");
    assert_eq!(events[0]["device_name"], "pairing phone");
    drop(task);
}
