//! SPAKE2 配对状态机。见 docs/dev/04-identity-security.md 配对。
//!
//! Callers supply TLS certificate fingerprints and transport messages themselves.
//! Only SPAKE2 ephemeral randomness is obtained internally; no transport, clock,
//! code lifecycle, attempt counter, or local approval policy is implemented here.

use hkdf::Hkdf;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use zeroize::Zeroizing;

use crate::{ids::id_from_fp, pairfile::spake_password, pb};

pub const MAX_CODE_FAILURES: u32 = 5;
pub const PASSWORD_ACCOUNT_MAX_BYTES: usize = 256;
pub const PASSWORD_MAX_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PairingError {
    #[error("pairing code must contain exactly six ASCII digits")]
    InvalidCode,
    #[error("account or password is empty or too long")]
    InvalidCredential,
    #[error("invalid pairing message or message order")]
    Protocol,
    #[error("pairing code or peer identity did not match")]
    BadCode,
    #[error("host key confirmation failed: possible man in the middle")]
    ManInTheMiddle,
}

/// Proof of mutual key confirmation, not of local approval or pairing-table persistence.
/// It can only be constructed by a successful state-machine transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    device_fp: [u8; 32],
    host_fp: [u8; 32],
}

impl Verified {
    pub fn device_fp(&self) -> &[u8; 32] {
        &self.device_fp
    }

    pub fn host_fp(&self) -> &[u8; 32] {
        &self.host_fp
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    WaitingLocal,
    Paired { host_id: String, host_name: String },
    Denied,
    BadCode { attempts_left: u32 },
    Expired,
    Locked,
    DisabledOnPath,
    MethodDisabled,
    Failed(PairingError),
}

type Spake = Spake2<Ed25519Group>;

enum AppState {
    Ready(Spake, Vec<u8>),
    Started(Spake, Vec<u8>),
    AwaitingConfirm(Confirmation),
    Verified,
    WaitingLocal,
    Paired,
    Finished,
    Failed,
}

pub struct AppPairing {
    state: AppState,
    start: pb::PairStart,
    device_fp: [u8; 32],
    host_fp: [u8; 32],
}

impl AppPairing {
    /// Create a fresh, single-use attempt. `host_fp` must be the pinned TLS peer FP.
    pub fn new(
        code: impl AsRef<str>,
        device_fp: [u8; 32],
        host_fp: [u8; 32],
        device_name: impl Into<String>,
        platform: pb::Platform,
    ) -> Result<Self, PairingError> {
        Ok(Self::from_password(
            code_password(code.as_ref())?,
            device_fp,
            host_fp,
            pb::PairStart {
                device_name: device_name.into(),
                platform: platform as i32,
                has_qr: false,
                ..Default::default()
            },
        ))
    }

    /// Use a pairing file's password with the same pinned TLS identities and confirmation.
    pub fn new_file(
        file_id: [u8; 16],
        secret: &[u8; 32],
        device_fp: [u8; 32],
        host_fp: [u8; 32],
        device_name: impl Into<String>,
        platform: pb::Platform,
    ) -> Self {
        let bytes = spake_password(&file_id, secret);
        Self::from_password(
            bytes_password(bytes.as_ref()),
            device_fp,
            host_fp,
            pb::PairStart {
                device_name: device_name.into(),
                platform: platform as i32,
                has_qr: false,
                pair_file_id: file_id.to_vec(),
            },
        )
    }

    fn from_password(
        password: Password,
        device_fp: [u8; 32],
        host_fp: [u8; 32],
        start: pb::PairStart,
    ) -> Self {
        let (app, host) = identities(&device_fp, &host_fp);
        let (spake, msg) = Spake::start_a(&password, &app, &host);
        Self {
            state: AppState::Ready(spake, msg),
            start,
            device_fp,
            host_fp,
        }
    }

