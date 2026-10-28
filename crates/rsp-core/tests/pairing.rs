use hkdf::Hkdf;
use hmac::{Hmac, KeyInit, Mac};
use rsp_core::{
    ids::id_from_fp,
    pairing::{AppPairing, HostPairing, Outcome, PairingError, MAX_CODE_FAILURES},
    pb::{self, pair_result::Status},
};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};

const DEVICE_FP: [u8; 32] = [0x11; 32];
const HOST_FP: [u8; 32] = [0x22; 32];

fn app() -> AppPairing {
    AppPairing::new(
        "012345",
        DEVICE_FP,
        HOST_FP,
        "小明的手机",
        pb::Platform::Ios,
    )
    .unwrap()
    .with_qr(true)
}

fn host() -> HostPairing {
    HostPairing::new("012345", HOST_FP, DEVICE_FP).unwrap()
}

fn exchange(app: &mut AppPairing, host: &mut HostPairing) -> pb::PairConfirm {
    let start = app.start();
    assert_eq!(start.device_name, "小明的手机");
    assert_eq!(start.platform, pb::Platform::Ios as i32);
    assert!(start.has_qr);
    let msg_host = host.on_start(start);
    assert_eq!(msg_host.msg[0], b'B');
    let (msg_app, confirm) = app.on_spake(msg_host).unwrap();
    assert_eq!(msg_app.msg[0], b'A');
    host.on_spake(msg_app).unwrap();
    confirm
}

fn confirmed_pair() -> (AppPairing, HostPairing) {
    let (mut app, mut host) = (app(), host());
    let confirm = exchange(&mut app, &mut host);
    let confirm = host.on_confirm(confirm).unwrap();
    let verified = app.on_confirm(confirm).unwrap();
    assert_eq!(verified.device_fp(), &DEVICE_FP);
    assert_eq!(verified.host_fp(), &HOST_FP);
    assert!(app.is_verified());
    assert!(host.is_verified());
    (app, host)
}

fn result(status: Status) -> pb::PairResult {
    pb::PairResult {
        status: status as i32,
        host_id: id_from_fp(&HOST_FP),
        host_name: "小明的电脑".into(),
        attempts_left: 4,
        ..Default::default()
    }
}

#[test]
fn correct_code_confirms_both_sides_then_waits_for_local_approval() {
    assert_eq!(MAX_CODE_FAILURES, 5);
    let (mut app, _) = confirmed_pair();
    assert_eq!(
        app.on_result(result(Status::WaitingLocal)),
        Outcome::WaitingLocal
    );
    assert_eq!(
        app.on_result(result(Status::Ok)),
        Outcome::Paired {
            host_id: id_from_fp(&HOST_FP),
            host_name: "小明的电脑".into(),
        }
    );
    assert!(app.is_verified());
    assert_eq!(
        app.on_result(result(Status::Ok)),
        Outcome::Failed(PairingError::Protocol)
    );
}

#[test]
fn ok_can_follow_confirmation_without_waiting_local() {
    let (mut app, _) = confirmed_pair();
    assert!(matches!(
        app.on_result(result(Status::Ok)),
        Outcome::Paired { .. }
    ));
}

#[test]
fn wrong_code_fails_host_and_app_cannot_become_verified() {
    let mut app = app();
    let mut host = HostPairing::new("654321", HOST_FP, DEVICE_FP).unwrap();
    let confirm = exchange(&mut app, &mut host);
    assert_eq!(host.on_confirm(confirm.clone()), Err(PairingError::BadCode));
    assert!(!host.is_verified());
    assert!(!app.is_verified());
    assert_eq!(host.on_confirm(confirm), Err(PairingError::Protocol));
    assert_eq!(
        app.on_result(result(Status::BadCode)),
        Outcome::BadCode { attempts_left: 4 }
    );
    assert_eq!(
        app.on_confirm(pb::PairConfirm { mac: vec![0; 32] }),
        Err(PairingError::Protocol)
    );
    assert!(!app.is_verified());
}

