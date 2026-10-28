mod support;

use std::{
    collections::HashMap,
    ffi::{c_char, c_void, CStr, CString},
    ptr, slice,
    sync::{
        atomic::{AtomicI32, AtomicUsize, Ordering},
        Arc, LazyLock, Mutex,
    },
};

use rs_mobile::rscore::adapters::{
    driver::Credentials,
    dto::SavedCredentialInfo,
    ffi_secrets::rs_register_secret_store,
    secrets::{
        can_save, forget, load, registered, save, saved_info, set_registered, MemSecretStore,
        SecretError, SecretStore, MAX_KEY_BYTES, MAX_VALUE_BYTES, SAVED_INDEX_PREFIX,
    },
};
use serde_json::{json, Value};
use zeroize::Zeroizing;

static LOCK: Mutex<()> = Mutex::new(());
const KEY: &str = "vnc:host-1";
const INDEX_KEY: &str = "adapter.saved.vnc:host-1";

fn credentials(user: Option<&str>, password: &str, domain: Option<&str>) -> Credentials {
    Credentials {
        user: user.map(str::to_owned),
        password: Zeroizing::new(password.to_owned()),
        domain: domain.map(str::to_owned),
    }
}

fn assert_credentials(actual: &Credentials, expected: &Credentials) {
    assert_eq!(actual.user, expected.user);
    assert_eq!(*actual.password, *expected.password);
    assert_eq!(actual.domain, expected.domain);
}

#[test]
fn not_registered() {
    let _lock = LOCK.lock().unwrap();
    set_registered(None);
    let core = support::test_core();
    assert!(!can_save());
    assert!(registered().is_none());
    assert_eq!(
        save(&core, KEY, &credentials(Some("alice"), "pw 1", None)),
        Err(SecretError::NotRegistered)
    );
    assert_eq!(load(KEY).err(), Some(SecretError::NotRegistered));
    assert_eq!(saved_info(&core, KEY), SavedCredentialInfo::default());
    assert_eq!(forget(&core, KEY), Ok(()));
}

#[test]
fn save_load_roundtrip() {
    let _lock = LOCK.lock().unwrap();
    set_registered(None);
    let core = support::test_core();
    let store = MemSecretStore::new();
    set_registered(Some(store.clone()));
    assert!(can_save());
    let creds = credentials(Some("alice"), "pw 1", None);
    save(&core, KEY, &creds).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&store.raw(KEY).unwrap()).unwrap(),
        json!({"v": 1, "user": "alice", "password": "pw 1"})
    );
    assert_credentials(&load(KEY).unwrap().unwrap(), &creds);
    assert_eq!(
        saved_info(&core, KEY),
        SavedCredentialInfo {
            saved: true,
            user: Some("alice".into()),
            domain: None,
        }
    );
    assert_eq!(store.len(), 1);
    assert_eq!(store.put_count(), 1);
    assert_eq!(store.get_count(), 1);
    assert_eq!(store.delete_count(), 0);
}

#[test]
fn index_has_no_password() {
    let _lock = LOCK.lock().unwrap();
    set_registered(None);
    let core = support::test_core();
    set_registered(Some(MemSecretStore::new()));
    save(&core, KEY, &credentials(Some("alice"), "pw 1", None)).unwrap();
    let index = core.store().kv_get(INDEX_KEY).unwrap().unwrap();
    assert!(!index.contains("pw 1"));
    assert!(!index.contains("password"));
    assert_eq!(
        serde_json::from_str::<Value>(&index).unwrap(),
        json!({"user": "alice"})
    );
}

#[test]
fn forget_removes_both() {
    let _lock = LOCK.lock().unwrap();
    set_registered(None);
    let core = support::test_core();
    let store = MemSecretStore::new();
    set_registered(Some(store.clone()));
    save(&core, KEY, &credentials(Some("alice"), "pw 1", None)).unwrap();
    forget(&core, KEY).unwrap();
    assert!(store.raw(KEY).is_none());
    assert!(core.store().kv_get(INDEX_KEY).unwrap().is_none());
    assert_eq!(saved_info(&core, KEY), SavedCredentialInfo::default());
    assert!(load(KEY).unwrap().is_none());
    assert!(store.is_empty());
    forget(&core, KEY).unwrap();
    assert_eq!(store.delete_count(), 2);
}

