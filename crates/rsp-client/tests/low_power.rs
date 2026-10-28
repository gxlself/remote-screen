#[allow(dead_code)]
mod support;

use std::time::Duration;

use rsp_client::session::{Session, SessionEvent, SessionParams};
use rsp_core::{features, pb};
use rsp_transport::identity::{CertKind, LocalIdentity};
use tokio::time::timeout;

fn params(host: &support::Host) -> SessionParams {
    SessionParams::single(
        host.addr,
        host.fingerprint,
        LocalIdentity::generate_software(CertKind::App).expect("app identity"),
    )
}

#[tokio::test]
async fn low_power_updates_apply_to_the_next_feedback_and_default_is_false() {
    let host = support::Host::bind().await;
    let params = params(&host);
    let (set_tx, set_rx) = tokio::sync::oneshot::channel();
    let (reset_tx, reset_rx) = tokio::sync::oneshot::channel();
    let (feedback_tx, feedback_rx) = tokio::sync::oneshot::channel();

    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        assert!(matches!(
            support::recv(&control).await,
            pb::envelope::Msg::ClientHello(_)
        ));
        support::send(
            &control,
            pb::ServerHello {
                protocol: 1,
                features: features::RATE_FEEDBACK,
                ..Default::default()
            },
        )
        .await;
        support::auth(&control, pb::auth_result::Status::Ok).await;
        assert!(matches!(
            support::recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));

        let mut feedback = Vec::new();
        let mut set_tx = Some(set_tx);
        let mut reset_tx = Some(reset_tx);
        while feedback.len() < 3 {
            match timeout(Duration::from_secs(5), support::recv(&control))
                .await
                .expect("Feedback deadline")
            {
                pb::envelope::Msg::Ping(ping) => {
                    support::send(
                        &control,
                        pb::Pong {
                            t1_us: ping.t1_us,
                            t2_us: rsp_core::clock::unix_micros(),
                        },
                    )
                    .await;
                }
                pb::envelope::Msg::Feedback(value) => {
                    feedback.push(value.low_power);
                    match feedback.len() {
                        1 => set_tx
                            .take()
                            .expect("set sender")
                            .send(())
                            .expect("client updates low power"),
                        2 => reset_tx
                            .take()
                            .expect("reset sender")
                            .send(())
                            .expect("client clears low power"),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        feedback_tx.send(feedback).expect("feedback result");
        let _ = conn.closed().await;
    });

    let (handle, mut events) = Session::open(params).await.expect("open");
    while !matches!(
        events.recv().await,
        Some(SessionEvent::StateChanged(
            rsp_client::SessionStateKind::Streaming
        ))
    ) {}
    set_rx.await.expect("first Feedback");
    handle.set_low_power(true);
    reset_rx.await.expect("second Feedback");
    handle.set_low_power(false);
    let feedback = feedback_rx.await.expect("feedback sequence");
    assert_eq!(feedback, [false, true, false]);
    handle.close();
    assert!(conn_end(&mut events).await, "session ends");
    server.await.expect("scripted Host");
}

async fn conn_end(events: &mut rsp_client::EventStream) -> bool {
    timeout(Duration::from_secs(5), async {
        while let Some(event) = events.recv().await {
            if matches!(event, SessionEvent::Ended { .. }) {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false)
}
