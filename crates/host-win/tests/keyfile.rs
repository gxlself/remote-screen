use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Barrier, Mutex,
};

use host_core::platform::{KeyStore, PlatformError, Result};
use host_win::keyfile::{
    decode_file, encode_file, validate_name, DirSecurer, FileKeyStore, NoopSecurer, Protector,
    DIR_SDDL, ENTROPY, FILE_SDDL, MAGIC, MAX_BLOB,
};
use host_win::paths::{WinPaths, DEFAULT_PROGRAM_DATA};
use zeroize::Zeroizing;

// Deliberately does not implement Debug: a store must not expose its protector.
struct XorProtector;

impl Protector for XorProtector {
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>> {
        Ok(plain.iter().map(|byte| byte ^ 0x5a).collect())
    }

    fn unprotect(&self, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        Ok(Zeroizing::new(
            blob.iter().map(|byte| byte ^ 0x5a).collect(),
        ))
    }
}

struct FailingProtector;

impl Protector for FailingProtector {
    fn protect(&self, _plain: &[u8]) -> Result<Vec<u8>> {
        Err(PlatformError::Platform("protect failed".to_owned()))
    }

    fn unprotect(&self, _blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        Err(PlatformError::Platform("unprotect failed".to_owned()))
    }
}

struct OversizedProtector;

impl Protector for OversizedProtector {
    fn protect(&self, _plain: &[u8]) -> Result<Vec<u8>> {
        Ok(vec![0; MAX_BLOB + 1])
    }

