#![allow(dead_code)]

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use data_encoding::{BASE64URL_NOPAD, HEXLOWER};
use ed25519_dalek::{Signer, SigningKey};
use host_core::clock::Clock;
use host_core::update::manifest::{signing_bytes, TrustedKeys, VerifiedManifest};
use host_core::update::prepare::{Downloader, ReleaseSource};
use host_core::update::types::{Arch, Channel, HostOs};
use ring::digest::{digest, SHA256};
use serde_json::{json, Map, Value};
use tokio::sync::watch;
use tokio::time::Instant;

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct TestDir(pub PathBuf);

impl TestDir {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rs-h16b-update-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceReply {
    Bytes(Vec<u8>),
    None,
    Error(String),
}

pub struct FakeSource {
    replies: Mutex<VecDeque<SourceReply>>,
    default: Mutex<SourceReply>,
    calls: AtomicUsize,
    from: Mutex<Vec<String>>,
}

impl FakeSource {
    pub fn new(reply: SourceReply) -> Self {
        Self {
            replies: Mutex::new(VecDeque::new()),
            default: Mutex::new(reply),
            calls: AtomicUsize::new(0),
            from: Mutex::new(Vec::new()),
        }
    }

    pub fn bytes(raw: Vec<u8>) -> Self {
        Self::new(SourceReply::Bytes(raw))
    }

    pub fn queue(&self, reply: SourceReply) {
        self.replies.lock().unwrap().push_back(reply);
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn froms(&self) -> Vec<String> {
        self.from.lock().unwrap().clone()
    }
}

impl Default for FakeSource {
    fn default() -> Self {
        Self::new(SourceReply::None)
    }
}

#[async_trait]
impl ReleaseSource for FakeSource {
    async fn fetch(
        &self,
        _channel: Channel,
        _os: HostOs,
        _arch: Arch,
        from: &str,
    ) -> Result<Option<Vec<u8>>, String> {
        tokio::task::yield_now().await;
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.from.lock().unwrap().push(from.to_owned());
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| self.default.lock().unwrap().clone());
        match reply {
            SourceReply::Bytes(bytes) => Ok(Some(bytes)),
            SourceReply::None => Ok(None),
            SourceReply::Error(error) => Err(error),
        }
    }
}

pub struct FakeDownloader {
    contents: Mutex<VecDeque<Vec<u8>>>,
    failures: AtomicUsize,
    calls: AtomicUsize,
    max_bytes: Mutex<Vec<u64>>,
}

impl FakeDownloader {
    pub fn new(contents: Vec<u8>) -> Self {
        Self {
            contents: Mutex::new(VecDeque::from([contents])),
            failures: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            max_bytes: Mutex::new(Vec::new()),
        }
    }

    /// Make the next `count` download attempts fail.
    pub fn fail_first(&self, count: usize) {
        self.failures.store(count, Ordering::SeqCst);
    }

    pub fn push_contents(&self, contents: Vec<u8>) {
        self.contents.lock().unwrap().push_back(contents);
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn max_bytes(&self) -> Vec<u64> {
        self.max_bytes.lock().unwrap().clone()
    }
}

#[async_trait]
impl Downloader for FakeDownloader {
    async fn download(&self, _url: &str, dest: &Path, max_bytes: u64) -> Result<(), String> {
        tokio::task::yield_now().await;
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.max_bytes.lock().unwrap().push(max_bytes);

        let remaining_failures = self.failures.load(Ordering::SeqCst);
        if remaining_failures > 0 {
            self.failures
                .store(remaining_failures - 1, Ordering::SeqCst);
            return Err("simulated download failure".to_owned());
        }

        let contents = self
            .contents
            .lock()
            .unwrap()
            .front()
            .cloned()
            .unwrap_or_default();
        if contents.len() as u64 > max_bytes {
            return Err("download exceeded max_bytes".to_owned());
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(dest, contents).map_err(|error| error.to_string())
    }
}

pub struct FakeVerifier {
    error: Mutex<Option<String>>,
    calls: AtomicUsize,
}

impl FakeVerifier {
    pub fn new() -> Self {
        Self {
            error: Mutex::new(None),
            calls: AtomicUsize::new(0),
        }
    }

    pub fn fail(&self, message: impl Into<String>) {
        *self.error.lock().unwrap() = Some(message.into());
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Default for FakeVerifier {
    fn default() -> Self {
        Self::new()
    }
}

impl host_core::update::prepare::PackageVerifier for FakeVerifier {
    fn verify_platform(&self, _package: &Path) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.error.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[derive(Debug)]
pub struct TestClock {
    started: Instant,
    base_micros: u64,
}

impl TestClock {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            base_micros: 1_792_465_134_000_000,
        }
    }
}

impl Default for TestClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for TestClock {
    fn now_micros(&self) -> u64 {
        self.base_micros + self.started.elapsed().as_micros() as u64
    }

