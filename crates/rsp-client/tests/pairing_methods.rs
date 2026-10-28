#[allow(dead_code)]
mod support;

use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};

use prost::Message;
use rsp_client::session::{
    EndReason, EventStream, PairingCredential, Session, SessionError, SessionEvent, SessionParams,
};
use rsp_core::{
    errors::ErrorCode,
    features,
    ids::id_from_fp,
    pairing::{HostPairing, PairingError},
    pb::{self, pair_result::Status},
};
use rsp_transport::{
    identity::{CertKind, LocalIdentity},
    Conn, ControlStream,
};
use tokio::time::timeout;
use zeroize::{Zeroize, Zeroizing};

const FILE_ID: [u8; 16] = [0xfb; 16];
const SECRET: [u8; 32] = *b"file-secret-must-stay-redacted!!";
const ACCOUNT: &str = "test-pair-account-private";
const PASSWORD: &[u8] = b"test-pair-password-private";
const DEVICE_NAME: &str = "pairing test phone";

fn params(host: &support::Host) -> SessionParams {
    let mut params = SessionParams::single(
        host.addr,
        host.fingerprint,
        LocalIdentity::generate_software(CertKind::App).expect("app identity"),
    );
    params.intent = pb::Intent::Pair;
    params.device_name = DEVICE_NAME.into();
    params.platform = pb::Platform::Ios;
    params
}

async fn hello(control: &ControlStream, features: u64, required: u64) {
    let pb::envelope::Msg::ClientHello(client) = support::recv(control).await else {
        panic!("expected ClientHello");
    };
    assert_eq!(client.intent, pb::Intent::Pair as i32);
    assert_eq!(client.required_features, required);
    support::send(
        control,
        pb::ServerHello {
            protocol: features::PROTOCOL_VERSION,
            features,
            ..Default::default()
        },
    )
    .await;
}

async fn outcome(events: &mut EventStream) -> (Vec<pb::PairResult>, EndReason) {
    let mut results = Vec::new();
    loop {
        match timeout(support::TEST_TIMEOUT, events.recv())
            .await
            .expect("event deadline")
            .expect("session event")
        {
            SessionEvent::PairResult(result) => results.push(result),
            SessionEvent::Ended { code, kind, .. } => {
                match kind {
                    EndReason::Goodbye(_) => assert_eq!(code, None),
                    EndReason::Error(expected) => assert_eq!(code, Some(expected)),
                    _ => {}
                }
                return (results, kind);
            }
            _ => {}
        }
    }
}

async fn closed(conn: &Conn, code: u64) {
    assert_eq!(
        timeout(support::TEST_TIMEOUT, conn.closed())
            .await
            .expect("close deadline")
            .code,
        Some(code)
    );
}

fn result(status: Status, fp: &[u8; 32]) -> pb::PairResult {
    pb::PairResult {
        status: status as i32,
        host_id: id_from_fp(fp),
        host_name: "scripted host".into(),
        ..Default::default()
    }
}

async fn file_exchange(wrong_secret: bool) -> (Vec<pb::PairResult>, EndReason) {
    let host = support::Host::bind().await;
    // Invalid legacy code also proves that the file credential takes precedence.
    let params = params(&host)
        .with_pairing_file(FILE_ID, Zeroizing::new(SECRET))
        .with_pairing_code("ignored legacy code");
    let device_fp = params.local_identity.fingerprint();
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        hello(&control, features::PAIR_FILE, features::PAIR_FILE).await;
        let pb::envelope::Msg::PairStart(start) = support::recv(&control).await else {
            panic!("expected PairStart");
        };
        assert_eq!(start.pair_file_id, FILE_ID);
        assert!(!start.has_qr);
        assert_eq!(start.device_name, DEVICE_NAME);
        assert_eq!(start.platform, pb::Platform::Ios as i32);
        let mut secret = Zeroizing::new(SECRET);
        if wrong_secret {
            secret[31] ^= 1;
        }
        let mut pairing = HostPairing::new_file(&FILE_ID, &secret, host.fingerprint, device_fp);
        support::send(&control, pairing.try_on_start(start).expect("Host start")).await;
        let pb::envelope::Msg::PairSpake(spake) = support::recv(&control).await else {
            panic!("expected PairSpake");
        };
        pairing.on_spake(spake).expect("Host SPAKE");
        let pb::envelope::Msg::PairConfirm(confirm) = support::recv(&control).await else {
            panic!("expected PairConfirm");
        };
        if wrong_secret {
            assert_eq!(pairing.on_confirm(confirm), Err(PairingError::BadCode));
            // A peer ignoring its failed confirmation still cannot convince the App.
            support::send(&control, pb::PairConfirm { mac: vec![0; 32] }).await;
            closed(&conn, 1).await;
        } else {
            support::send(&control, pairing.on_confirm(confirm).expect("Host confirm")).await;
            assert!(pairing.is_verified());
            support::send(&control, result(Status::Ok, &host.fingerprint)).await;
            closed(&conn, 0).await;
        }
    });
    let (_handle, mut events) = Session::open(params).await.expect("open");
    let outcome = outcome(&mut events).await;
    server.await.expect("Host script");
    outcome
}