    fn unprotect(&self, _blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        Err(PlatformError::Unsupported)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SecureCall {
    Dir(PathBuf),
    File(PathBuf),
}

#[derive(Clone, Default)]
struct RecordingSecurer {
    calls: Arc<Mutex<Vec<SecureCall>>>,
}

impl DirSecurer for RecordingSecurer {
    fn secure_dir(&self, dir: &Path) -> Result<()> {
        self.calls
            .lock()
            .expect("recording mutex")
            .push(SecureCall::Dir(dir.to_owned()));
        Ok(())
    }

    fn secure_file(&self, file: &Path) -> Result<()> {
        // The entire encoded file must already be written before its ACL is set.
        decode_file(&fs::read(file)?)?;
        self.calls
            .lock()
            .expect("recording mutex")
            .push(SecureCall::File(file.to_owned()));
        Ok(())
    }
}

enum FailingSecurer {
    Dir,
    File,
}

impl DirSecurer for FailingSecurer {
    fn secure_dir(&self, _dir: &Path) -> Result<()> {
        match self {
            Self::Dir => Err(PlatformError::Platform("dir acl failed".to_owned())),
            Self::File => Ok(()),
        }
    }

    fn secure_file(&self, _file: &Path) -> Result<()> {
        Err(PlatformError::Platform("file acl failed".to_owned()))
    }
}

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let number = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!(
            "remote-host-win-keyfile-{}-{number}",
            std::process::id(),
        )))
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn store(&self) -> FileKeyStore<XorProtector, NoopSecurer> {
        FileKeyStore::new(self.0.clone(), XorProtector, NoopSecurer)
    }

    fn key(&self) -> PathBuf {
        self.0.join("host-key-v1.dpapi")
    }

    fn temp(&self) -> PathBuf {
        self.0.join("host-key-v1.dpapi.tmp")
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_platform_error<T>(result: Result<T>, expected: &str) {
    match result {
        Err(PlatformError::Platform(message)) => assert_eq!(message, expected),
        Err(error) => panic!("expected Platform error, got {error}"),
        Ok(_) => panic!("expected Platform error"),
    }
}

#[test]
fn default_paths_use_windows_separators_on_every_target() {
    let paths = WinPaths::from_program_data(None);
    assert_eq!(paths.data_dir, PathBuf::from(r"C:\ProgramData\RemoteHost"));
    assert_eq!(
        paths.keys_dir,
        PathBuf::from(r"C:\ProgramData\RemoteHost\keys")
    );
    assert_eq!(
        paths.logs_dir,
        PathBuf::from(r"C:\ProgramData\RemoteHost\logs")
    );
    assert_eq!(
        paths.config_file,
        PathBuf::from(r"C:\ProgramData\RemoteHost\config.toml")
    );
    assert_eq!(
        paths.db_file,
        PathBuf::from(r"C:\ProgramData\RemoteHost\host.db")
    );
    assert_eq!(
        paths.update_dir,
        PathBuf::from(r"C:\ProgramData\RemoteHost\update")
    );
}

#[test]
fn program_data_accepts_drive_and_unc_paths_and_removes_one_trailing_separator() {
    for (input, expected) in [
        (r"D:\PD\", r"D:\PD\RemoteHost"),
        (r"D:\PD", r"D:\PD\RemoteHost"),
        (r"D:\", r"D:\RemoteHost"),
        (r"d:\数据\", r"d:\数据\RemoteHost"),
        (r"\\server\share\", r"\\server\share\RemoteHost"),
        (r"\\server\share", r"\\server\share\RemoteHost"),
    ] {
        assert_eq!(
            WinPaths::from_program_data(Some(input)).data_dir,
            PathBuf::from(expected)
        );
    }
}

#[test]
fn program_data_rejects_empty_and_relative_paths() {
    let defaults = WinPaths::from_program_data(None);
    for input in ["", "relative", r"D:relative", r"\PD", "D:/PD", r"1:\PD"] {
        assert_eq!(WinPaths::from_program_data(Some(input)), defaults);
    }
}

#[test]
fn valid_names_match_identity_keys_and_ascii_boundaries() {
    for name in ["host-key-v1", "host-cert-v1", "a", "A0._-", "a.b"] {
        validate_name(name).expect("valid key name");
    }
    validate_name(&"a".repeat(64)).expect("64 characters is valid");
}

#[test]
fn invalid_names_are_rejected_before_any_file_operation() {
    let dir = TestDir::new();
    let store = dir.store();
    for name in [
        "",
        &"a".repeat(65),
        ".hidden",
        ".",
        "..",
        "a/b",
        r"a\b",
        "a:b",
        "键",
        "a\0b",
        "a b",
    ] {
        assert_platform_error(validate_name(name), "invalid key name");
        assert_platform_error(store.store(name, b"secret"), "invalid key name");
        assert_platform_error(store.load(name), "invalid key name");
        assert_platform_error(store.delete(name), "invalid key name");
    }
    assert!(!dir.path().exists());
}

#[test]
fn security_and_format_constants_match_contract() {
    assert_eq!(DEFAULT_PROGRAM_DATA, r"C:\ProgramData");
    assert_eq!(MAGIC, b"RSK1");
    assert_eq!(MAX_BLOB, 64 * 1024);
    assert_eq!(ENTROPY, b"ai.gxlself.remotehost.keystore.v1");
    assert_eq!(DIR_SDDL, "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
    assert_eq!(FILE_SDDL, "D:P(A;;FA;;;SY)(A;;FA;;;BA)");
}

#[test]
fn encoded_files_use_a_little_endian_length_and_round_trip_boundaries() {
    let bytes = encode_file(&[1, 2, 3]).expect("encode");
    assert_eq!(bytes, b"RSK1\x03\0\0\0\x01\x02\x03");
    assert_eq!(decode_file(&bytes).expect("decode"), &[1, 2, 3]);
    for blob in [Vec::new(), vec![1; MAX_BLOB]] {
        let bytes = encode_file(&blob).expect("boundary encode");
        assert_eq!(decode_file(&bytes).expect("boundary decode"), blob);
    }
    assert_platform_error(encode_file(&[0; 65_537]), "key too large");
}

#[test]
fn decoder_rejects_short_headers_bad_magic_lengths_and_trailing_bytes() {
    let valid = encode_file(&[1, 2, 3]).expect("encode");
    for end in 0..8 {
        assert_platform_error(decode_file(&valid[..end]), "corrupt key file");
    }
    let mut corruptions = Vec::new();
    let mut bad_magic = valid.clone();
    bad_magic[..4].copy_from_slice(b"XXXX");
    corruptions.push(bad_magic);
    corruptions.push(valid[..valid.len() - 1].to_vec());
    let mut extra = valid;
    extra.push(0);
    corruptions.push(extra);
    corruptions.push([MAGIC.as_slice(), &70_000u32.to_le_bytes()].concat());
    let mut oversized = [MAGIC.as_slice(), &65_537u32.to_le_bytes()].concat();
    oversized.extend_from_slice(&[0; 65_537]);
    corruptions.push(oversized);
    for bytes in corruptions {
        assert_platform_error(decode_file(&bytes), "corrupt key file");
    }
}

#[test]
fn store_encrypts_then_secures_the_temporary_file_and_loads_plaintext() {
    let dir = TestDir::new();
    let securer = RecordingSecurer::default();
    let calls = Arc::clone(&securer.calls);
    let store = FileKeyStore::new(dir.0.clone(), XorProtector, securer);

    store.store("host-key-v1", b"secret").expect("store");
    let bytes = fs::read(dir.key()).expect("key file");
    assert!(bytes.starts_with(MAGIC));
    assert!(!bytes.windows(6).any(|window| window == b"secret"));
    assert_eq!(
        store.load("host-key-v1").expect("load"),
        Some(b"secret".to_vec())
    );
    assert_eq!(
        *calls.lock().expect("recording mutex"),
        vec![SecureCall::Dir(dir.0.clone()), SecureCall::File(dir.temp()),]
    );
    assert!(!dir.temp().exists());
    assert_eq!(fs::read_dir(dir.path()).expect("key directory").count(), 1);
}

#[test]
fn overwrites_replace_the_old_value_without_leaving_a_temporary_file() {
    let dir = TestDir::new();
    let store = dir.store();
    store
        .store("host-key-v1", b"long original secret")
        .expect("initial store");
    store.store("host-key-v1", b"new").expect("overwrite");
    assert_eq!(
        store.load("host-key-v1").expect("load overwrite"),
        Some(b"new".to_vec())
    );
    assert!(!dir.temp().exists());
    store.store("host-key-v1", b"").expect("empty overwrite");
    assert_eq!(store.load("host-key-v1").expect("empty load"), Some(vec![]));
}

#[test]
fn missing_load_and_delete_are_idempotent_and_do_not_create_the_directory() {
    let dir = TestDir::new();
    let store = dir.store();
    assert_eq!(store.load("missing").expect("missing load"), None);
    store.delete("missing").expect("missing delete");
    assert!(!dir.path().exists());
    store.store("host-key-v1", b"secret").expect("store");
    store.delete("host-key-v1").expect("delete");
    store.delete("host-key-v1").expect("repeat delete");
    assert_eq!(store.load("host-key-v1").expect("deleted load"), None);
}

#[test]
fn corrupted_files_return_platform_errors_from_load() {
    let dir = TestDir::new();
    let store = dir.store();
    store.store("host-key-v1", b"secret").expect("store");
    let valid = fs::read(dir.key()).expect("read");
    let mut oversized = [MAGIC.as_slice(), &70_000u32.to_le_bytes()].concat();
    oversized.extend_from_slice(&[0; 70_000]);
    for bytes in [
        b"XXXX\0\0\0\0".to_vec(),
        valid[..valid.len() - 1].to_vec(),
        oversized,
    ] {
        fs::write(dir.key(), bytes).expect("corrupt file");
        assert_platform_error(store.load("host-key-v1"), "corrupt key file");
    }
}

#[test]
fn protection_failure_leaves_no_key_or_temporary_file() {
    let dir = TestDir::new();
    let store = FileKeyStore::new(dir.0.clone(), FailingProtector, NoopSecurer);
    assert_platform_error(store.store("host-key-v1", b"secret"), "protect failed");
    assert_eq!(fs::read_dir(dir.path()).expect("directory").count(), 0);
}

#[test]
fn unprotect_failure_is_propagated_without_modifying_the_file() {
    let dir = TestDir::new();
    dir.store().store("host-key-v1", b"secret").expect("store");
    let before = fs::read(dir.key()).expect("file before");
    let store = FileKeyStore::new(dir.0.clone(), FailingProtector, NoopSecurer);
    assert_platform_error(store.load("host-key-v1"), "unprotect failed");
    assert_eq!(fs::read(dir.key()).expect("file after"), before);
}

#[test]
fn acl_failures_preserve_the_previous_key_and_clean_up_temporary_files() {
    let dir = TestDir::new();
    let store = dir.store();
    store.store("host-key-v1", b"original").expect("store");
    for (securer, expected) in [
        (FailingSecurer::Dir, "dir acl failed"),
        (FailingSecurer::File, "file acl failed"),
    ] {
        fs::write(dir.temp(), b"stale").expect("stale temporary file");
        let failing = FileKeyStore::new(dir.0.clone(), XorProtector, securer);
        assert_platform_error(failing.store("host-key-v1", b"replacement"), expected);
        assert!(!dir.temp().exists());
        assert_eq!(
            store.load("host-key-v1").expect("original load"),
            Some(b"original".to_vec())
        );
    }
}

#[test]
fn protector_and_encoding_failures_also_clean_stale_temporary_files() {
    let dir = TestDir::new();
    let store = dir.store();
    store.store("host-key-v1", b"original").expect("store");
    fs::write(dir.temp(), b"stale").expect("stale temporary file");
    let failing = FileKeyStore::new(dir.0.clone(), FailingProtector, NoopSecurer);
    assert_platform_error(
        failing.store("host-key-v1", b"replacement"),
        "protect failed",
    );
    assert!(!dir.temp().exists());

    fs::write(dir.temp(), b"stale").expect("stale temporary file");
    let oversized = FileKeyStore::new(dir.0.clone(), OversizedProtector, NoopSecurer);
    assert_platform_error(
        oversized.store("host-key-v1", b"replacement"),
        "key too large",
    );
    assert!(!dir.temp().exists());
    assert_eq!(
        store.load("host-key-v1").expect("original load"),
        Some(b"original".to_vec())
    );
}

#[test]
fn rename_failure_cleans_the_temporary_file_and_preserves_the_destination() {
    let dir = TestDir::new();
    fs::create_dir_all(dir.key()).expect("blocking destination directory");
    let sentinel = dir.key().join("keep");
    fs::write(&sentinel, b"keep").expect("sentinel");
    assert!(matches!(
        dir.store().store("host-key-v1", b"secret"),
        Err(PlatformError::Io(_))
    ));
    assert!(!dir.temp().exists());
    assert_eq!(fs::read(sentinel).expect("preserved sentinel"), b"keep");
}

#[test]
fn write_open_failures_preserve_the_original_key() {
    let dir = TestDir::new();
    let store = dir.store();
    store.store("host-key-v1", b"original").expect("store");
    fs::create_dir(dir.temp()).expect("block temporary path with directory");
    assert!(matches!(
        store.store("host-key-v1", b"replacement"),
        Err(PlatformError::Io(_))
    ));
    assert_eq!(
        store.load("host-key-v1").expect("original load"),
        Some(b"original".to_vec())
    );
}

#[test]
fn parallel_writes_on_one_store_leave_a_complete_value() {
    let dir = TestDir::new();
    let store = Arc::new(dir.store());
    let start = Arc::new(Barrier::new(4));
    let workers: Vec<_> = (0..4u8)
        .map(|value| {
            let store = Arc::clone(&store);
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                store.store("host-key-v1", &[value; 32])
            })
        })
        .collect();
    for worker in workers {
        worker
            .join()
            .expect("writer thread")
            .expect("parallel store");
    }
    let value = store.load("host-key-v1").expect("load").expect("value");
    assert!((0..4).any(|byte| value == [byte; 32]));
    assert!(!dir.temp().exists());
}

#[test]
fn debug_omits_the_protector_and_securer() {
    let dir = TestDir::new();
    let store = dir.store();
    let debug = format!("{store:?}");
    assert!(debug.starts_with("FileKeyStore"));
    assert!(!debug.contains("protector"));
    assert!(!debug.contains("securer"));
}