    fn sleep(&self, duration: Duration) -> host_core::clock::BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(duration))
    }
}

pub fn seed_bytes(seed: [u8; 32]) -> SigningKey {
    SigningKey::from_bytes(&seed)
}

pub fn public_key(seed: [u8; 32]) -> String {
    BASE64URL_NOPAD.encode(seed_bytes(seed).verifying_key().as_bytes())
}

pub fn trusted(seed: [u8; 32]) -> TrustedKeys {
    let key = public_key(seed);
    TrustedKeys::from_base64url(&[key.as_str()]).expect("valid trusted key")
}

pub fn empty_trusted() -> TrustedKeys {
    TrustedKeys::from_base64url(&[]).expect("empty trusted keys")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    HEXLOWER.encode(digest(&SHA256, bytes).as_ref())
}

/// Build a signed manifest. Overrides can replace or add any JSON member;
/// `url` is overridden before signing, exactly like a genuine release.
pub fn signed_manifest(
    seed: [u8; 32],
    version: &str,
    package_bytes: impl AsRef<[u8]>,
    overrides: Value,
) -> Vec<u8> {
    let package_bytes = package_bytes.as_ref();
    let overrides = overrides.as_object().cloned().unwrap_or_else(Map::new);
    let mut fields = json!({
        "channel": "stable",
        "os": "macos",
        "arch": "universal",
        "version": version,
        "url": format!("https://dl.example.com/RemoteHost-{version}.dmg"),
        "sha256": sha256_hex(package_bytes),
        "size": package_bytes.len(),
        "notes": "修复网络切换后的重连问题。",
        "published_at": 1_792_465_134_000_000_u64,
    });
    let unsigned = {
        let map = fields.as_object_mut().expect("manifest object");
        for (name, value) in overrides {
            map.insert(name, value);
        }
        serde_json::to_vec(&fields).expect("serialize manifest")
    };
    let signing = signing_bytes(&unsigned).expect("canonical manifest bytes");
    let signature = seed_bytes(seed).sign(&signing);
    fields.as_object_mut().expect("manifest object").insert(
        "signature".to_owned(),
        Value::String(BASE64URL_NOPAD.encode(&signature.to_bytes())),
    );
    serde_json::to_vec(&fields).expect("serialize signed manifest")
}

pub fn manifest_from(raw: &[u8], keys: &TrustedKeys) -> VerifiedManifest {
    host_core::update::manifest::verify_manifest(raw, keys).expect("verified manifest")
}

pub fn status_channel() -> (
    watch::Sender<host_core::update::status::UpdateStatus>,
    watch::Receiver<host_core::update::status::UpdateStatus>,
) {
    watch::channel(host_core::update::status::UpdateStatus::idle(
        Channel::Stable,
    ))
}

pub fn package_file(status_dir: &Path, version: &str) -> PathBuf {
    status_dir
        .join("download")
        .join(version)
        .join("package.dmg")
}

pub struct FakeInstaller {
    installs: Mutex<Vec<(PathBuf, PathBuf)>>,
    restarts: AtomicUsize,
    rollbacks: AtomicUsize,
    install_error: Mutex<Option<String>>,
    restart_error: Mutex<Option<String>>,
    rollback_error: Mutex<Option<String>>,
}

impl FakeInstaller {
    pub fn new() -> Self {
        Self {
            installs: Mutex::new(Vec::new()),
            restarts: AtomicUsize::new(0),
            rollbacks: AtomicUsize::new(0),
            install_error: Mutex::new(None),
            restart_error: Mutex::new(None),
            rollback_error: Mutex::new(None),
        }
    }

    pub fn fail_install(&self, error: impl Into<String>) {
        *self.install_error.lock().unwrap() = Some(error.into());
    }

    pub fn fail_restart(&self, error: impl Into<String>) {
        *self.restart_error.lock().unwrap() = Some(error.into());
    }

    pub fn fail_rollback(&self, error: impl Into<String>) {
        *self.rollback_error.lock().unwrap() = Some(error.into());
    }

    pub fn installs(&self) -> Vec<(PathBuf, PathBuf)> {
        self.installs.lock().unwrap().clone()
    }

    pub fn restart_count(&self) -> usize {
        self.restarts.load(Ordering::SeqCst)
    }

