//! 系统账户密码配对：密码锁与校验流程。见 04「其他配对方式」、06「配对」。

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::time::{sleep_until, Instant};
use tracing::{info, warn};
use zeroize::Zeroizing;

use crate::sysauth::{SysAuthError, SystemAccount, SystemAuthenticator};

pub const PASSWORD_MAX_FAILURES: u32 = 5;
pub const PASSWORD_LOCK_US: u64 = 15 * 60 * 1_000_000;
pub const PASSWORD_FAILURE_WINDOW_US: u64 = 15 * 60 * 1_000_000;
pub const PASSWORD_MIN_FAILURE_DELAY: Duration = Duration::from_secs(1);
pub const LOCK_FILE: &str = "password_lock.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordVerdict {
    Ok(SystemAccount),
    BadCredentials { attempts_left: u32 },
    Locked { retry_after_s: u32 },
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureState {
    Remaining(u32),
    Locked { retry_after_s: u32 },
}

#[derive(Serialize, Deserialize)]
struct LockState {
    v: u32,
    failures: u32,
    last_failure_at_us: u64,
    locked_until_us: u64,
}

impl Default for LockState {
    fn default() -> Self {
        Self {
            v: 1,
            failures: 0,
            last_failure_at_us: 0,
            locked_until_us: 0,
        }
    }
}

impl LockState {
    fn valid(&self) -> bool {
        self.v == 1
            && self.failures <= PASSWORD_MAX_FAILURES
            && (self.locked_until_us != 0) == (self.failures == PASSWORD_MAX_FAILURES)
            && (self.failures != 0 || self.last_failure_at_us == 0)
    }

    fn retry_after(&self, now_us: u64) -> u32 {
        self.locked_until_us
            .saturating_sub(now_us)
            .div_ceil(1_000_000)
            .clamp(1, u64::from(u32::MAX)) as u32
    }
}

/// One machine-wide lock, shared by every password pairing connection.
/// The mutex covers both state changes and persistence so writes cannot reorder.
pub struct PasswordLock {
    path: PathBuf,
    state: Mutex<LockState>,
}

impl PasswordLock {
    /// Missing state starts unlocked; unreadable, corrupt or unsupported state
    /// starts locked for fifteen minutes, without logging its contents.
    pub fn open(state_dir: &Path, now_us: u64) -> Self {
        let path = state_dir.join(LOCK_FILE);
        let loaded = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<LockState>(&bytes)
                .ok()
                .filter(LockState::valid),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Some(LockState::default()),
            Err(_) => None,
        };
        let corrupt = loaded.is_none();
        let state = loaded.unwrap_or_else(|| {
            warn!(
                event = "pairing.password",
                result = "locked",
                "unreadable password lock state; locking password pairing"
            );
            LockState {
                failures: PASSWORD_MAX_FAILURES,
                last_failure_at_us: now_us,
                locked_until_us: now_us.saturating_add(PASSWORD_LOCK_US),
                ..LockState::default()
            }
        });
        let lock = Self {
            path,
            state: Mutex::new(state),
        };
        if corrupt {
            lock.persist(&lock.state());
        }
        lock
    }

    /// Expiring a lock also clears and persists its failure count.
    pub fn check(&self, now_us: u64) -> Result<(), u32> {
        let mut state = self.state();
        if state.locked_until_us == 0 {
            return Ok(());
        }
        if now_us < state.locked_until_us {
            return Err(state.retry_after(now_us));
        }
        *state = LockState::default();
        self.persist(&state);
        Ok(())
    }

    /// Record at most five failures, including concurrent in-flight attempts.
    /// An existing lock is never extended by another failed attempt.
    pub fn record_failure(&self, now_us: u64) -> FailureState {
        let mut state = self.state();
        if state.locked_until_us != 0 && now_us < state.locked_until_us {
            return FailureState::Locked {
                retry_after_s: state.retry_after(now_us),
            };
        }
        if state.locked_until_us != 0
            || now_us.saturating_sub(state.last_failure_at_us) > PASSWORD_FAILURE_WINDOW_US
        {
            *state = LockState::default();
        }
        state.failures += 1;
        state.last_failure_at_us = now_us;
        let failure = if state.failures == PASSWORD_MAX_FAILURES {
            state.locked_until_us = now_us.saturating_add(PASSWORD_LOCK_US);
            FailureState::Locked {
                retry_after_s: state.retry_after(now_us),
            }
        } else {
            FailureState::Remaining(PASSWORD_MAX_FAILURES - state.failures)
        };
        self.persist(&state);
        failure
    }

    pub fn record_success(&self, _now_us: u64) {
        let mut state = self.state();
        *state = LockState::default();
        self.persist(&state);
    }

    fn state(&self) -> MutexGuard<'_, LockState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn persist(&self, state: &LockState) {
        // Like Store's atomic JSON writes, but failures leave the in-memory lock
        // effective. Do not log paths or file contents here.
        if write_json(&self.path, state).is_err() {
            warn!(
                event = "pairing.password",
                result = "unavailable",
                "could not persist password lock state"
            );
        }
    }
}