    /// Set the PairStart QR hint before starting (manual-code pairing is the default).
    pub fn with_qr(mut self, has_qr: bool) -> Self {
        self.start.has_qr = has_qr;
        self
    }

    /// Start the exchange. Prefer `try_start` when a local duplicate call is possible.
    /// The required infallible API returns an empty message on misuse and terminates
    /// the attempt; subsequent fallible transitions return `Protocol`.
    #[allow(clippy::manual_unwrap_or_default)] // Keep explicit error handling; no unwrap-family calls.
    pub fn start(&mut self) -> pb::PairStart {
        match self.try_start() {
            Ok(start) => start,
            Err(_) => pb::PairStart::default(),
        }
    }

    pub fn try_start(&mut self) -> Result<pb::PairStart, PairingError> {
        match std::mem::replace(&mut self.state, AppState::Failed) {
            AppState::Ready(spake, msg) => {
                self.state = AppState::Started(spake, msg);
                Ok(self.start.clone())
            }
            _ => Err(PairingError::Protocol),
        }
    }

    /// Process the Host's first SPAKE2 message, then send App SPAKE2 and App MAC.
    pub fn on_spake(
        &mut self,
        message: pb::PairSpake,
    ) -> Result<(pb::PairSpake, pb::PairConfirm), PairingError> {
        let AppState::Started(spake, msg_app) =
            std::mem::replace(&mut self.state, AppState::Failed)
        else {
            return Err(PairingError::Protocol);
        };
        let key = spake
            .finish(&message.msg)
            .map_err(|_| PairingError::Protocol)?;
        let confirm =
            Confirmation::new(&key, &msg_app, &message.msg, &self.device_fp, &self.host_fp)?;
        let mac = confirm.mac(b'A')?;
        self.state = AppState::AwaitingConfirm(confirm);
        Ok((pb::PairSpake { msg: msg_app }, pb::PairConfirm { mac }))
    }

    pub fn on_confirm(&mut self, message: pb::PairConfirm) -> Result<Verified, PairingError> {
        let AppState::AwaitingConfirm(confirm) =
            std::mem::replace(&mut self.state, AppState::Failed)
        else {
            return Err(PairingError::Protocol);
        };
        if !confirm.verify(b'B', &message.mac)? {
            return Err(PairingError::ManInTheMiddle);
        }
        self.state = AppState::Verified;
        Ok(Verified {
            device_fp: self.device_fp,
            host_fp: self.host_fp,
        })
    }

    pub fn is_verified(&self) -> bool {
        matches!(
            self.state,
            AppState::Verified | AppState::WaitingLocal | AppState::Paired
        )
    }

    /// An OK result cannot bypass key confirmation. Terminal results are single-use.
    /// WAITING_LOCAL is optional, but if present it may only occur once after confirmation.
    pub fn on_result(&mut self, result: pb::PairResult) -> Outcome {
        use pb::pair_result::Status;

        let previous = std::mem::replace(&mut self.state, AppState::Failed);
        let confirmed = matches!(previous, AppState::Verified | AppState::WaitingLocal);
        let active = matches!(
            previous,
            AppState::Started(_, _)
                | AppState::AwaitingConfirm(_)
                | AppState::Verified
                | AppState::WaitingLocal
        );
        if !active {
            return Outcome::Failed(PairingError::Protocol);
        }
        let outcome = match Status::try_from(result.status) {
            Ok(Status::WaitingLocal) if matches!(previous, AppState::Verified) => {
                self.state = AppState::WaitingLocal;
                return Outcome::WaitingLocal;
            }
            Ok(Status::Ok) if confirmed && result.host_id == id_from_fp(&self.host_fp) => {
                self.state = AppState::Paired;
                return Outcome::Paired {
                    host_id: result.host_id,
                    host_name: result.host_name,
                };
            }
            Ok(Status::Denied) => Outcome::Denied,
            Ok(Status::BadCode) => Outcome::BadCode {
                attempts_left: result.attempts_left,
            },
            Ok(Status::Expired) => Outcome::Expired,
            Ok(Status::Locked) => Outcome::Locked,
            Ok(Status::DisabledOnPath) => Outcome::DisabledOnPath,
            Ok(Status::MethodDisabled) => Outcome::MethodDisabled,
            _ => return Outcome::Failed(PairingError::Protocol),
        };
        self.state = AppState::Finished;
        outcome
    }
}

enum PasswordState {
    Ready {
        account: Zeroizing<String>,
        password: Zeroizing<Vec<u8>>,
    },
    Started,
    Finished,
    Failed,
}

/// A single password request over TLS to a Host whose certificate fingerprint is pinned.
/// Credentials are erased on drop, or moved to the caller's message by `try_start`.
pub struct AppPasswordPairing {
    state: PasswordState,
    host_fp: [u8; 32],
    device_name: String,
    platform: pb::Platform,
}

impl std::fmt::Debug for AppPasswordPairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match self.state {
            PasswordState::Ready { .. } => "Ready",
            PasswordState::Started => "Started",
            PasswordState::Finished => "Finished",
            PasswordState::Failed => "Failed",
        };
        f.debug_struct("AppPasswordPairing")
            .field("state", &state)
            .field("host_fp", &self.host_fp)
            .finish_non_exhaustive()
    }
}