    pub fn rollback_count(&self) -> usize {
        self.rollbacks.load(Ordering::SeqCst)
    }
}

impl Default for FakeInstaller {
    fn default() -> Self {
        Self::new()
    }
}

impl host_core::update::install::Installer for FakeInstaller {
    fn install(&self, package: &Path, backup_dir: &Path) -> Result<(), String> {
        self.installs
            .lock()
            .unwrap()
            .push((package.to_owned(), backup_dir.to_owned()));
        self.install_error
            .lock()
            .unwrap()
            .clone()
            .map_or(Ok(()), Err)
    }

    fn restart(&self) -> Result<(), String> {
        self.restarts.fetch_add(1, Ordering::SeqCst);
        self.restart_error
            .lock()
            .unwrap()
            .clone()
            .map_or(Ok(()), Err)
    }

    fn rollback(&self, _backup_dir: &Path) -> Result<(), String> {
        self.rollbacks.fetch_add(1, Ordering::SeqCst);
        self.rollback_error
            .lock()
            .unwrap()
            .clone()
            .map_or(Ok(()), Err)
    }
}

pub struct FakeGate {
    active: AtomicUsize,
    idle: tokio::sync::Notify,
    close_calls: AtomicUsize,
}

impl FakeGate {
    pub fn new(active: usize) -> Self {
        Self {
            active: AtomicUsize::new(active),
            idle: tokio::sync::Notify::new(),
            close_calls: AtomicUsize::new(0),
        }
    }

    pub fn release(&self) {
        self.active.store(0, Ordering::SeqCst);
        self.idle.notify_waiters();
    }

    pub fn close_calls(&self) -> usize {
        self.close_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl host_core::update::install::SessionGate for FakeGate {
    fn active_sessions(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    async fn wait_idle(&self) {
        while self.active_sessions() > 0 {
            self.idle.notified().await;
        }
    }

    async fn close_all_for_update(&self) {
        self.close_calls.fetch_add(1, Ordering::SeqCst);
        self.release();
    }
}

pub struct FakeSelfCheck {
    result: Mutex<Result<(), String>>,
    calls: AtomicUsize,
    never_returns: std::sync::atomic::AtomicBool,
}

impl FakeSelfCheck {
    pub fn new() -> Self {
        Self {
            result: Mutex::new(Ok(())),
            calls: AtomicUsize::new(0),
            never_returns: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn fail(&self, reason: impl Into<String>) {
        *self.result.lock().unwrap() = Err(reason.into());
    }

    pub fn never_returns(&self) {
        self.never_returns.store(true, Ordering::SeqCst);
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Default for FakeSelfCheck {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl host_core::update::recovery::SelfCheck for FakeSelfCheck {
    async fn run(&self) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.never_returns.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        self.result.lock().unwrap().clone()
    }
}

pub struct RecordingInstaller {
    rollbacks: Mutex<Vec<PathBuf>>,
    restarts: AtomicUsize,
    rollback_error: Mutex<Option<String>>,
    restart_error: Mutex<Option<String>>,
}

impl RecordingInstaller {
    pub fn new() -> Self {
        Self {
            rollbacks: Mutex::new(Vec::new()),
            restarts: AtomicUsize::new(0),
            rollback_error: Mutex::new(None),
            restart_error: Mutex::new(None),
        }
    }

    pub fn fail_rollback(&self, error: impl Into<String>) {
        *self.rollback_error.lock().unwrap() = Some(error.into());
    }

    pub fn fail_restart(&self, error: impl Into<String>) {
        *self.restart_error.lock().unwrap() = Some(error.into());
    }

    pub fn rollbacks(&self) -> Vec<PathBuf> {
        self.rollbacks.lock().unwrap().clone()
    }

    pub fn restart_count(&self) -> usize {
        self.restarts.load(Ordering::SeqCst)
    }

    pub fn rollback_count(&self) -> usize {
        self.rollbacks.lock().unwrap().len()
    }
}

impl Default for RecordingInstaller {
    fn default() -> Self {
        Self::new()
    }
}

impl host_core::update::install::Installer for RecordingInstaller {
    fn install(&self, _package: &Path, _backup_dir: &Path) -> Result<(), String> {
        unreachable!("recovery tests do not install")
    }

    fn restart(&self) -> Result<(), String> {
        self.restarts.fetch_add(1, Ordering::SeqCst);
        self.restart_error
            .lock()
            .unwrap()
            .clone()
            .map_or(Ok(()), Err)
    }

    fn rollback(&self, backup_dir: &Path) -> Result<(), String> {
        self.rollbacks.lock().unwrap().push(backup_dir.to_owned());
        self.rollback_error
            .lock()
            .unwrap()
            .clone()
            .map_or(Ok(()), Err)
    }
}