#[tokio::test]
async fn file_pairing_completes_spake_and_key_confirmation() {
    let (results, end) = file_exchange(false).await;
    assert_eq!(end, EndReason::Goodbye(pb::goodbye::Reason::UserClosed));
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, Status::Ok as i32);
}

#[tokio::test]
async fn file_pairing_with_wrong_secret_fails_client_confirmation() {
    let (results, end) = file_exchange(true).await;
    assert_eq!(end, EndReason::Protocol);
    assert!(results.is_empty());
}

async fn unsupported(feature: u64) {
    let host = support::Host::bind().await;
    let mut params = if feature == features::PAIR_FILE {
        params(&host).with_pairing_file(FILE_ID, Zeroizing::new(SECRET))
    } else {
        params(&host).with_pairing_password(ACCOUNT, Zeroizing::new(PASSWORD.to_vec()))
    };
    // The new bit must be ORed into existing requirements.
    params.required_features = features::FEC_RS;
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        hello(&control, features::FEC_RS, feature | features::FEC_RS).await;
        // Reading until closure detects an attempted credential send, without
        // relying on a short timeout to assert the absence of a message.
        if let Ok(Some(payload)) = timeout(support::TEST_TIMEOUT, control.recv())
            .await
            .expect("read deadline")
        {
            panic!(
                "credential sent to unsupported Host: {:?}",
                pb::Envelope::decode(payload).expect("protobuf")
            );
        }
        closed(&conn, 2).await;
    });
    let (_handle, mut events) = Session::open(params).await.expect("open");
    let (results, end) = outcome(&mut events).await;
    assert!(results.is_empty());
    assert_eq!(end, EndReason::Error(ErrorCode::HostTooOld));
    server.await.expect("Host script");
}

#[tokio::test]
async fn file_pairing_requires_host_feature_before_pair_start() {
    unsupported(features::PAIR_FILE).await;
}

#[tokio::test]
async fn password_pairing_requires_host_feature_before_credentials() {
    unsupported(features::PAIR_PASSWORD).await;
}

async fn password_exchange(
    reply: impl FnOnce(&[u8; 32]) -> pb::Envelope + Send + 'static,
    expected: EndReason,
) -> Vec<pb::PairResult> {
    let host = support::Host::bind().await;
    let params = params(&host)
        .with_pairing_password(ACCOUNT, Zeroizing::new(PASSWORD.to_vec()))
        .with_pairing_code("ignored legacy code");
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        hello(&control, features::PAIR_PASSWORD, features::PAIR_PASSWORD).await;
        let pb::envelope::Msg::PairPassword(mut request) = support::recv(&control).await else {
            panic!("expected PairPassword");
        };
        assert_eq!(request.account, ACCOUNT);
        assert_eq!(request.password, PASSWORD);
        assert_eq!(request.device_name, DEVICE_NAME);
        assert_eq!(request.platform, pb::Platform::Ios as i32);
        request.account.zeroize();
        request.password.zeroize();
        support::send(&control, reply(&host.fingerprint)).await;
        // No second request or SPAKE traffic is permitted after the terminal reply.
        assert!(!matches!(
            timeout(support::TEST_TIMEOUT, control.recv())
                .await
                .expect("read deadline"),
            Ok(Some(_))
        ));
        let close_code = match expected {
            EndReason::Goodbye(_) => 0,
            EndReason::Protocol => 1,
            EndReason::Error(_) => 2,
            _ => panic!("unexpected test ending"),
        };
        closed(&conn, close_code).await;
    });
    let (_handle, mut events) = Session::open(params).await.expect("open");
    let (results, end) = outcome(&mut events).await;
    assert_eq!(end, expected);
    server.await.expect("Host script");
    results
}

#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);

impl Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("log buffer").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn password_pairing_succeeds_without_logging_credentials() {
    let logs = LogCapture::default();
    let writer = logs.clone();
    // A global subscriber also covers the session and transport tasks spawned
    // by Session::open, rather than only the test's own future.
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("test log subscriber");
    tracing::debug!("pairing log capture active");
    let results = password_exchange(
        |fp| result(Status::Ok, fp).into(),
        EndReason::Goodbye(pb::goodbye::Reason::UserClosed),
    )
    .await;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, Status::Ok as i32);
    // Exercise the protocol error log as well as the successful request.
    let rejected = password_exchange(
        |_| pb::PairConfirm { mac: vec![0; 32] }.into(),
        EndReason::Protocol,
    )
    .await;
    assert!(rejected.is_empty());
    let bytes = logs.0.lock().expect("logs").clone();
    let text = String::from_utf8(bytes).expect("UTF-8 logs");
    assert!(text.contains("pairing log capture active"));
    assert!(text.contains("unexpected PairConfirm"));
    assert!(!text.contains(ACCOUNT));
    assert!(!text.contains(std::str::from_utf8(PASSWORD).expect("UTF-8 password")));
}