impl AppPasswordPairing {
    /// Account is trimmed, 1–256 UTF-8 bytes, with no control characters.
    /// Password must contain 1–1024 bytes and is never trimmed.
    pub fn new(
        account: &str,
        password: Zeroizing<Vec<u8>>,
        host_fp: [u8; 32],
        device_name: impl Into<String>,
        platform: pb::Platform,
    ) -> Result<Self, PairingError> {
        let account = account.trim();
        if account.is_empty()
            || account.len() > PASSWORD_ACCOUNT_MAX_BYTES
            || account.chars().any(char::is_control)
            || password.is_empty()
            || password.len() > PASSWORD_MAX_BYTES
        {
            return Err(PairingError::InvalidCredential);
        }
        Ok(Self {
            state: PasswordState::Ready {
                account: Zeroizing::new(account.to_owned()),
                password,
            },
            host_fp,
            device_name: device_name.into(),
            platform,
        })
    }

    /// Emit exactly one request. The caller must erase its credentials after encoding/sending.
    pub fn try_start(&mut self) -> Result<pb::PairPassword, PairingError> {
        let PasswordState::Ready {
            mut account,
            mut password,
        } = std::mem::replace(&mut self.state, PasswordState::Failed)
        else {
            return Err(PairingError::Protocol);
        };
        self.state = PasswordState::Started;
        Ok(pb::PairPassword {
            device_name: std::mem::take(&mut self.device_name),
            platform: self.platform as i32,
            account: std::mem::take(&mut *account),
            password: std::mem::take(&mut *password),
        })
    }

    /// Accept one terminal result after the request. Password pairing never waits for approval.
    pub fn on_result(&mut self, result: pb::PairResult) -> Outcome {
        use pb::pair_result::Status;

        if !matches!(
            std::mem::replace(&mut self.state, PasswordState::Failed),
            PasswordState::Started
        ) {
            return Outcome::Failed(PairingError::Protocol);
        }
        let outcome = match Status::try_from(result.status) {
            Ok(Status::Ok) if result.host_id == id_from_fp(&self.host_fp) => Outcome::Paired {
                host_id: result.host_id,
                host_name: result.host_name,
            },
            Ok(Status::Denied) => Outcome::Denied,
            Ok(Status::BadCode) => Outcome::BadCode {
                attempts_left: result.attempts_left,
            },
            Ok(Status::Expired) => Outcome::Expired,
            Ok(Status::Locked) => Outcome::Locked,
            Ok(Status::DisabledOnPath) => Outcome::DisabledOnPath,
            Ok(Status::MethodDisabled) => Outcome::MethodDisabled,
            _ => return Outcome::Failed(PairingError::Protocol),
        };
        self.state = PasswordState::Finished;
        outcome
    }
}