#[test]
fn replaced_tls_fingerprints_fail_host_confirmation() {
    for (host_fp, device_fp) in [([0x33; 32], DEVICE_FP), (HOST_FP, [0x44; 32])] {
        let mut app = app();
        let mut host = HostPairing::new("012345", host_fp, device_fp).unwrap();
        let confirm = exchange(&mut app, &mut host);
        assert_eq!(host.on_confirm(confirm), Err(PairingError::BadCode));
        assert!(!host.is_verified());
        assert!(!app.is_verified());
        assert_eq!(
            app.on_confirm(pb::PairConfirm { mac: vec![0; 32] }),
            Err(PairingError::ManInTheMiddle)
        );
        assert!(!app.is_verified());
    }
}

// An independent peer also verifies the precise identity labels, raw FP bytes,
// transcript order, empty HKDF salt and role-specific MACs used by the state machine.
fn independent_host(host_fp: [u8; 32], device_fp: [u8; 32]) -> Result<(), PairingError> {
    let mut app = app();
    app.start();
    let id_app = [b"rsp-app:".as_slice(), &device_fp].concat();
    let id_host = [b"rsp-host:".as_slice(), &host_fp].concat();
    let (spake, msg_host) = Spake2::<Ed25519Group>::start_b(
        &Password::new(b"012345"),
        &Identity::new(&id_app),
        &Identity::new(&id_host),
    );
    let (msg_app, confirm_app) = app
        .on_spake(pb::PairSpake {
            msg: msg_host.clone(),
        })
        .unwrap();
    let key = spake.finish(&msg_app.msg).unwrap();
    let mut key_confirm = [0; 32];
    Hkdf::<Sha256>::new(Some(b""), &key)
        .expand(b"rsp-pair-confirm", &mut key_confirm)
        .unwrap();
    let transcript = [msg_app.msg.as_slice(), &msg_host, &device_fp, &host_fp].concat();
    let mut mac_app = Hmac::<Sha256>::new_from_slice(&key_confirm).unwrap();
    mac_app.update(b"A");
    mac_app.update(&transcript);
    if host_fp == HOST_FP && device_fp == DEVICE_FP {
        mac_app.verify_slice(&confirm_app.mac).unwrap();
    } else {
        assert!(mac_app.verify_slice(&confirm_app.mac).is_err());
    }
    // Even if an attacker sends Host confirmation after rejecting App confirmation,
    // the App must reject it when either TLS identity was changed.
    let mut mac_host = Hmac::<Sha256>::new_from_slice(&key_confirm).unwrap();
    mac_host.update(b"B");
    mac_host.update(&transcript);
    app.on_confirm(pb::PairConfirm {
        mac: mac_host.finalize().into_bytes().to_vec(),
    })
    .map(|_| ())
}

#[test]
fn app_verifies_independent_peer_and_rejects_swapped_certificates() {
    assert_eq!(independent_host(HOST_FP, DEVICE_FP), Ok(()));
    assert_eq!(
        independent_host([0x33; 32], DEVICE_FP),
        Err(PairingError::ManInTheMiddle)
    );
    assert_eq!(
        independent_host(HOST_FP, [0x44; 32]),
        Err(PairingError::ManInTheMiddle)
    );
}

#[test]
fn rejects_early_confirmations_and_spake_messages() {
    let mut app = app();
    assert_eq!(
        app.on_confirm(pb::PairConfirm::default()),
        Err(PairingError::Protocol)
    );
    assert_eq!(app.try_start(), Err(PairingError::Protocol));
    let mut app = self::app();
    assert_eq!(
        app.on_spake(pb::PairSpake::default()),
        Err(PairingError::Protocol)
    );
    let mut host = host();
    assert_eq!(
        host.on_confirm(pb::PairConfirm::default()),
        Err(PairingError::Protocol)
    );
    assert_eq!(
        host.try_on_start(pb::PairStart::default()),
        Err(PairingError::Protocol)
    );
    let mut host = self::host();
    assert_eq!(
        host.on_spake(pb::PairSpake::default()),
        Err(PairingError::Protocol)
    );
    let (mut app, mut host) = (self::app(), self::host());
    host.on_start(app.start());
    assert_eq!(
        app.on_confirm(pb::PairConfirm::default()),
        Err(PairingError::Protocol)
    );
    assert_eq!(
        host.on_confirm(pb::PairConfirm::default()),
        Err(PairingError::Protocol)
    );
}

