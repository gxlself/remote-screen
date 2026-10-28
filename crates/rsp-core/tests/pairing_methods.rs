use hkdf::Hkdf;
use hmac::{Hmac, KeyInit, Mac};
use rsp_core::{
    ids::id_from_fp,
    pairfile::spake_password,
    pairing::{
        AppPairing, AppPasswordPairing, HostPairing, Outcome, PairingError,
        PASSWORD_ACCOUNT_MAX_BYTES, PASSWORD_MAX_BYTES,
    },
    pb::{self, pair_result::Status},
};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use zeroize::{Zeroize, Zeroizing};

const FILE_ID: [u8; 16] = [0x11; 16];
const SECRET: [u8; 32] = [0x22; 32];
const HOST_FP: [u8; 32] = [0x33; 32];
const DEVICE_FP: [u8; 32] = [0x44; 32];
const ACCOUNT: &str = "pair-user";
const TEST_PASSWORD: &[u8] = b"s3cret-pw";

fn file_app() -> AppPairing {
    AppPairing::new_file(
        FILE_ID,
        &SECRET,
        DEVICE_FP,
        HOST_FP,
        "小明的手机",
        pb::Platform::Ios,
    )
}

fn file_host() -> HostPairing {
    HostPairing::new_file(&FILE_ID, &SECRET, HOST_FP, DEVICE_FP)
}

fn result(status: Status) -> pb::PairResult {
    pb::PairResult {
        status: status as i32,
        host_id: id_from_fp(&HOST_FP),
        host_name: "小明的 Mac mini".into(),
        attempts_left: 3,
        retry_after_s: if status == Status::Locked { 900 } else { 0 },
    }
}

fn exchange(app: &mut AppPairing, host: &mut HostPairing) -> pb::PairConfirm {
    let msg_host = host.on_start(app.try_start().unwrap());
    let (msg_app, confirm_app) = app.on_spake(msg_host).unwrap();
    host.on_spake(msg_app).unwrap();
    confirm_app
}

#[test]
fn file_pairing_confirms_both_sides_and_pairs_without_local_approval() {
    let (mut app, mut host) = (file_app(), file_host());
    let confirm_app = exchange(&mut app, &mut host);
    let confirm_host = host.on_confirm(confirm_app).unwrap();
    let verified = app.on_confirm(confirm_host).unwrap();
    assert_eq!(verified.device_fp(), &DEVICE_FP);
    assert_eq!(verified.host_fp(), &HOST_FP);
    assert!(app.is_verified());
    assert!(host.is_verified());
    assert_eq!(
        app.on_result(result(Status::Ok)),
        Outcome::Paired {
            host_id: id_from_fp(&HOST_FP),
            host_name: "小明的 Mac mini".into(),
        }
    );
    assert_eq!(
        app.on_result(result(Status::Ok)),
        Outcome::Failed(PairingError::Protocol)
    );
}

#[test]
fn file_pair_start_identifies_the_file_and_code_start_has_no_file_id() {
    let start = file_app().try_start().unwrap();
    assert_eq!(start.pair_file_id.len(), 16);
    assert_eq!(start.pair_file_id, FILE_ID);
    assert!(!start.has_qr);
    assert_eq!(start.device_name, "小明的手机");
    assert_eq!(start.platform, pb::Platform::Ios as i32);
    let mut code_app =
        AppPairing::new("012345", DEVICE_FP, HOST_FP, "phone", pb::Platform::Ios).unwrap();
    assert!(code_app.try_start().unwrap().pair_file_id.is_empty());
}

fn assert_mismatched_password_rejected(mut app: AppPairing, mut host: HostPairing) {
    let confirm_app = exchange(&mut app, &mut host);
    assert_eq!(host.on_confirm(confirm_app), Err(PairingError::BadCode));
    assert!(!host.is_verified());
    assert!(!app.is_verified());
    assert_eq!(
        app.on_confirm(pb::PairConfirm { mac: vec![0; 32] }),
        Err(PairingError::ManInTheMiddle)
    );
    assert!(!app.is_verified());
    assert_eq!(
        app.on_result(result(Status::Ok)),
        Outcome::Failed(PairingError::Protocol)
    );
}

#[test]
fn one_bit_secret_change_is_rejected() {
    let mut changed = SECRET;
    changed[31] ^= 1;
    assert_mismatched_password_rejected(
        file_app(),
        HostPairing::new_file(&FILE_ID, &changed, HOST_FP, DEVICE_FP),
    );
}

#[test]
fn different_file_ids_are_rejected() {
    let mut changed = FILE_ID;
    changed[15] ^= 1;
    assert_mismatched_password_rejected(
        file_app(),
        HostPairing::new_file(&changed, &SECRET, HOST_FP, DEVICE_FP),
    );
}