enum HostState {
    Ready(Spake, Vec<u8>),
    Started(Spake, Vec<u8>),
    AwaitingConfirm(Confirmation),
    Verified,
    Failed,
}

pub struct HostPairing {
    state: HostState,
    device_fp: [u8; 32],
    host_fp: [u8; 32],
}

impl HostPairing {
    /// `device_fp` and `host_fp` come from the actual TLS connection's certificates.
    pub fn new(
        code: impl AsRef<str>,
        host_fp: [u8; 32],
        device_fp: [u8; 32],
    ) -> Result<Self, PairingError> {
        Ok(Self::from_password(
            code_password(code.as_ref())?,
            host_fp,
            device_fp,
        ))
    }

    pub fn new_file(
        file_id: &[u8; 16],
        secret: &[u8; 32],
        host_fp: [u8; 32],
        device_fp: [u8; 32],
    ) -> Self {
        let bytes = spake_password(file_id, secret);
        Self::from_password(bytes_password(bytes.as_ref()), host_fp, device_fp)
    }

    fn from_password(password: Password, host_fp: [u8; 32], device_fp: [u8; 32]) -> Self {
        let (app, host) = identities(&device_fp, &host_fp);
        let (spake, msg) = Spake::start_b(&password, &app, &host);
        Self {
            state: HostState::Ready(spake, msg),
            device_fp,
            host_fp,
        }
    }

    /// Emit Host SPAKE2 first. Use `try_on_start` to report a duplicate immediately.
    /// The required infallible API returns an empty SPAKE2 message on misuse and
    /// terminates the attempt; subsequent transitions return `Protocol`.
    #[allow(clippy::manual_unwrap_or_default)] // Keep explicit error handling; no unwrap-family calls.
    pub fn on_start(&mut self, start: pb::PairStart) -> pb::PairSpake {
        match self.try_on_start(start) {
            Ok(message) => message,
            Err(_) => pb::PairSpake::default(),
        }
    }

    pub fn try_on_start(&mut self, _start: pb::PairStart) -> Result<pb::PairSpake, PairingError> {
        match std::mem::replace(&mut self.state, HostState::Failed) {
            HostState::Ready(spake, msg) => {
                let message = pb::PairSpake { msg: msg.clone() };
                self.state = HostState::Started(spake, msg);
                Ok(message)
            }
            _ => Err(PairingError::Protocol),
        }
    }

    pub fn on_spake(&mut self, message: pb::PairSpake) -> Result<(), PairingError> {
        let HostState::Started(spake, msg_host) =
            std::mem::replace(&mut self.state, HostState::Failed)
        else {
            return Err(PairingError::Protocol);
        };
        let key = spake
            .finish(&message.msg)
            .map_err(|_| PairingError::Protocol)?;
        // Fingerprints are also needed in the confirmation transcript, separately
        // from their binding inside SPAKE2's identities.
        self.state = HostState::AwaitingConfirm(Confirmation::new(
            &key,
            &message.msg,
            &msg_host,
            &self.device_fp,
            &self.host_fp,
        )?);
        Ok(())
    }

    pub fn on_confirm(
        &mut self,
        message: pb::PairConfirm,
    ) -> Result<pb::PairConfirm, PairingError> {
        let HostState::AwaitingConfirm(confirm) =
            std::mem::replace(&mut self.state, HostState::Failed)
        else {
            return Err(PairingError::Protocol);
        };
        if !confirm.verify(b'A', &message.mac)? {
            return Err(PairingError::BadCode);
        }
        let mac = confirm.mac(b'B')?;
        self.state = HostState::Verified;
        Ok(pb::PairConfirm { mac })
    }

    pub fn is_verified(&self) -> bool {
        matches!(self.state, HostState::Verified)
    }
}