#[test]
fn rejects_duplicate_starts_with_checked_and_infallible_apis() {
    let mut app = app();
    app.try_start().unwrap();
    assert_eq!(app.try_start(), Err(PairingError::Protocol));
    assert_eq!(
        app.on_spake(pb::PairSpake::default()),
        Err(PairingError::Protocol)
    );
    let mut host = host();
    host.try_on_start(pb::PairStart::default()).unwrap();
    assert_eq!(
        host.try_on_start(pb::PairStart::default()),
        Err(PairingError::Protocol)
    );
    let (mut app, mut host) = (self::app(), self::host());
    let start = app.start();
    host.on_start(start.clone());
    assert!(host.on_start(start).msg.is_empty());
    assert_eq!(
        host.on_spake(pb::PairSpake::default()),
        Err(PairingError::Protocol)
    );
    assert_eq!(app.start(), pb::PairStart::default());
    assert_eq!(
        app.on_spake(pb::PairSpake::default()),
        Err(PairingError::Protocol)
    );
}

#[test]
fn rejects_duplicate_spake_and_confirmation_messages() {
    let (mut app, mut host) = (app(), host());
    let msg_host = host.on_start(app.start());
    let (msg_app, _) = app.on_spake(msg_host.clone()).unwrap();
    assert_eq!(app.on_spake(msg_host), Err(PairingError::Protocol));
    host.on_spake(msg_app.clone()).unwrap();
    assert_eq!(host.on_spake(msg_app), Err(PairingError::Protocol));
    let (mut app, mut host) = (self::app(), self::host());
    let confirm_app = exchange(&mut app, &mut host);
    let confirm_host = host.on_confirm(confirm_app.clone()).unwrap();
    app.on_confirm(confirm_host.clone()).unwrap();
    assert_eq!(host.on_confirm(confirm_app), Err(PairingError::Protocol));
    assert_eq!(app.on_confirm(confirm_host), Err(PairingError::Protocol));
    assert!(!app.is_verified());
    assert!(!host.is_verified());
}

#[test]
fn rejects_all_truncated_spake_messages_and_wrong_role() {
    for len in 0..33 {
        let (mut app, mut host) = (app(), host());
        let mut msg_host = host.on_start(app.start());
        msg_host.msg.truncate(len);
        assert_eq!(app.on_spake(msg_host), Err(PairingError::Protocol));
        assert_eq!(
            app.on_confirm(pb::PairConfirm::default()),
            Err(PairingError::Protocol)
        );
        let (mut app, mut host) = (self::app(), self::host());
        let msg_host = host.on_start(app.start());
        let (mut msg_app, _) = app.on_spake(msg_host).unwrap();
        msg_app.msg.truncate(len);
        assert_eq!(host.on_spake(msg_app), Err(PairingError::Protocol));
        assert_eq!(
            host.on_confirm(pb::PairConfirm::default()),
            Err(PairingError::Protocol)
        );
    }
    let (mut app, mut host) = (app(), host());
    let msg_host = host.on_start(app.start());
    assert_eq!(host.on_spake(msg_host), Err(PairingError::Protocol));
    let (mut app, mut host) = (self::app(), self::host());
    let mut msg_host = host.on_start(app.start());
    msg_host.msg[0] = b'A';
    assert_eq!(app.on_spake(msg_host), Err(PairingError::Protocol));
}