#[test]
fn saved_info_without_store() {
    let _lock = LOCK.lock().unwrap();
    set_registered(None);
    let core = support::test_core();
    let store = MemSecretStore::new();
    set_registered(Some(store.clone()));
    save(&core, KEY, &credentials(Some("alice"), "pw 1", None)).unwrap();
    set_registered(None);
    assert!(!can_save());
    assert!(core.store().kv_get(INDEX_KEY).unwrap().is_some());
    assert_eq!(saved_info(&core, KEY), SavedCredentialInfo::default());
    forget(&core, KEY).unwrap();
    assert!(core.store().kv_get(INDEX_KEY).unwrap().is_none());
    assert!(store.raw(KEY).is_some());
    assert_eq!(store.delete_count(), 0);
}

#[test]
fn limits() {
    let _lock = LOCK.lock().unwrap();
    set_registered(None);
    let core = support::test_core();
    let store = MemSecretStore::new();
    let creds = credentials(Some("alice"), "pw 1", None);
    // Validation precedes both registry lookup and every backend operation.
    for registered_store in [None, Some(store.clone() as Arc<dyn SecretStore>)] {
        set_registered(registered_store);
        for key in [
            String::new(),
            "x".repeat(MAX_KEY_BYTES + 1),
            "vnc:host\0suffix".to_owned(),
            "é".repeat(151),
        ] {
            assert_eq!(save(&core, &key, &creds), Err(SecretError::BadKey));
            assert_eq!(load(&key).err(), Some(SecretError::BadKey));
            assert_eq!(forget(&core, &key), Err(SecretError::BadKey));
            assert_eq!(saved_info(&core, &key), SavedCredentialInfo::default());
            assert_eq!(store.get(&key).err(), Some(SecretError::BadKey));
            assert_eq!(store.put(&key, b"x"), Err(SecretError::BadKey));
            assert_eq!(store.delete(&key), Err(SecretError::BadKey));
            assert!(store.raw(&key).is_none());
        }
    }
    for password in ["x".repeat(5000), "\n".repeat(3000)] {
        assert_eq!(
            save(&core, KEY, &credentials(None, &password, None)),
            Err(SecretError::TooLarge)
        );
    }
    assert_eq!(
        store.put(KEY, &vec![0; MAX_VALUE_BYTES + 1]),
        Err(SecretError::TooLarge)
    );
    assert!(store.is_empty());
    assert!(core.store().kv_get(INDEX_KEY).unwrap().is_none());
    assert_eq!(store.get_count(), 0);
    assert_eq!(store.put_count(), 0);
    assert_eq!(store.delete_count(), 0);
}