fn code_password(code: &str) -> Result<Password, PairingError> {
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(PairingError::InvalidCode);
    }
    Ok(bytes_password(code.as_bytes()))
}

fn bytes_password(bytes: &[u8]) -> Password {
    Password::new(bytes)
}

fn identities(device_fp: &[u8; 32], host_fp: &[u8; 32]) -> (Identity, Identity) {
    let app = [b"rsp-app:".as_slice(), device_fp].concat();
    let host = [b"rsp-host:".as_slice(), host_fp].concat();
    (Identity::new(&app), Identity::new(&host))
}

struct Confirmation {
    key: [u8; 32],
    transcript: Vec<u8>,
}

impl Confirmation {
    fn new(
        key: &[u8],
        msg_app: &[u8],
        msg_host: &[u8],
        device_fp: &[u8; 32],
        host_fp: &[u8; 32],
    ) -> Result<Self, PairingError> {
        let mut confirm_key = [0; 32];
        Hkdf::<Sha256>::new(Some(b""), key)
            .expand(b"rsp-pair-confirm", &mut confirm_key)
            .map_err(|_| PairingError::Protocol)?;
        Ok(Self {
            key: confirm_key,
            transcript: [msg_app, msg_host, device_fp, host_fp].concat(),
        })
    }

    fn hmac(&self, role: u8) -> Result<Hmac<Sha256>, PairingError> {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.key).map_err(|_| PairingError::Protocol)?;
        mac.update(&[role]);
        mac.update(&self.transcript);
        Ok(mac)
    }

    fn mac(&self, role: u8) -> Result<Vec<u8>, PairingError> {
        Ok(self.hmac(role)?.finalize().into_bytes().to_vec())
    }

    fn verify(&self, role: u8, received: &[u8]) -> Result<bool, PairingError> {
        // RustCrypto's verify_slice performs a constant-time tag comparison.
        Ok(self.hmac(role)?.verify_slice(received).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::Confirmation;
    use data_encoding::HEXLOWER;
    use std::collections::BTreeMap;

    #[test]
    fn fixed_confirmation_vector() {
        let values: BTreeMap<_, _> = include_str!("../tests/vectors/pairing/confirmation.txt")
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .map(|line| {
                let (key, hex) = line.split_once('=').unwrap();
                (key, HEXLOWER.decode(hex.as_bytes()).unwrap())
            })
            .collect();
        let device_fp = values["device_fp"].as_slice().try_into().unwrap();
        let host_fp = values["host_fp"].as_slice().try_into().unwrap();
        let confirm = Confirmation::new(
            &values["k"],
            &values["msg_app"],
            &values["msg_host"],
            &device_fp,
            &host_fp,
        )
        .unwrap();
        assert_eq!(confirm.key.as_slice(), values["k_confirm"]);
        assert_eq!(confirm.transcript, values["transcript"]);
        assert_eq!(confirm.mac(b'A').unwrap(), values["mac_app"]);
        assert_eq!(confirm.mac(b'B').unwrap(), values["mac_host"]);
        assert!(confirm.verify(b'A', &values["mac_app"]).unwrap());
        assert!(confirm.verify(b'B', &values["mac_host"]).unwrap());
        assert!(!confirm.verify(b'B', &values["mac_app"]).unwrap());
        // Changing either raw fingerprint independently changes confirmation,
        // even when the shared K and both SPAKE2 messages stay fixed.
        for device in [true, false] {
            let mut device_fp = device_fp;
            let mut host_fp = host_fp;
            if device {
                device_fp[31] ^= 1;
            } else {
                host_fp[31] ^= 1;
            }
            let changed = Confirmation::new(
                &values["k"],
                &values["msg_app"],
                &values["msg_host"],
                &device_fp,
                &host_fp,
            )
            .unwrap();
            assert!(!changed.verify(b'A', &values["mac_app"]).unwrap());
            assert!(!changed.verify(b'B', &values["mac_host"]).unwrap());
        }
    }
}