#[test]
fn rejects_reflected_and_tampered_confirmations() {
    for tamper in [false, true] {
        let (mut app, mut host) = (app(), host());
        let confirm_app = exchange(&mut app, &mut host);
        let mut confirm_host = host.on_confirm(confirm_app.clone()).unwrap();
        let invalid = if tamper {
            confirm_host.mac[0] ^= 1;
            confirm_host
        } else {
            confirm_app
        };
        assert_eq!(app.on_confirm(invalid), Err(PairingError::ManInTheMiddle));
        assert_eq!(
            app.on_result(result(Status::Ok)),
            Outcome::Failed(PairingError::Protocol)
        );
    }
    for len in [0, 1, 31, 33] {
        let (mut app, mut host) = (app(), host());
        exchange(&mut app, &mut host);
        assert_eq!(
            host.on_confirm(pb::PairConfirm { mac: vec![0; len] }),
            Err(PairingError::BadCode)
        );
        assert_eq!(
            app.on_confirm(pb::PairConfirm { mac: vec![0; len] }),
            Err(PairingError::ManInTheMiddle)
        );
    }
}

#[test]
fn results_cannot_bypass_confirmation_or_rebind_host_id() {
    for status in [Status::Ok, Status::WaitingLocal] {
        let mut app = app();
        assert_eq!(
            app.on_result(result(status)),
            Outcome::Failed(PairingError::Protocol)
        );
        let mut app = self::app();
        app.start();
        assert_eq!(
            app.on_result(result(status)),
            Outcome::Failed(PairingError::Protocol)
        );
        let (mut app, mut host) = (self::app(), host());
        exchange(&mut app, &mut host);
        assert_eq!(
            app.on_result(result(status)),
            Outcome::Failed(PairingError::Protocol)
        );
    }
    let (mut app, _) = confirmed_pair();
    let mut success = result(Status::Ok);
    success.host_id = id_from_fp(&[0x33; 32]);
    assert_eq!(
        app.on_result(success),
        Outcome::Failed(PairingError::Protocol)
    );
    let (mut app, _) = confirmed_pair();
    app.on_result(result(Status::WaitingLocal));
    assert_eq!(
        app.on_result(result(Status::WaitingLocal)),
        Outcome::Failed(PairingError::Protocol)
    );
}

#[test]
fn surfaces_all_terminal_results_and_rejects_unknown_statuses() {
    for (status, outcome) in [
        (Status::Denied, Outcome::Denied),
        (Status::BadCode, Outcome::BadCode { attempts_left: 4 }),
        (Status::Expired, Outcome::Expired),
        (Status::Locked, Outcome::Locked),
        (Status::DisabledOnPath, Outcome::DisabledOnPath),
    ] {
        let mut app = app();
        app.start();
        assert_eq!(app.on_result(result(status)), outcome);
        assert_eq!(
            app.on_result(result(status)),
            Outcome::Failed(PairingError::Protocol)
        );
    }
    for status in [0, -1, 999] {
        let (mut app, _) = confirmed_pair();
        let mut message = result(Status::Ok);
        message.status = status;
        assert_eq!(
            app.on_result(message),
            Outcome::Failed(PairingError::Protocol)
        );
    }
}

#[test]
fn validates_six_ascii_digits_without_panicking() {
    for code in ["", "12345", "1234567", "１２３４５６", "12 345", "12345x"] {
        assert!(matches!(
            AppPairing::new(code, DEVICE_FP, HOST_FP, "phone", pb::Platform::Ios),
            Err(PairingError::InvalidCode)
        ));
        assert!(matches!(
            HostPairing::new(code, HOST_FP, DEVICE_FP),
            Err(PairingError::InvalidCode)
        ));
    }
    let mut manual =
        AppPairing::new("000000", DEVICE_FP, HOST_FP, "phone", pb::Platform::Ios).unwrap();
    assert!(!manual.start().has_qr);
}