#[test]
fn exact_byte_limits() {
    let _lock = LOCK.lock().unwrap();
    set_registered(None);
    let core = support::test_core();
    let store = MemSecretStore::new();
    set_registered(Some(store.clone()));
    let key = "é".repeat(MAX_KEY_BYTES / 2);
    let password = "x".repeat(MAX_VALUE_BYTES - r#"{"v":1,"password":""}"#.len());
    let creds = credentials(None, &password, None);
    save(&core, &key, &creds).unwrap();
    assert_eq!(store.raw(&key).unwrap().len(), MAX_VALUE_BYTES);
    assert_credentials(&load(&key).unwrap().unwrap(), &creds);
    assert_eq!(
        save(&core, &key, &credentials(None, &(password + "x"), None)),
        Err(SecretError::TooLarge)
    );
    assert_eq!(store.put_count(), 1);
    assert_credentials(&load(&key).unwrap().unwrap(), &creds);
    forget(&core, &key).unwrap();
    assert!(store.is_empty());
}

#[test]
fn optional_fields_and_replacement() {
    let _lock = LOCK.lock().unwrap();
    set_registered(None);
    let core = support::test_core();
    let store = MemSecretStore::new();
    set_registered(Some(store.clone()));
    for creds in [
        credentials(Some("álîce"), "pw \\\"雪", Some("EXAMPLE")),
        credentials(None, "", Some("")),
        credentials(None, "replacement", None),
    ] {
        save(&core, KEY, &creds).unwrap();
        assert_credentials(&load(KEY).unwrap().unwrap(), &creds);
        assert_eq!(
            saved_info(&core, KEY),
            SavedCredentialInfo {
                saved: true,
                user: creds.user.clone(),
                domain: creds.domain.clone(),
            }
        );
    }
    assert_eq!(store.len(), 1);
    assert_eq!(store.put_count(), 3);
    assert_eq!(
        serde_json::from_slice::<Value>(&store.raw(KEY).unwrap()).unwrap(),
        json!({"v": 1, "password": "replacement"})
    );
    assert_eq!(core.store().kv_get(INDEX_KEY).unwrap().unwrap(), "{}");
}

#[test]
fn malformed_value() {
    let _lock = LOCK.lock().unwrap();
    set_registered(None);
    let store = MemSecretStore::new();
    set_registered(Some(store.clone()));
    for value in [
        b"not json".as_slice(),
        br#"{"v":2,"password":"x"}"#,
        br#"{"v":1}"#,
        br#"{"password":"x"}"#,
        br#"{"v":1,"password":null}"#,
        br#"{"v":1,"password":"x","domain":5}"#,
        br#"{"v":1,"password":"x","password":"y"}"#,
        br#"{"v":1,"password":"x"} trailing"#,
        b"",
        b"\xff",
    ] {
        store.put(KEY, value).unwrap();
        assert_eq!(load(KEY).err(), Some(SecretError::Malformed));
        assert_eq!(store.raw(KEY).unwrap(), value);
    }
    assert_eq!(store.delete_count(), 0);
}

#[test]
fn saved_info_only_reads_index() {
    let _lock = LOCK.lock().unwrap();
    set_registered(None);
    let core = support::test_core();
    let store = MemSecretStore::new();
    set_registered(Some(store.clone()));
    save(&core, KEY, &credentials(Some("alice"), "pw 1", None)).unwrap();
    store.delete(KEY).unwrap();
    assert!(saved_info(&core, KEY).saved);
    assert_eq!(store.get_count(), 0);
    core.store().kv_set(INDEX_KEY, "not json").unwrap();
    assert_eq!(saved_info(&core, KEY), SavedCredentialInfo::default());
    assert_eq!(store.get_count(), 0);
    core.store().kv_delete(INDEX_KEY).unwrap();
    assert_eq!(saved_info(&core, KEY), SavedCredentialInfo::default());
}

#[test]
fn index_write_failure_keeps_secret() {
    let _lock = LOCK.lock().unwrap();
    set_registered(None);
    let core = support::test_core();
    let store = MemSecretStore::new();
    set_registered(Some(store.clone()));
    let db = rusqlite::Connection::open(core.data_dir().join("rs.db")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER reject_saved_index BEFORE INSERT ON kv
         WHEN NEW.key LIKE 'adapter.saved.%'
         BEGIN SELECT RAISE(FAIL, 'index write failed'); END;",
    )
    .unwrap();
    let creds = credentials(Some("alice"), "pw 1", None);
    save(&core, KEY, &creds).unwrap();
    assert_credentials(&load(KEY).unwrap().unwrap(), &creds);
    assert!(core.store().kv_get(INDEX_KEY).unwrap().is_none());
    assert_eq!(saved_info(&core, KEY), SavedCredentialInfo::default());
    assert_eq!(store.delete_count(), 0);
    db.execute_batch("DROP TRIGGER reject_saved_index;")
        .unwrap();
    save(&core, KEY, &creds).unwrap();
    assert!(saved_info(&core, KEY).saved);
}

static NATIVE_VALUES: LazyLock<Mutex<HashMap<CString, Vec<u8>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct NativeState {
    capacity: AtomicUsize,
    reported_len: AtomicUsize,
    gets: AtomicUsize,
    puts: AtomicUsize,
    deletes: AtomicUsize,
    get_rc: AtomicI32,
    put_rc: AtomicI32,
    del_rc: AtomicI32,
}

static NATIVE: NativeState = NativeState {
    capacity: AtomicUsize::new(0),
    reported_len: AtomicUsize::new(0),
    gets: AtomicUsize::new(0),
    puts: AtomicUsize::new(0),
    deletes: AtomicUsize::new(0),
    get_rc: AtomicI32::new(0),
    put_rc: AtomicI32::new(0),
    del_rc: AtomicI32::new(0),
};

fn reset_native() {
    NATIVE_VALUES.lock().unwrap().clear();
    for count in [
        &NATIVE.capacity,
        &NATIVE.reported_len,
        &NATIVE.gets,
        &NATIVE.puts,
        &NATIVE.deletes,
    ] {
        count.store(0, Ordering::Relaxed);
    }
    for code in [&NATIVE.get_rc, &NATIVE.put_rc, &NATIVE.del_rc] {
        code.store(0, Ordering::Relaxed);
    }
}

fn native_ctx() -> *mut c_void {
    ptr::from_ref(&NATIVE).cast_mut().cast()
}

extern "C" fn native_get(
    ctx: *mut c_void,
    key: *const c_char,
    buf: *mut u8,
    len: *mut usize,
) -> i32 {
    // SAFETY: Registration supplies the static NativeState. CallbackStore gives
    // a NUL-terminated key and a writable length with the output capacity.
    let (state, key, capacity) =
        unsafe { (&*ctx.cast::<NativeState>(), CStr::from_ptr(key), *len) };
    state.gets.fetch_add(1, Ordering::Relaxed);
    state.capacity.store(capacity, Ordering::Relaxed);
    let rc = state.get_rc.load(Ordering::Relaxed);
    if rc != 0 {
        return rc;
    }
    let reported_len = state.reported_len.load(Ordering::Relaxed);
    if reported_len != 0 {
        // SAFETY: Only report an invalid length; never overrun the actual buffer.
        unsafe { *len = reported_len };
        return 0;
    }
    let values = NATIVE_VALUES
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(value) = values.get(key) else {
        return 1;
    };
    if value.len() > capacity {
        return 2;
    }
    // SAFETY: buf has capacity bytes, the checked value fits, and these allocations
    // do not overlap. len is writable for the duration of the callback.
    unsafe {
        ptr::copy_nonoverlapping(value.as_ptr(), buf, value.len());
        *len = value.len();
    }
    0
}

extern "C" fn native_put(
    ctx: *mut c_void,
    key: *const c_char,
    value: *const u8,
    len: usize,
) -> i32 {
    // SAFETY: The static context and the NUL-terminated key are valid throughout
    // this call; CallbackStore supplies exactly len readable value bytes.
    let (state, key, value) = unsafe {
        (
            &*ctx.cast::<NativeState>(),
            CStr::from_ptr(key),
            slice::from_raw_parts(value, len),
        )
    };
    state.puts.fetch_add(1, Ordering::Relaxed);
    let rc = state.put_rc.load(Ordering::Relaxed);
    if rc != 0 {
        return rc;
    }
    NATIVE_VALUES
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(key.to_owned(), value.to_vec());
    0
}

extern "C" fn native_del(ctx: *mut c_void, key: *const c_char) -> i32 {
    // SAFETY: The context is static and CallbackStore supplies a NUL-terminated key.
    let (state, key) = unsafe { (&*ctx.cast::<NativeState>(), CStr::from_ptr(key)) };
    state.deletes.fetch_add(1, Ordering::Relaxed);
    let rc = state.del_rc.load(Ordering::Relaxed);
    if rc != 0 {
        return rc;
    }
    NATIVE_VALUES
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(key);
    0
}

fn register_native() {
    // SAFETY: The context, map, and callbacks live for the process lifetime;
    // the callbacks synchronize their state and respect the supplied buffers.
    unsafe {
        rs_register_secret_store(
            native_ctx(),
            Some(native_get),
            Some(native_put),
            Some(native_del),
        );
    }
}

#[test]
fn ffi_callbacks() {
    let _lock = LOCK.lock().unwrap();
    set_registered(None);
    reset_native();
    let core = support::test_core();
    register_native();
    assert!(can_save());
    let creds = credentials(Some("alice"), "pw 1", Some("EXAMPLE"));
    save(&core, KEY, &creds).unwrap();
    assert_credentials(&load(KEY).unwrap().unwrap(), &creds);
    assert_eq!(NATIVE.capacity.load(Ordering::Relaxed), MAX_VALUE_BYTES);
    assert_eq!(NATIVE.puts.load(Ordering::Relaxed), 1);
    assert_eq!(NATIVE.gets.load(Ordering::Relaxed), 1);
    forget(&core, KEY).unwrap();
    forget(&core, KEY).unwrap();
    assert!(load(KEY).unwrap().is_none());
    assert!(NATIVE_VALUES.lock().unwrap().is_empty());
    assert!(core.store().kv_get(INDEX_KEY).unwrap().is_none());
}

#[test]
fn ffi_return_codes() {
    let _lock = LOCK.lock().unwrap();
    set_registered(None);
    reset_native();
    let core = support::test_core();
    register_native();
    NATIVE.get_rc.store(7, Ordering::Relaxed);
    assert_eq!(load(KEY).err(), Some(SecretError::Native(7)));
    NATIVE.get_rc.store(0, Ordering::Relaxed);
    NATIVE.reported_len.store(5000, Ordering::Relaxed);
    assert_eq!(load(KEY).err(), Some(SecretError::Native(0)));
    NATIVE.get_rc.store(1, Ordering::Relaxed);
    assert!(load(KEY).unwrap().is_none());
    NATIVE.put_rc.store(2, Ordering::Relaxed);
    assert_eq!(
        save(&core, KEY, &credentials(Some("alice"), "pw 1", None)),
        Err(SecretError::Native(2))
    );
    assert!(core.store().kv_get(INDEX_KEY).unwrap().is_none());
    assert!(NATIVE_VALUES.lock().unwrap().is_empty());
    NATIVE.put_rc.store(0, Ordering::Relaxed);
    save(&core, KEY, &credentials(Some("alice"), "pw 1", None)).unwrap();
    NATIVE.del_rc.store(3, Ordering::Relaxed);
    assert_eq!(forget(&core, KEY), Err(SecretError::Native(3)));
    assert!(saved_info(&core, KEY).saved);
    assert!(!NATIVE_VALUES.lock().unwrap().is_empty());
}

#[test]
fn ffi_limits_do_not_call_native() {
    let _lock = LOCK.lock().unwrap();
    set_registered(None);
    reset_native();
    register_native();
    let store = registered().unwrap();
    for key in [String::new(), "x".repeat(301), "vnc:\0host-1".into()] {
        assert_eq!(store.get(&key).err(), Some(SecretError::BadKey));
        assert_eq!(store.put(&key, b"x"), Err(SecretError::BadKey));
        assert_eq!(store.delete(&key), Err(SecretError::BadKey));
    }
    assert_eq!(
        store.put(KEY, &vec![0; MAX_VALUE_BYTES + 1]),
        Err(SecretError::TooLarge)
    );
    assert_eq!(NATIVE.gets.load(Ordering::Relaxed), 0);
    assert_eq!(NATIVE.puts.load(Ordering::Relaxed), 0);
    assert_eq!(NATIVE.deletes.load(Ordering::Relaxed), 0);
    // Exact limits and a zero-length success are legal at the byte-store layer.
    let key = "é".repeat(MAX_KEY_BYTES / 2);
    let value = vec![b'x'; MAX_VALUE_BYTES];
    store.put(&key, &value).unwrap();
    assert_eq!(*store.get(&key).unwrap().unwrap(), value);
    store.put(KEY, b"").unwrap();
    assert!(store.get(KEY).unwrap().unwrap().is_empty());
}

#[test]
fn ffi_null_ignored() {
    let _lock = LOCK.lock().unwrap();
    set_registered(None);
    reset_native();
    let core = support::test_core();
    register_native();
    let previous = registered().unwrap();
    let creds = credentials(Some("alice"), "pw 1", None);
    save(&core, KEY, &creds).unwrap();
    for missing in 0..3 {
        // SAFETY: All callbacks and the context are static; each registration is
        // deliberately missing one callback and must preserve the current store.
        unsafe {
            rs_register_secret_store(
                native_ctx(),
                (missing != 0).then_some(native_get),
                (missing != 1).then_some(native_put),
                (missing != 2).then_some(native_del),
            );
        }
        assert!(Arc::ptr_eq(&previous, &registered().unwrap()));
        assert_credentials(&load(KEY).unwrap().unwrap(), &creds);
    }
    set_registered(None);
    // SAFETY: Invalid registration must also leave an empty registry untouched.
    unsafe { rs_register_secret_store(ptr::null_mut(), None, None, None) };
    assert!(!can_save());
}

struct PanickingStore;

impl SecretStore for PanickingStore {
    fn get(&self, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        panic!("test get panic")
    }

    fn put(&self, _: &str, _: &[u8]) -> Result<(), SecretError> {
        panic!("test put panic")
    }

    fn delete(&self, _: &str) -> Result<(), SecretError> {
        panic!("test delete panic")
    }
}

#[test]
fn panic_contained() {
    let _lock = LOCK.lock().unwrap();
    set_registered(None);
    let core = support::test_core();
    set_registered(Some(Arc::new(PanickingStore)));
    assert_eq!(load(KEY).err(), Some(SecretError::Native(-1)));
    assert_eq!(
        save(&core, KEY, &credentials(None, "pw 1", None)),
        Err(SecretError::Native(-1))
    );
    assert!(core.store().kv_get(INDEX_KEY).unwrap().is_none());
    assert_eq!(forget(&core, KEY), Err(SecretError::Native(-1)));
    // A later valid registration still works after any of the caught panics.
    set_registered(Some(MemSecretStore::new()));
    save(&core, KEY, &credentials(None, "pw 1", None)).unwrap();
    assert!(load(KEY).unwrap().is_some());
}

#[test]
fn ffi_callbacks_on_concurrent_threads() {
    let _lock = LOCK.lock().unwrap();
    set_registered(None);
    reset_native();
    register_native();
    let core = support::test_core();
    std::thread::scope(|scope| {
        for id in 0..8 {
            let core = &core;
            scope.spawn(move || {
                let key = format!("rdp:host-{id}");
                let creds = credentials(Some("alice"), "pw 1", Some("EXAMPLE"));
                save(core, &key, &creds).unwrap();
                assert_credentials(&load(&key).unwrap().unwrap(), &creds);
                assert!(saved_info(core, &key).saved);
                forget(core, &key).unwrap();
                assert!(core
                    .store()
                    .kv_get(&format!("{SAVED_INDEX_PREFIX}{key}"))
                    .unwrap()
                    .is_none());
            });
        }
    });
    assert!(NATIVE_VALUES.lock().unwrap().is_empty());
    assert_eq!(NATIVE.gets.load(Ordering::Relaxed), 8);
    assert_eq!(NATIVE.puts.load(Ordering::Relaxed), 8);
    assert_eq!(NATIVE.deletes.load(Ordering::Relaxed), 8);
}