#[tokio::test]
async fn password_pairing_rejects_ok_with_wrong_host_id() {
    let results =
        password_exchange(|_| result(Status::Ok, &[0; 32]).into(), EndReason::Protocol).await;
    assert!(results.is_empty());
}

#[tokio::test]
async fn password_pairing_rejects_waiting_local() {
    let results = password_exchange(
        |fp| result(Status::WaitingLocal, fp).into(),
        EndReason::Protocol,
    )
    .await;
    assert!(results.is_empty());
}

#[tokio::test]
async fn password_pairing_rejects_spake() {
    let results = password_exchange(
        |_| pb::PairSpake { msg: vec![0; 33] }.into(),
        EndReason::Protocol,
    )
    .await;
    assert!(results.is_empty());
}

#[tokio::test]
async fn password_pairing_locked_preserves_retry_after() {
    let results = password_exchange(
        |fp| {
            pb::PairResult {
                retry_after_s: 873,
                ..result(Status::Locked, fp)
            }
            .into()
        },
        EndReason::Error(ErrorCode::PolicyDenied),
    )
    .await;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, Status::Locked as i32);
    assert_eq!(results[0].retry_after_s, 873);
}

#[tokio::test]
async fn password_pairing_method_disabled_is_a_policy_result() {
    let results = password_exchange(
        |fp| result(Status::MethodDisabled, fp).into(),
        EndReason::Error(ErrorCode::PolicyDenied),
    )
    .await;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, 8);
}

#[tokio::test]
async fn password_pairing_bad_code_preserves_attempts_left() {
    let results = password_exchange(
        |fp| {
            pb::PairResult {
                attempts_left: 3,
                ..result(Status::BadCode, fp)
            }
            .into()
        },
        EndReason::Error(ErrorCode::PolicyDenied),
    )
    .await;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, Status::BadCode as i32);
    assert_eq!(results[0].attempts_left, 3);
}

#[tokio::test]
async fn empty_password_is_rejected_without_account_in_error() {
    let host = support::Host::bind().await;
    let params = params(&host).with_pairing_password(ACCOUNT, Zeroizing::new(Vec::new()));
    let Err(error) = Session::open(params).await else {
        panic!("empty password accepted");
    };
    assert!(matches!(error, SessionError::Protocol(_)));
    assert_eq!(
        error.to_string(),
        "protocol error: invalid pairing credential"
    );
    assert!(!format!("{error:?}").contains(ACCOUNT));
}

#[tokio::test]
async fn credentials_and_params_debug_redact_secrets() {
    let password = PairingCredential::Password {
        account: ACCOUNT.into(),
        password: Zeroizing::new(PASSWORD.to_vec()),
    };
    assert_eq!(format!("{password:?}"), "Password(<redacted>)");
    let file = PairingCredential::File {
        file_id: FILE_ID,
        secret: Zeroizing::new(SECRET),
    };
    assert_eq!(format!("{file:?}"), "File(-_v7-_v7-_v7-_v7-_v7-w)");
    let code = PairingCredential::Code("123456".into());
    assert_eq!(format!("{code:?}"), "Code(<redacted>)");
    let host = support::Host::bind().await;
    for credential in [password, file, code] {
        let mut params = params(&host).with_pairing_code("987654");
        params.pairing = Some(credential);
        let text = format!("{params:?}");
        for secret in [
            ACCOUNT,
            std::str::from_utf8(PASSWORD).unwrap(),
            std::str::from_utf8(&SECRET).unwrap(),
            "123456",
            "987654",
        ] {
            assert!(!text.contains(secret));
        }
    }
}

#[tokio::test]
async fn explicit_code_credential_overrides_legacy_pairing_code() {
    let host = support::Host::bind().await;
    let mut params = params(&host).with_pairing_code("invalid legacy code");
    params.pairing = Some(PairingCredential::Code("123456".into()));
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        hello(&control, 0, 0).await;
        let pb::envelope::Msg::PairStart(start) = support::recv(&control).await else {
            panic!("expected PairStart");
        };
        assert!(start.pair_file_id.is_empty());
        assert!(!start.has_qr);
        support::send(&control, result(Status::Denied, &host.fingerprint)).await;
        closed(&conn, 2).await;
    });
    let (_handle, mut events) = Session::open(params).await.expect("open");
    let (results, end) = outcome(&mut events).await;
    assert_eq!(end, EndReason::Error(ErrorCode::PolicyDenied));
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, Status::Denied as i32);
    server.await.expect("Host script");
}