/// Validate without retaining the password across the minimum failure delay.
/// Both system calls (including the nonexistent-account dummy call) run on a
/// blocking worker; no account or password is included in tracing fields.
pub async fn verify(
    auth: Arc<dyn SystemAuthenticator>,
    lock: &PasswordLock,
    account: &str,
    password: Zeroizing<Vec<u8>>,
    now_us: u64,
) -> PasswordVerdict {
    let deadline = Instant::now() + PASSWORD_MIN_FAILURE_DELAY;
    let account = account.trim();
    let verdict = if let Err(retry_after_s) = lock.check(now_us) {
        drop(password);
        PasswordVerdict::Locked { retry_after_s }
    } else if account.is_empty()
        || account.len() > 256
        || account.chars().any(char::is_control)
        || password.is_empty()
        || password.len() > 1024
    {
        drop(password);
        failed(lock, now_us)
    } else {
        let account = account.to_owned();
        let result = tokio::task::spawn_blocking(move || {
            let result = authenticate(auth.as_ref(), &account, &password);
            drop(password);
            result
        })
        .await;
        match result {
            Ok(Ok(account)) => {
                lock.record_success(now_us);
                PasswordVerdict::Ok(account)
            }
            Ok(Err(SysAuthError::WrongPassword)) => failed(lock, now_us),
            Ok(Err(SysAuthError::Unavailable(message))) => {
                warn!(event = "pairing.password", result = "unavailable", error = %message);
                PasswordVerdict::Unavailable
            }
            Err(_) => {
                // A panic's payload could contain user input; never log it.
                warn!(
                    event = "pairing.password",
                    result = "unavailable",
                    "system authentication worker failed"
                );
                PasswordVerdict::Unavailable
            }
        }
    };
    if !matches!(verdict, PasswordVerdict::Ok(_)) {
        sleep_until(deadline).await;
    }
    match &verdict {
        PasswordVerdict::Ok(_) => info!(event = "pairing.password", result = "ok"),
        PasswordVerdict::BadCredentials { attempts_left } => {
            info!(event = "pairing.password", result = "bad", attempts_left);
        }
        PasswordVerdict::Locked { .. } => {
            info!(
                event = "pairing.password",
                result = "locked",
                attempts_left = 0
            );
        }
        PasswordVerdict::Unavailable => {}
    }
    verdict
}

fn authenticate(
    auth: &dyn SystemAuthenticator,
    account: &str,
    password: &[u8],
) -> Result<SystemAccount, SysAuthError> {
    let Some(account) = auth.lookup(account)? else {
        // Exercise the same system password path even for unknown accounts.
        // Its result must not change the unknown-account failure verdict.
        let _ = auth.verify("_rsp_nonexistent", password);
        return Err(SysAuthError::WrongPassword);
    };
    if !account.is_admin && !account.is_console_user {
        return Err(SysAuthError::WrongPassword);
    }
    auth.verify(&account.short_name, password)?;
    Ok(account)
}

fn failed(lock: &PasswordLock, now_us: u64) -> PasswordVerdict {
    match lock.record_failure(now_us) {
        FailureState::Remaining(attempts_left) => PasswordVerdict::BadCredentials { attempts_left },
        FailureState::Locked { retry_after_s } => PasswordVerdict::Locked { retry_after_s },
    }
}

// Kept local as Store's helpers are private. Preserve its write + fsync + rename
// + chmod + directory fsync sequence without including accounts in the schema.
fn write_json(path: &Path, value: &LockState) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let tmp = path.with_file_name(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let mut file = options.open(&tmp)?;
    tighten_file(&tmp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)?;
    tighten_file(path)?;
    sync_dir(parent)?;
    Ok(())
}

#[cfg(unix)]
fn tighten_file(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if fs::metadata(path)?.permissions().mode() & 0o777 != 0o600 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn tighten_file(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