#[test]
fn file_and_six_digit_passwords_cannot_be_mixed() {
    assert_mismatched_password_rejected(
        file_app(),
        HostPairing::new("012345", HOST_FP, DEVICE_FP).unwrap(),
    );
    let app = AppPairing::new("012345", DEVICE_FP, HOST_FP, "phone", pb::Platform::Ios).unwrap();
    assert_mismatched_password_rejected(app, file_host());
}

// Derive the Host MAC independently to check the precise identity labels, transcript,
// HKDF context, and role binding, including a peer that ignores App confirmation failure.
fn independent_host_confirm(app: &mut AppPairing, host_fp: [u8; 32]) -> pb::PairConfirm {
    app.try_start().unwrap();
    let password = spake_password(&FILE_ID, &SECRET);
    let identity_app = [b"rsp-app:".as_slice(), &DEVICE_FP].concat();
    let identity_host = [b"rsp-host:".as_slice(), &host_fp].concat();
    let (spake, msg_host) = Spake2::<Ed25519Group>::start_b(
        &Password::new(password.as_ref()),
        &Identity::new(&identity_app),
        &Identity::new(&identity_host),
    );
    let (msg_app, confirm_app) = app
        .on_spake(pb::PairSpake {
            msg: msg_host.clone(),
        })
        .unwrap();
    let key = spake.finish(&msg_app.msg).unwrap();
    let mut confirm_key = [0; 32];
    Hkdf::<Sha256>::new(Some(b""), &key)
        .expand(b"rsp-pair-confirm", &mut confirm_key)
        .unwrap();
    let transcript = [msg_app.msg.as_slice(), &msg_host, &DEVICE_FP, &host_fp].concat();
    let mut mac_app = Hmac::<Sha256>::new_from_slice(&confirm_key).unwrap();
    mac_app.update(b"A");
    mac_app.update(&transcript);
    assert_eq!(
        mac_app.verify_slice(&confirm_app.mac).is_ok(),
        host_fp == HOST_FP
    );
    let mut mac_host = Hmac::<Sha256>::new_from_slice(&confirm_key).unwrap();
    mac_host.update(b"B");
    mac_host.update(&transcript);
    pb::PairConfirm {
        mac: mac_host.finalize().into_bytes().to_vec(),
    }
}

#[test]
fn file_confirmation_matches_independent_peer_and_detects_replaced_host_fingerprint() {
    let mut app = file_app();
    let confirm = independent_host_confirm(&mut app, HOST_FP);
    assert!(app.on_confirm(confirm).is_ok());
    let mut app = file_app();
    let confirm = independent_host_confirm(&mut app, [0x55; 32]);
    assert_eq!(app.on_confirm(confirm), Err(PairingError::ManInTheMiddle));
    assert!(!app.is_verified());
    assert_eq!(
        app.on_result(result(Status::Ok)),
        Outcome::Failed(PairingError::Protocol)
    );
}

fn password_pairing(account: &str, password: Vec<u8>) -> Result<AppPasswordPairing, PairingError> {
    AppPasswordPairing::new(
        account,
        Zeroizing::new(password),
        HOST_FP,
        "小明的手机",
        pb::Platform::Ios,
    )
}

fn password_app() -> AppPasswordPairing {
    password_pairing(ACCOUNT, TEST_PASSWORD.to_vec()).unwrap()
}

fn erase(mut message: pb::PairPassword) {
    message.account.zeroize();
    message.password.zeroize();
}

#[test]
fn password_credentials_enforce_byte_limits_and_control_character_rules() {
    assert_eq!(PASSWORD_ACCOUNT_MAX_BYTES, 256);
    assert_eq!(PASSWORD_MAX_BYTES, 1024);
    assert_eq!(
        password_pairing("", TEST_PASSWORD.to_vec()).unwrap_err(),
        PairingError::InvalidCredential
    );
    assert_eq!(
        password_pairing(&"a".repeat(257), TEST_PASSWORD.to_vec()).unwrap_err(),
        PairingError::InvalidCredential
    );
    assert_eq!(
        password_pairing("user\nname", TEST_PASSWORD.to_vec()).unwrap_err(),
        PairingError::InvalidCredential
    );
    assert_eq!(
        password_pairing(ACCOUNT, vec![]).unwrap_err(),
        PairingError::InvalidCredential
    );
    assert_eq!(
        password_pairing(ACCOUNT, vec![b'x'; 1025]).unwrap_err(),
        PairingError::InvalidCredential
    );
    for account in ["  ", "user\0name", "user\u{85}name", &"龙".repeat(86)] {
        assert_eq!(
            password_pairing(account, TEST_PASSWORD.to_vec()).unwrap_err(),
            PairingError::InvalidCredential
        );
    }
    for account in ["a".repeat(256), "é".repeat(128)] {
        let mut app = password_pairing(&format!("  {account}  "), vec![b'x'; 1024]).unwrap();
        let message = app.try_start().unwrap();
        assert_eq!(message.account, account);
        assert_eq!(message.password.len(), 1024);
        erase(message);
    }
    let mut app = password_pairing(" a ", vec![b' ']).unwrap();
    let message = app.try_start().unwrap();
    assert_eq!(message.account, "a");
    assert_eq!(message.password, b" ");
    erase(message);
    assert_eq!(
        PairingError::InvalidCredential.to_string(),
        "account or password is empty or too long"
    );
}

