//! Connections waiting for local approval must keep the update gate closed.
use async_trait::async_trait;
use host_core::{
    session::{
        handlers::roster,
        handshake::{ConnectionApprovalOutcome, ConnectionApprovalRequest, ConnectionApprover},
        roster::LINGER_US,
    },
    update::{agent::MgrGate, install::SessionGate},
};
use rsp_core::pb;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{mpsc, oneshot},
    time::{timeout, Instant},
};

mod wire_support;
use wire_support::{Dev, WireClient, WireHost, WireOptions};

type ApprovalReply = oneshot::Sender<ConnectionApprovalOutcome>;

struct HoldApprover {
    calls: mpsc::UnboundedSender<ApprovalReply>,
}

#[async_trait]
impl ConnectionApprover for HoldApprover {
    async fn request(&self, _: ConnectionApprovalRequest) -> bool {
        unreachable!("wire_pendinggate uses the deadline-aware approval path")
    }

    async fn request_until(
        &self,
        _: ConnectionApprovalRequest,
        _: Instant,
    ) -> ConnectionApprovalOutcome {
        let (reply, answer) = oneshot::channel();
        if self.calls.send(reply).is_err() {
            return ConnectionApprovalOutcome::Rejected;
        }
        answer.await.unwrap_or(ConnectionApprovalOutcome::Rejected)
    }
}

fn hold_options() -> (WireOptions, mpsc::UnboundedReceiver<ApprovalReply>) {
    let (calls, requests) = mpsc::unbounded_channel();
    let mut options = WireOptions::default();
    options.options.auth_policy.attended_mode = true;
    options.connection_approver = Some(Arc::new(HoldApprover { calls }));
    (options, requests)
}

async fn hold(requests: &mut mpsc::UnboundedReceiver<ApprovalReply>) -> ApprovalReply {
    timeout(Duration::from_secs(3), requests.recv())
        .await
        .expect("approval request timed out")
        .expect("approval channel closed")
}

async fn auth(client: &mut WireClient, status: pb::auth_result::Status) -> pb::AuthResult {
    let result = client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::AuthResult(result) => Some(result.clone()),
            _ => None,
        })
        .await;
    assert_eq!(result.status, status as i32);
    result
}

async fn wait_for_idle(host: &WireHost) {
    timeout(Duration::from_secs(2), async {
        while roster::active_sessions(&host.mgr) != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("pending connection count was not released");
}

#[tokio::test]
async fn pending_approval_counts_as_active() {
    let (options, mut requests) = hold_options();
    let host = WireHost::start_with(options).await;
    let mut client = host.connect_as(Dev::A, 0).await;
    assert_eq!(
        client.hello().await.1.status,
        pb::auth_result::Status::PendingApproval as i32
    );
    let reply = hold(&mut requests).await;
    assert_eq!(roster::active_sessions(&host.mgr), 1);
    assert!(host.mgr.conns().is_empty());

    reply.send(ConnectionApprovalOutcome::Rejected).unwrap();
    let result = auth(&mut client, pb::auth_result::Status::Denied).await;
    assert_eq!(result.error.unwrap().code, 140);
    wait_for_idle(&host).await;
}

#[tokio::test]
async fn approved_connection_keeps_blocking_after_accept() {
    let (mut options, mut requests) = hold_options();
    options.clock = Some(Arc::new(wire_support::OffsetClock::manual()));
    let host = WireHost::start_with(options).await;
    let mut client = host.connect_as(Dev::A, 0).await;
    assert_eq!(
        client.hello().await.1.status,
        pb::auth_result::Status::PendingApproval as i32
    );
    let reply = hold(&mut requests).await;
    reply.send(ConnectionApprovalOutcome::Allowed).unwrap();
    auth(&mut client, pb::auth_result::Status::Ok).await;
    assert_eq!(roster::active_sessions(&host.mgr), 1);

    client.goodbye(pb::goodbye::Reason::Background).await;
    host.wait_until(|| host.mgr.conns().is_empty()).await;
    assert_eq!(roster::active_sessions(&host.mgr), 1);
    host.clock.skip(Duration::from_micros(LINGER_US + 1));
    host.wait_until(|| roster::active_sessions(&host.mgr) == 0)
        .await;
}

#[tokio::test]
async fn client_drop_while_pending_releases_count() {
    let (options, mut requests) = hold_options();
    let host = WireHost::start_with(options).await;
    let mut client = host.connect_as(Dev::A, 0).await;
    assert_eq!(
        client.hello().await.1.status,
        pb::auth_result::Status::PendingApproval as i32
    );
    let reply = hold(&mut requests).await;
    assert_eq!(roster::active_sessions(&host.mgr), 1);

    drop(client);
    wait_for_idle(&host).await;
    assert!(reply.send(ConnectionApprovalOutcome::Allowed).is_err());
    assert!(requests.try_recv().is_err());
}

#[tokio::test]
async fn approval_timeout_releases_count() {
    let (options, mut requests) = hold_options();
    let host = WireHost::start_with(options).await;
    let mut client = host.connect_as(Dev::A, 0).await;
    assert_eq!(
        client.hello().await.1.status,
        pb::auth_result::Status::PendingApproval as i32
    );
    let _reply = hold(&mut requests).await;

    // The transport has a 10 second idle timeout. Keep it active while the
    // 30 second approval deadline advances, just as a waiting app does.
    let connection = client.conn.clone();
    let keep_alive = tokio::spawn(async move {
        loop {
            let _ = connection.send_datagram(vec![0xff].into());
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    tokio::time::pause();
    tokio::time::sleep(Duration::from_secs(31)).await;
    tokio::time::resume();
    keep_alive.abort();
    let result = auth(&mut client, pb::auth_result::Status::Denied).await;
    let error = result.error.unwrap();
    assert_eq!(error.code, 140);
    assert_eq!(error.detail["reason"], "timeout");
    wait_for_idle(&host).await;
}

#[tokio::test]
async fn update_gate_waits_for_pending_approval() {
    let (options, mut requests) = hold_options();
    let host = WireHost::start_with(options).await;
    let mut client = host.connect_as(Dev::A, 0).await;
    assert_eq!(
        client.hello().await.1.status,
        pb::auth_result::Status::PendingApproval as i32
    );
    let reply = hold(&mut requests).await;
    assert_eq!(roster::active_sessions(&host.mgr), 1);

    let gate = MgrGate::new(&host.mgr);
    let mut waiting = tokio::spawn(async move { gate.wait_idle().await });
    assert!(
        timeout(Duration::from_secs(3), &mut waiting).await.is_err(),
        "update gate completed while approval was pending"
    );

    reply.send(ConnectionApprovalOutcome::Rejected).unwrap();
    auth(&mut client, pb::auth_result::Status::Denied).await;
    timeout(Duration::from_secs(3), waiting)
        .await
        .expect("update gate did not unblock after approval ended")
        .expect("update gate task failed");
}
