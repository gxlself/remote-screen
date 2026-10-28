use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex, OnceLock};
use std::thread::ThreadId;

use host_core::pwpair::{
    verify, FailureState, PasswordLock, PasswordVerdict, LOCK_FILE, PASSWORD_FAILURE_WINDOW_US,
    PASSWORD_LOCK_US, PASSWORD_MAX_FAILURES, PASSWORD_MIN_FAILURE_DELAY,
};
use host_core::sysauth::{SysAuthError, SystemAccount, SystemAuthenticator};
use serde_json::{json, Value};
use tokio::time::Instant;
use zeroize::Zeroizing;

const NOW: u64 = 1_792_465_134_000_000;
const SHORT_NAME: &str = "pwpair_private_login";
const FULL_NAME: &str = "Pwpair Private Full Name";
const PASSWORD: &str = "pwpair-private-password!";
const WRONG_PASSWORD: &str = "pwpair-private-wrong-password!";

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        captured_logs();
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target"));
        let path = root.join(format!(
            "rs-pwpair-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn state(&self) -> Value {
        serde_json::from_slice(&fs::read(self.0.join(LOCK_FILE)).unwrap()).unwrap()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[derive(Clone)]
struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn captured_logs() -> &'static Arc<Mutex<Vec<u8>>> {
    static LOGS: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    LOGS.get_or_init(|| {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(LogWriter(bytes.clone()))
            .finish();
        tracing::subscriber::set_global_default(subscriber).unwrap();
        bytes
    })
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
struct Calls {
    lookups: Vec<String>,
    verifications: Vec<String>,
    threads: Vec<ThreadId>,
}

struct FakeAuth {
    accounts: Vec<SystemAccount>,
    password: Zeroizing<Vec<u8>>,
    calls: Mutex<Calls>,
    lookup_error: Mutex<Option<SysAuthError>>,
    verify_override: Mutex<Option<Result<(), SysAuthError>>>,
    barrier: Option<Arc<Barrier>>,
}

impl FakeAuth {
    fn new() -> Self {
        Self {
            accounts: vec![admin()],
            password: secret(PASSWORD),
            calls: Mutex::new(Calls::default()),
            lookup_error: Mutex::new(None),
            verify_override: Mutex::new(None),
            barrier: None,
        }
    }

    fn calls(&self) -> Calls {
        self.calls.lock().unwrap().clone()
    }
}

impl SystemAuthenticator for FakeAuth {
    fn lookup(&self, account: &str) -> Result<Option<SystemAccount>, SysAuthError> {
        {
            let mut calls = self.calls.lock().unwrap();
            calls.lookups.push(account.to_owned());
            calls.threads.push(std::thread::current().id());
        }
        if let Some(error) = self.lookup_error.lock().unwrap().clone() {
            return Err(error);
        }
        Ok(self
            .accounts
            .iter()
            .find(|entry| entry.short_name == account || entry.full_name == account)
            .cloned())
    }

    fn verify(&self, short_name: &str, password: &[u8]) -> Result<(), SysAuthError> {
        {
            let mut calls = self.calls.lock().unwrap();
            calls.verifications.push(short_name.to_owned());
            calls.threads.push(std::thread::current().id());
        }
        if let Some(barrier) = &self.barrier {
            barrier.wait();
        }
        if let Some(result) = self.verify_override.lock().unwrap().clone() {
            return result;
        }
        if self
            .accounts
            .iter()
            .any(|entry| entry.short_name == short_name)
            && password == self.password.as_slice()
        {
            Ok(())
        } else {
            Err(SysAuthError::WrongPassword)
        }
    }
}

fn secret(value: &str) -> Zeroizing<Vec<u8>> {
    Zeroizing::new(value.as_bytes().to_vec())
}

fn admin() -> SystemAccount {
    SystemAccount {
        short_name: SHORT_NAME.into(),
        full_name: FULL_NAME.into(),
        is_admin: true,
        is_console_user: false,
    }
}

fn bad(attempts_left: u32) -> PasswordVerdict {
    PasswordVerdict::BadCredentials { attempts_left }
}

async fn attempt(
    auth: &Arc<FakeAuth>,
    lock: &PasswordLock,
    account: &str,
    password: &str,
    now_us: u64,
) -> PasswordVerdict {
    let start = Instant::now();
    let result = verify(auth.clone(), lock, account, secret(password), now_us).await;
    if matches!(result, PasswordVerdict::Ok(_)) {
        assert!(start.elapsed() < PASSWORD_MIN_FAILURE_DELAY);
    } else {
        assert!(start.elapsed() >= PASSWORD_MIN_FAILURE_DELAY);
    }
    result
}

#[tokio::test(start_paused = true)]
async fn admin_success_resets_failures_and_uses_blocking_worker() {
    let dir = TestDir::new();
    let lock = PasswordLock::open(&dir.0, NOW);
    let auth = Arc::new(FakeAuth::new());
    assert_eq!(
        attempt(&auth, &lock, SHORT_NAME, WRONG_PASSWORD, NOW).await,
        bad(4)
    );
    assert_eq!(
        attempt(&auth, &lock, SHORT_NAME, PASSWORD, NOW + 1).await,
        PasswordVerdict::Ok(admin())
    );
    assert_eq!(
        dir.state(),
        json!({"v": 1, "failures": 0, "last_failure_at_us": 0, "locked_until_us": 0})
    );
    let calls = auth.calls();
    assert_eq!(calls.verifications, [SHORT_NAME, SHORT_NAME]);
    assert!(calls
        .threads
        .iter()
        .all(|id| *id != std::thread::current().id()));
}

#[tokio::test(start_paused = true)]
async fn full_name_and_trimmed_account_resolve_to_short_name() {
    let dir = TestDir::new();
    let lock = PasswordLock::open(&dir.0, NOW);
    let auth = Arc::new(FakeAuth::new());
    assert_eq!(
        attempt(&auth, &lock, &format!("  {FULL_NAME}  "), PASSWORD, NOW).await,
        PasswordVerdict::Ok(admin())
    );
    assert_eq!(auth.calls().lookups, [FULL_NAME]);
    assert_eq!(auth.calls().verifications, [SHORT_NAME]);
}

#[tokio::test(start_paused = true)]
async fn five_failures_lock_and_sixth_skips_system_authentication() {
    let dir = TestDir::new();
    let lock = PasswordLock::open(&dir.0, NOW);
    let auth = Arc::new(FakeAuth::new());
    for count in 1..PASSWORD_MAX_FAILURES {
        assert_eq!(
            attempt(&auth, &lock, SHORT_NAME, WRONG_PASSWORD, NOW).await,
            bad(PASSWORD_MAX_FAILURES - count)
        );
    }
    assert_eq!(
        attempt(&auth, &lock, SHORT_NAME, WRONG_PASSWORD, NOW).await,
        PasswordVerdict::Locked { retry_after_s: 900 }
    );
    let calls = auth.calls();
    let state = dir.state();
    assert_eq!(calls.lookups.len(), 5);
    assert_eq!(calls.verifications.len(), 5);
    assert_eq!(state["failures"], 5);
    assert_eq!(state["locked_until_us"], NOW + PASSWORD_LOCK_US);
    // Lock precedes input validation and even a correct password cannot pass it.
    for (account, password) in [(SHORT_NAME, PASSWORD), ("", "")] {
        assert_eq!(
            attempt(&auth, &lock, account, password, NOW + 1_000_000).await,
            PasswordVerdict::Locked { retry_after_s: 899 }
        );
    }
    assert_eq!(auth.calls(), calls);
    assert_eq!(dir.state(), state);
    assert_eq!(lock.check(NOW + PASSWORD_LOCK_US - 1), Err(1));
    assert_eq!(
        attempt(&auth, &lock, SHORT_NAME, PASSWORD, NOW + PASSWORD_LOCK_US).await,
        PasswordVerdict::Ok(admin())
    );
    assert_eq!(dir.state()["failures"], 0);
    assert_eq!(dir.state()["locked_until_us"], 0);
}

#[tokio::test(start_paused = true)]
async fn failure_window_resets_only_after_fifteen_minutes() {
    let dir = TestDir::new();
    let lock = PasswordLock::open(&dir.0, NOW);
    let auth = Arc::new(FakeAuth::new());
    assert_eq!(attempt(&auth, &lock, SHORT_NAME, "bad", NOW).await, bad(4));
    let boundary = NOW + PASSWORD_FAILURE_WINDOW_US;
    assert_eq!(
        attempt(&auth, &lock, SHORT_NAME, "bad", boundary).await,
        bad(3)
    );
    let after_window = boundary + PASSWORD_FAILURE_WINDOW_US + 1;
    assert_eq!(
        attempt(&auth, &lock, SHORT_NAME, "bad", after_window).await,
        bad(4)
    );
    assert_eq!(dir.state()["failures"], 1);
    assert_eq!(dir.state()["last_failure_at_us"], after_window);
}

#[tokio::test(start_paused = true)]
async fn every_bad_input_matches_the_wrong_password_verdict_and_counts() {
    let cases = [
        ("pwpair_missing_account".to_owned(), PASSWORD.to_owned()),
        ("pwpair_unprivileged".to_owned(), PASSWORD.to_owned()),
        (String::new(), PASSWORD.to_owned()),
        ("   ".to_owned(), PASSWORD.to_owned()),
        ("a".repeat(257), PASSWORD.to_owned()),
        ("é".repeat(129), PASSWORD.to_owned()),
        ("private\u{7}account".to_owned(), PASSWORD.to_owned()),
        (SHORT_NAME.to_owned(), String::new()),
        (SHORT_NAME.to_owned(), "p".repeat(1025)),
    ];
    for (account, password) in cases {
        let dir = TestDir::new();
        let reference_dir = TestDir::new();
        let lock = PasswordLock::open(&dir.0, NOW);
        let reference = PasswordLock::open(&reference_dir.0, NOW);
        let mut fake = FakeAuth::new();
        fake.accounts.push(SystemAccount {
            short_name: "pwpair_unprivileged".into(),
            full_name: "Pwpair Unprivileged".into(),
            is_admin: false,
            is_console_user: false,
        });
        let auth = Arc::new(fake);
        for count in 1..=PASSWORD_MAX_FAILURES {
            let expected = attempt(&auth, &reference, SHORT_NAME, WRONG_PASSWORD, NOW).await;
            let before = auth.calls();
            assert_eq!(
                attempt(&auth, &lock, &account, &password, NOW).await,
                expected
            );
            assert_eq!(dir.state()["failures"], count);
            let after = auth.calls();
            if account == "pwpair_missing_account" {
                assert_eq!(after.lookups.len(), before.lookups.len() + 1);
                assert_eq!(after.verifications.len(), before.verifications.len() + 1);
                assert_eq!(after.verifications.last().unwrap(), "_rsp_nonexistent");
            } else if account == "pwpair_unprivileged" {
                assert_eq!(after.lookups.len(), before.lookups.len() + 1);
                assert_eq!(after.verifications, before.verifications);
            } else {
                assert_eq!(after, before);
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn nonexistent_account_discards_dummy_success_and_unavailability() {
    for dummy_result in [
        Ok(()),
        Err(SysAuthError::Unavailable("directory offline".into())),
    ] {
        let dir = TestDir::new();
        let lock = PasswordLock::open(&dir.0, NOW);
        let auth = Arc::new(FakeAuth::new());
        *auth.verify_override.lock().unwrap() = Some(dummy_result);
        assert_eq!(
            attempt(&auth, &lock, "pwpair_missing_account", PASSWORD, NOW).await,
            bad(4)
        );
        assert_eq!(auth.calls().verifications, ["_rsp_nonexistent"]);
        assert_eq!(dir.state()["failures"], 1);
    }
}

#[tokio::test(start_paused = true)]
async fn console_user_is_allowed_without_admin_membership() {
    let dir = TestDir::new();
    let lock = PasswordLock::open(&dir.0, NOW);
    let mut fake = FakeAuth::new();
    fake.accounts[0].is_admin = false;
    fake.accounts[0].is_console_user = true;
    let expected = fake.accounts[0].clone();
    let auth = Arc::new(fake);
    assert_eq!(
        attempt(&auth, &lock, SHORT_NAME, PASSWORD, NOW).await,
        PasswordVerdict::Ok(expected)
    );
}

#[tokio::test(start_paused = true)]
async fn byte_length_boundaries_are_accepted() {
    for account in ["a".to_owned(), "é".repeat(128)] {
        for password in ["p".to_owned(), "p".repeat(1024)] {
            let dir = TestDir::new();
            let lock = PasswordLock::open(&dir.0, NOW);
            let mut fake = FakeAuth::new();
            fake.accounts[0].short_name = account.clone();
            fake.password = secret(&password);
            let expected = fake.accounts[0].clone();
            let auth = Arc::new(fake);
            assert_eq!(
                attempt(&auth, &lock, &account, &password, NOW).await,
                PasswordVerdict::Ok(expected)
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn unavailable_does_not_change_counts_or_file() {
    let dir = TestDir::new();
    let lock = PasswordLock::open(&dir.0, NOW);
    let auth = Arc::new(FakeAuth::new());
    assert_eq!(attempt(&auth, &lock, SHORT_NAME, "bad", NOW).await, bad(4));
    let state = dir.state();
    *auth.lookup_error.lock().unwrap() = Some(SysAuthError::Unavailable("lookup offline".into()));
    assert_eq!(
        attempt(&auth, &lock, SHORT_NAME, PASSWORD, NOW).await,
        PasswordVerdict::Unavailable
    );
    assert_eq!(auth.calls().verifications.len(), 1);
    assert_eq!(dir.state(), state);
    *auth.lookup_error.lock().unwrap() = None;
    *auth.verify_override.lock().unwrap() =
        Some(Err(SysAuthError::Unavailable("verify offline".into())));
    assert_eq!(
        attempt(&auth, &lock, SHORT_NAME, PASSWORD, NOW).await,
        PasswordVerdict::Unavailable
    );
    assert_eq!(dir.state(), state);
    assert_eq!(auth.calls().verifications.len(), 2);
}

#[test]
fn restart_preserves_counts_and_lock_and_expiry_persists_reset() {
    let dir = TestDir::new();
    {
        let lock = PasswordLock::open(&dir.0, NOW);
        assert_eq!(lock.check(NOW), Ok(()));
        assert_eq!(lock.record_failure(NOW), FailureState::Remaining(4));
        assert_eq!(lock.record_failure(NOW), FailureState::Remaining(3));
    }
    {
        let lock = PasswordLock::open(&dir.0, NOW);
        assert_eq!(lock.record_failure(NOW), FailureState::Remaining(2));
        assert_eq!(lock.record_failure(NOW), FailureState::Remaining(1));
        assert_eq!(
            lock.record_failure(NOW),
            FailureState::Locked { retry_after_s: 900 }
        );
    }
    let lock = PasswordLock::open(&dir.0, NOW + 1_000_000);
    assert_eq!(lock.check(NOW + 1_000_000), Err(899));
    assert_eq!(lock.check(NOW + PASSWORD_LOCK_US - 1), Err(1));
    assert_eq!(lock.check(NOW + PASSWORD_LOCK_US), Ok(()));
    assert_eq!(dir.state()["failures"], 0);
    assert_eq!(dir.state()["last_failure_at_us"], 0);
    assert_eq!(dir.state()["locked_until_us"], 0);
    let reopened = PasswordLock::open(&dir.0, NOW + PASSWORD_LOCK_US);
    assert_eq!(reopened.check(NOW + PASSWORD_LOCK_US), Ok(()));
    assert_eq!(
        reopened.record_failure(NOW + PASSWORD_LOCK_US),
        FailureState::Remaining(4)
    );
}

#[test]
fn corrupt_unsupported_and_inconsistent_state_fail_closed() {
    for content in [
        "not json, private account pwpair_private_login",
        r#"{"v":2,"failures":0,"last_failure_at_us":0,"locked_until_us":0}"#,
        r#"{"v":1,"failures":6,"last_failure_at_us":0,"locked_until_us":0}"#,
        r#"{"v":1,"failures":5,"last_failure_at_us":0,"locked_until_us":0}"#,
        r#"{"v":1,"failures":0,"last_failure_at_us":0,"locked_until_us":1}"#,
        r#"{"v":1,"failures":-1,"last_failure_at_us":0,"locked_until_us":0}"#,
        r#"{"v":1,"failures":0}"#,
    ] {
        let dir = TestDir::new();
        fs::write(dir.0.join(LOCK_FILE), content).unwrap();
        let lock = PasswordLock::open(&dir.0, NOW);
        assert_eq!(lock.check(NOW), Err(900));
        assert_eq!(dir.state()["failures"], 5);
        assert_eq!(dir.state()["locked_until_us"], NOW + PASSWORD_LOCK_US);
        let reopened = PasswordLock::open(&dir.0, NOW + 1_000_000);
        assert_eq!(reopened.check(NOW + 1_000_000), Err(899));
    }
}

#[test]
fn unreadable_state_is_locked_and_write_failure_keeps_memory_effective() {
    let dir = TestDir::new();
    fs::create_dir(dir.0.join(LOCK_FILE)).unwrap();
    let corrupt = PasswordLock::open(&dir.0, NOW);
    assert_eq!(corrupt.check(NOW), Err(900));
    fs::remove_dir(dir.0.join(LOCK_FILE)).unwrap();
    fs::remove_file(dir.0.join(format!(".{LOCK_FILE}.tmp"))).unwrap();
    let lock = PasswordLock::open(&dir.0, NOW);
    // A directory at the temporary filename deterministically prevents writing,
    // including when tests are run as root (chmod alone would not).
    fs::create_dir(dir.0.join(format!(".{LOCK_FILE}.tmp"))).unwrap();
    for count in 1..PASSWORD_MAX_FAILURES {
        assert_eq!(
            lock.record_failure(NOW),
            FailureState::Remaining(PASSWORD_MAX_FAILURES - count)
        );
    }
    assert_eq!(
        lock.record_failure(NOW),
        FailureState::Locked { retry_after_s: 900 }
    );
    assert_eq!(lock.check(NOW), Err(900));
    assert_eq!(
        lock.record_failure(NOW + 1_000_000),
        FailureState::Locked { retry_after_s: 899 }
    );
    lock.record_success(NOW);
    assert_eq!(lock.check(NOW), Ok(()));
}

#[cfg(unix)]
#[test]
fn lock_file_and_reused_temporary_file_have_private_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TestDir::new();
    let tmp = dir.0.join(format!(".{LOCK_FILE}.tmp"));
    fs::write(&tmp, b"stale").unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o666)).unwrap();
    let lock = PasswordLock::open(&dir.0, NOW);
    lock.record_failure(NOW);
    let path = dir.0.join(LOCK_FILE);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!tmp.exists());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    lock.record_success(NOW);
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[tokio::test(start_paused = true)]
async fn concurrent_failures_are_capped_at_five_without_extending_lock() {
    let dir = TestDir::new();
    let lock = Arc::new(PasswordLock::open(&dir.0, NOW));
    let mut fake = FakeAuth::new();
    // Force all ten connections to pass check before any failure is recorded.
    fake.barrier = Some(Arc::new(Barrier::new(10)));
    let auth = Arc::new(fake);
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..10 {
        let lock = lock.clone();
        let auth = auth.clone();
        tasks.spawn(async move { attempt(&auth, &lock, SHORT_NAME, WRONG_PASSWORD, NOW).await });
    }
    let mut remaining = Vec::new();
    let mut locked = 0;
    while let Some(result) = tasks.join_next().await {
        match result.unwrap() {
            PasswordVerdict::BadCredentials { attempts_left } => remaining.push(attempts_left),
            PasswordVerdict::Locked { retry_after_s: 900 } => locked += 1,
            other => panic!("unexpected verdict: {other:?}"),
        }
    }
    remaining.sort_unstable();
    assert_eq!(remaining, [1, 2, 3, 4]);
    assert_eq!(locked, 6);
    assert_eq!(auth.calls().lookups.len(), 10);
    assert_eq!(auth.calls().verifications.len(), 10);
    assert_eq!(dir.state()["failures"], 5);
    assert_eq!(dir.state()["locked_until_us"], NOW + PASSWORD_LOCK_US);
    assert_eq!(lock.check(NOW), Err(900));
    let calls = auth.calls();
    assert_eq!(
        attempt(&auth, &lock, SHORT_NAME, WRONG_PASSWORD, NOW + 1_000_000).await,
        PasswordVerdict::Locked { retry_after_s: 899 }
    );
    assert_eq!(auth.calls(), calls);
}

#[tokio::test(start_paused = true)]
async fn logs_and_persistence_do_not_contain_accounts_passwords_or_lengths() {
    let dir = TestDir::new();
    let lock = PasswordLock::open(&dir.0, NOW);
    let auth = Arc::new(FakeAuth::new());
    assert_eq!(
        attempt(&auth, &lock, SHORT_NAME, WRONG_PASSWORD, NOW).await,
        bad(4)
    );
    assert_eq!(
        attempt(&auth, &lock, FULL_NAME, PASSWORD, NOW).await,
        PasswordVerdict::Ok(admin())
    );
    // Include a password accidentally supplied in the account field.
    assert_eq!(
        attempt(&auth, &lock, PASSWORD, WRONG_PASSWORD, NOW).await,
        bad(4)
    );
    *auth.lookup_error.lock().unwrap() =
        Some(SysAuthError::Unavailable("directory offline".into()));
    assert_eq!(
        attempt(&auth, &lock, SHORT_NAME, PASSWORD, NOW).await,
        PasswordVerdict::Unavailable
    );
    for _ in 0..4 {
        lock.record_failure(NOW);
    }
    assert_eq!(
        attempt(&auth, &lock, SHORT_NAME, PASSWORD, NOW).await,
        PasswordVerdict::Locked { retry_after_s: 900 }
    );
    let file = fs::read_to_string(dir.0.join(LOCK_FILE)).unwrap();
    let logs = String::from_utf8(captured_logs().lock().unwrap().clone()).unwrap();
    for private in [
        SHORT_NAME,
        FULL_NAME,
        PASSWORD,
        WRONG_PASSWORD,
        "pwpair_missing_account",
        "pwpair_unprivileged",
    ] {
        assert!(!file.contains(private));
        assert!(!logs.contains(private));
    }
    assert!(logs.contains("pairing.password"));
    for result in ["ok", "bad", "locked", "unavailable"] {
        assert!(logs.contains(&format!("result=\"{result}\"")));
    }
    assert!(!logs.contains("length"));
    assert!(!logs.contains("account="));
    assert!(!logs.contains("password="));
    let keys: Vec<_> = dir.state().as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        ["failures", "last_failure_at_us", "locked_until_us", "v"]
    );
}