#[test]
fn password_start_transfers_credentials_once_and_debug_redacts_them() {
    let mut app = password_pairing(&format!("  {ACCOUNT}  "), TEST_PASSWORD.to_vec()).unwrap();
    let debug_before = format!("{app:?}");
    let message = app.try_start().unwrap();
    assert_eq!(message.account, ACCOUNT);
    assert_eq!(message.password, TEST_PASSWORD);
    assert_eq!(message.device_name, "小明的手机");
    assert_eq!(message.platform, pb::Platform::Ios as i32);
    for debug in [debug_before, format!("{app:?}"), format!("{message:?}")] {
        assert!(!debug.contains(ACCOUNT));
        assert!(!debug.contains("s3cret-pw"));
        assert!(!debug.contains(&format!("{TEST_PASSWORD:?}")));
    }
    erase(message);
    assert_eq!(app.try_start(), Err(PairingError::Protocol));
    assert_eq!(
        app.on_result(result(Status::Ok)),
        Outcome::Failed(PairingError::Protocol)
    );
}

#[test]
fn password_result_before_start_terminates_the_attempt() {
    let mut app = password_app();
    assert_eq!(
        app.on_result(result(Status::Ok)),
        Outcome::Failed(PairingError::Protocol)
    );
    assert_eq!(app.try_start(), Err(PairingError::Protocol));
}

#[test]
fn password_ok_requires_pinned_host_id_and_is_terminal() {
    let mut app = password_app();
    erase(app.try_start().unwrap());
    assert_eq!(
        app.on_result(result(Status::Ok)),
        Outcome::Paired {
            host_id: id_from_fp(&HOST_FP),
            host_name: "小明的 Mac mini".into(),
        }
    );
    assert_eq!(
        app.on_result(result(Status::Ok)),
        Outcome::Failed(PairingError::Protocol)
    );
    assert_eq!(app.try_start(), Err(PairingError::Protocol));
    for host_id in [String::new(), id_from_fp(&[0x55; 32])] {
        let mut app = password_app();
        erase(app.try_start().unwrap());
        let mut message = result(Status::Ok);
        message.host_id = host_id;
        assert_eq!(
            app.on_result(message),
            Outcome::Failed(PairingError::Protocol)
        );
        assert_eq!(
            app.on_result(result(Status::Ok)),
            Outcome::Failed(PairingError::Protocol)
        );
    }
}

#[test]
fn password_rejects_waiting_local_and_unknown_results() {
    for status in [Status::WaitingLocal as i32, 0, -1, 999] {
        let mut app = password_app();
        erase(app.try_start().unwrap());
        let mut message = result(Status::Ok);
        message.status = status;
        assert_eq!(
            app.on_result(message),
            Outcome::Failed(PairingError::Protocol)
        );
        assert_eq!(
            app.on_result(result(Status::Ok)),
            Outcome::Failed(PairingError::Protocol)
        );
    }
}

#[test]
fn password_maps_each_terminal_failure_and_rejects_a_second_result() {
    for (status, outcome) in [
        (Status::BadCode, Outcome::BadCode { attempts_left: 3 }),
        (Status::Locked, Outcome::Locked),
        (Status::MethodDisabled, Outcome::MethodDisabled),
        (Status::Denied, Outcome::Denied),
        (Status::Expired, Outcome::Expired),
        (Status::DisabledOnPath, Outcome::DisabledOnPath),
    ] {
        let mut app = password_app();
        erase(app.try_start().unwrap());
        assert_eq!(app.on_result(result(status)), outcome);
        assert_eq!(
            app.on_result(result(status)),
            Outcome::Failed(PairingError::Protocol)
        );
        assert_eq!(
            app.on_result(result(Status::Ok)),
            Outcome::Failed(PairingError::Protocol)
        );
        assert_eq!(app.try_start(), Err(PairingError::Protocol));
    }
}

#[test]
fn six_digit_app_maps_method_disabled() {
    let mut app =
        AppPairing::new("012345", DEVICE_FP, HOST_FP, "phone", pb::Platform::Ios).unwrap();
    app.try_start().unwrap();
    assert_eq!(
        app.on_result(result(Status::MethodDisabled)),
        Outcome::MethodDisabled
    );
    assert_eq!(
        app.on_result(result(Status::MethodDisabled)),
        Outcome::Failed(PairingError::Protocol)
    );
}
