use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rs_mobile::rscore::dto_core::HubDisplay;
use rs_mobile::rscore::store::{
    HostSeenRow, KnownHostRow, LastGoodRow, PairedHostRow, SshKeyRow, Store, StoreError,
    StoredIdentity,
};
use rusqlite::Connection;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let serial = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("rs-mobile-store-{}-{serial}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn db(&self) -> PathBuf {
        self.0.join("rs.db")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn addr(value: &str) -> SocketAddr {
    value.parse().expect("test socket address")
}

fn paired(host_id: &str, paired_at_us: u64, addresses: &[&str]) -> PairedHostRow {
    PairedHostRow {
        host_id: host_id.to_owned(),
        fp: [host_id.len() as u8; 32],
        name: format!("name-{host_id}"),
        addresses: addresses.iter().map(|value| addr(value)).collect(),
        paired_at_us,
        permission: Some("control".to_owned()),
    }
}

fn known_host(server_id: &str, host: &str, port: u16, fingerprint: &str) -> KnownHostRow {
    KnownHostRow {
        server_id: server_id.to_owned(),
        host: host.to_owned(),
        port,
        algorithm: "ssh-ed25519".to_owned(),
        key: vec![1, 2, 3, 4],
        fingerprint: fingerprint.to_owned(),
        added_at_us: 10,
    }
}

fn ssh_key(key_id: &str) -> SshKeyRow {
    SshKeyRow {
        key_id: key_id.to_owned(),
        name: "work key".to_owned(),
        kind: "ed25519".to_owned(),
        fp: "SHA256:test".to_owned(),
        public_key: "ssh-ed25519 AAAA test".to_owned(),
        created_at_us: 42,
        installed_on: Vec::new(),
    }
}

#[test]
fn identity_round_trip_permissions_and_migration_version() {
    let dir = TempDir::new();
    let path = dir.db();
    let identity = StoredIdentity {
        cert_der: vec![0x30, 0x82, 0x01, 0x02],
        device_name: "Alice's iPhone".to_owned(),
        created_at_us: 11,
        registered_at_us: None,
    };

    {
        let store = Store::open(&path).expect("create store");
        assert_eq!(store.identity().expect("read empty identity"), None);
        assert!(!store.mark_registered(5).expect("mark without identity"));
        store.set_identity(&identity).expect("set identity");
        assert_eq!(
            store.identity().expect("read identity"),
            Some(identity.clone())
        );
        assert!(store.mark_registered(5).expect("mark registered"));
        assert_eq!(
            store.identity().expect("read registered identity"),
            Some(StoredIdentity {
                registered_at_us: Some(5),
                ..identity.clone()
            })
        );
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path)
            .expect("metadata for rs.db")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    {
        let store = Store::open(&path).expect("reopen store");
        assert_eq!(
            store.identity().expect("read identity after reopen"),
            Some(StoredIdentity {
                registered_at_us: Some(5),
                ..identity
            })
        );
        store.clear_identity().expect("clear identity");
        assert_eq!(store.identity().expect("read cleared identity"), None);
        assert!(!store.mark_registered(6).expect("mark after clear"));
    }

    let conn = Connection::open(&path).expect("open raw sqlite");
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("read user_version");
    assert_eq!(version, 1);
    let journal_mode: String = conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .expect("read journal mode");
    assert_eq!(journal_mode, "wal");
    conn.pragma_update(None, "user_version", 99)
        .expect("set future schema version");
    drop(conn);
    assert!(matches!(Store::open(&path), Err(StoreError::TooNew(99))));
}

#[test]
fn open_in_memory_identity_round_trip() {
    let store = Store::open_in_memory().expect("open in-memory store");
    let identity = StoredIdentity {
        cert_der: vec![9, 8, 7],
        device_name: "iPad".to_owned(),
        created_at_us: 100,
        registered_at_us: Some(200),
    };
    assert_eq!(store.identity().expect("read empty identity"), None);
    store.set_identity(&identity).expect("set identity");
    assert_eq!(store.identity().expect("read identity"), Some(identity));
    assert!(store.mark_registered(300).expect("mark registered"));
    assert!(store
        .take_resume_token("missing", 1)
        .expect("take missing")
        .is_none());
}

#[test]
fn paired_hosts_round_trip_order_update_and_forget() {
    let store = Store::open_in_memory().expect("open in-memory store");
    let first = paired("h1", 20, &["100.64.0.7:7400", "[fd7a:115c:a1e0::7]:7400"]);
    let second = paired("h2", 10, &["10.30.0.8:7400"]);
    store.upsert_paired_host(&first).expect("insert h1");
    store.upsert_paired_host(&second).expect("insert h2");

    let rows = store.paired_hosts().expect("list paired hosts");
    assert_eq!(rows, vec![second.clone(), first.clone()]);
    assert_eq!(rows[1].addresses, first.addresses);
    assert_eq!(
        store.paired_host("h1").expect("get h1"),
        Some(first.clone())
    );

    let mut renamed = first.clone();
    renamed.name = "renamed".to_owned();
    store.upsert_paired_host(&renamed).expect("update h1");
    assert_eq!(
        store.paired_host("h1").expect("get updated h1"),
        Some(renamed)
    );
    assert_eq!(store.paired_hosts().expect("list after update").len(), 2);

    assert!(store.forget_host("h1").expect("forget h1"));
    assert_eq!(store.paired_host("h1").expect("get forgotten h1"), None);
    assert!(!store.forget_host("h1").expect("forget h1 again"));
}

#[test]
fn host_seen_round_trip_and_forget() {
    let store = Store::open_in_memory().expect("open store");
    let row = HostSeenRow {
        host_id: "h1".into(),
        os: Some("macos".into()),
        os_version: Some("15.0".into()),
        host_version: Some("1.2.3".into()),
        features: Some(7),
        displays: vec![HubDisplay {
            id: 1,
            name: "Main".into(),
            width_px: 1920,
            height_px: 1080,
            primary: true,
        }],
        seen_at_us: 44,
    };
    store
        .upsert_paired_host(&paired("h1", 1, &["100.64.0.7:7400"]))
        .expect("pair");
    store.upsert_host_seen(&row).expect("seen");
    assert_eq!(store.host_seen("h1").expect("read seen"), Some(row));
    store.forget_host("h1").expect("forget");
    assert_eq!(store.host_seen("h1").expect("read forgotten"), None);
}

#[test]
fn host_seen_reads_displays_persisted_by_the_removed_hub_dto() {
    // The exact legacy display-row encoding remains readable.
    const OLD: &str =
        r#"[{"id":2,"name":"Studio Display","widthPx":5120,"heightPx":2880,"primary":false}]"#;
    let dir = TempDir::new();
    drop(Store::open(&dir.db()).expect("create database"));
    let conn = Connection::open(dir.db()).expect("open raw database");
    conn.execute(
        "INSERT INTO host_seen(host_id, os, os_version, host_version, features, displays, seen_at_us)
         VALUES ('h1', 'macos', NULL, NULL, NULL, ?1, 9)",
        [OLD],
    )
    .expect("write old row");
    drop(conn);

    let store = Store::open(&dir.db()).expect("reopen database");
    let seen = store
        .host_seen("h1")
        .expect("read old row")
        .expect("old row present");
    assert_eq!(
        seen.displays,
        vec![HubDisplay {
            id: 2,
            name: "Studio Display".into(),
            width_px: 5120,
            height_px: 2880,
            primary: false,
        }]
    );
    store.upsert_host_seen(&seen).expect("rewrite row");
    drop(store);
    let conn = Connection::open(dir.db()).expect("reopen raw database");
    let written: String = conn
        .query_row(
            "SELECT displays FROM host_seen WHERE host_id = 'h1'",
            [],
            |row| row.get(0),
        )
        .expect("read rewritten displays");
    assert_eq!(written, OLD);
}

#[test]
fn migration_adds_host_seen_without_losing_paired_hosts() {
    let dir = TempDir::new();
    let conn = Connection::open(dir.db()).expect("create old database");
    conn.execute_batch(
        "CREATE TABLE paired_hosts(
            host_id TEXT PRIMARY KEY,
            fp BLOB NOT NULL,
            name TEXT NOT NULL,
            addresses TEXT NOT NULL,
            paired_at_us INTEGER NOT NULL,
            permission TEXT
        );
        INSERT INTO paired_hosts(host_id, fp, name, addresses, paired_at_us, permission)
        VALUES ('h1', zeroblob(32), 'old host', '[\"100.64.0.7:7400\"]', 7, NULL);
        PRAGMA user_version = 1;",
    )
    .expect("write old schema");
    drop(conn);

    let store = Store::open(&dir.db()).expect("upgrade old database");
    assert_eq!(store.paired_hosts().expect("read paired host").len(), 1);
    assert_eq!(store.host_seen("h1").expect("read new table"), None);
}

#[test]
fn resume_tokens_are_single_use_expiring_and_forgotten_with_host() {
    let store = Store::open_in_memory().expect("open in-memory store");
    store
        .upsert_paired_host(&paired("h", 1, &["100.64.0.7:7400"]))
        .expect("insert host");

    store
        .save_resume_token("h", &[1; 16], 100)
        .expect("save resume token");
    assert_eq!(
        store.take_resume_token("h", 99).expect("take token"),
        Some(vec![1; 16])
    );
    assert_eq!(
        store.take_resume_token("h", 99).expect("take token again"),
        None
    );

    store
        .save_resume_token("h", &[2; 16], 100)
        .expect("save expiring token");
    assert_eq!(
        store
            .take_resume_token("h", 100)
            .expect("take expired token"),
        None
    );
    assert_eq!(
        store
            .take_resume_token("h", 0)
            .expect("expired token deleted"),
        None
    );

    store
        .save_resume_token("h", &[3; 16], 1_000)
        .expect("save token before forget");
    store
        .set_last_good(&LastGoodRow {
            host_id: "h".to_owned(),
            addr: addr("100.64.0.7:7400"),
            path: "tailnet".to_owned(),
            transport: "quic".to_owned(),
            at_us: 1,
        })
        .expect("save last good before forget");
    assert!(store.forget_host("h").expect("forget host"));
    assert_eq!(
        store.take_resume_token("h", 1).expect("take after forget"),
        None
    );
    assert_eq!(store.last_good("h").expect("last good after forget"), None);
}

#[test]
fn last_good_round_trip_and_overwrite() {
    let store = Store::open_in_memory().expect("open in-memory store");
    store
        .upsert_paired_host(&paired("h", 1, &["100.64.0.7:7400"]))
        .expect("insert host");
    let first = LastGoodRow {
        host_id: "h".to_owned(),
        addr: addr("100.64.0.7:7400"),
        path: "tailnet".to_owned(),
        transport: "quic".to_owned(),
        at_us: 5,
    };
    store.set_last_good(&first).expect("set last good");
    assert_eq!(
        store.last_good("h").expect("get last good"),
        Some(first.clone())
    );

    let second = LastGoodRow {
        addr: addr("192.168.31.7:7400"),
        path: "lan".to_owned(),
        transport: "tcp".to_owned(),
        at_us: 9,
        ..first
    };
    store.set_last_good(&second).expect("overwrite last good");
    assert_eq!(
        store.last_good("h").expect("get overwritten last good"),
        Some(second)
    );
    assert_eq!(
        store.last_good("missing").expect("get missing last good"),
        None
    );
}

#[test]
fn known_hosts_are_unique_per_address_and_scoped_to_server() {
    let store = Store::open_in_memory().expect("open in-memory store");
    let gateway = known_host("s1", "10.30.0.5", 22, "SHA256:gateway");
    let jump = known_host("s1", "10.30.0.6", 22, "SHA256:jump");
    let other = known_host("s2", "10.30.0.7", 22, "SHA256:other");
    store.save_known_host(&gateway).expect("save gateway");
    store.save_known_host(&jump).expect("save jump");
    store
        .save_known_host(&other)
        .expect("save other server key");

    let by_server = store
        .known_hosts_for_server("s1")
        .expect("list s1 known hosts");
    assert_eq!(by_server, vec![gateway.clone(), jump.clone()]);
    assert_eq!(store.known_hosts().expect("list all known hosts").len(), 3);

    let mut rotated = gateway.clone();
    rotated.fingerprint = "SHA256:rotated".to_owned();
    store.save_known_host(&rotated).expect("rotate gateway key");
    assert_eq!(store.known_hosts().expect("list after rotation").len(), 3);
    assert_eq!(
        store.known_host("10.30.0.5", 22).expect("get gateway"),
        Some(rotated)
    );

    assert!(store
        .forget_known_host("10.30.0.6", 22)
        .expect("forget jump host"));
    assert!(!store
        .forget_known_host("10.30.0.6", 22)
        .expect("forget jump host again"));
    assert_eq!(
        store
            .forget_known_hosts_for_server("s1")
            .expect("forget s1 hosts"),
        1
    );
    assert_eq!(
        store.known_hosts_for_server("s2").expect("s2 unaffected"),
        vec![other]
    );
}

#[test]
fn ssh_keys_round_trip_and_installed_on_deduplicates() {
    let store = Store::open_in_memory().expect("open in-memory store");
    let key = ssh_key("k1");
    store.upsert_ssh_key(&key).expect("insert key");
    assert_eq!(store.ssh_key("k1").expect("get key"), Some(key.clone()));
    assert_eq!(store.ssh_keys().expect("list keys"), vec![key.clone()]);

    assert!(store
        .add_key_installed_on("k1", "s1")
        .expect("install key on s1"));
    assert!(store
        .add_key_installed_on("k1", "s1")
        .expect("install key on s1 again"));
    assert_eq!(
        store.ssh_key("k1").expect("get key after installs"),
        Some(SshKeyRow {
            installed_on: vec!["s1".to_owned()],
            ..key
        })
    );
    assert!(!store
        .add_key_installed_on("missing", "s1")
        .expect("install missing key"));
}

#[test]
fn kv_set_get_overwrite_and_delete() {
    let store = Store::open_in_memory().expect("open in-memory store");
    assert_eq!(store.kv_get("theme").expect("get missing kv"), None);
    store.kv_set("theme", "dark").expect("set kv");
    assert_eq!(
        store.kv_get("theme").expect("get kv"),
        Some("dark".to_owned())
    );
    store.kv_set("theme", "light").expect("overwrite kv");
    assert_eq!(
        store.kv_get("theme").expect("get overwritten kv"),
        Some("light".to_owned())
    );
    assert!(store.kv_delete("theme").expect("delete kv"));
    assert_eq!(store.kv_get("theme").expect("get deleted kv"), None);
    assert!(!store.kv_delete("theme").expect("delete missing kv"));
}

#[test]
fn corrupt_paired_rows_are_skipped_by_lists_and_rejected_by_get() {
    let db_path = TempDir::new();
    let path = db_path.db();
    let conn = Connection::open(&path).expect("create raw database");
    conn.execute_batch(
        "CREATE TABLE identity (
             id INTEGER PRIMARY KEY CHECK (id = 1), cert_der BLOB NOT NULL,
             device_name TEXT NOT NULL, created_at_us INTEGER NOT NULL,
             registered_at_us INTEGER
         );
         CREATE TABLE paired_hosts (
             host_id TEXT PRIMARY KEY, fp BLOB NOT NULL, name TEXT NOT NULL,
             addresses TEXT NOT NULL, paired_at_us INTEGER NOT NULL, permission TEXT
         );",
    )
    .expect("create tables");
    conn.execute(
        "INSERT INTO paired_hosts VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            "good",
            [7_u8; 32].as_slice(),
            "good",
            "[\"100.64.0.7:7400\"]",
            1_i64,
            Option::<String>::None,
        ],
    )
    .expect("insert good raw row");
    conn.execute(
        "INSERT INTO paired_hosts VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            "bad",
            [1_u8, 2, 3].as_slice(),
            "bad",
            "[\"100.64.0.8:7400\"]",
            2_i64,
            Option::<String>::None
        ],
    )
    .expect("insert corrupt raw row");
    conn.execute(
        "INSERT INTO paired_hosts VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            "bad-json",
            [8_u8; 32].as_slice(),
            "bad-json",
            "not json",
            3_i64,
            Option::<String>::None,
        ],
    )
    .expect("insert invalid addresses json row");
    conn.execute(
        "INSERT INTO paired_hosts VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            "bad-address",
            [9_u8; 32].as_slice(),
            "bad-address",
            "[\"not-an-address\"]",
            4_i64,
            Option::<String>::None,
        ],
    )
    .expect("insert invalid address row");
    conn.pragma_update(None, "user_version", 1_i64)
        .expect("mark raw schema current");
    drop(conn);

    let path = db_path.db();
    let store = Store::open(&path).expect("open store with corrupt row");
    let rows = store.paired_hosts().expect("list skips corrupt row");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].host_id, "good");
    assert!(matches!(
        store.paired_host("bad"),
        Err(StoreError::Corrupt(_))
    ));
    assert!(matches!(
        store.paired_host("bad-json"),
        Err(StoreError::Corrupt(_))
    ));
    assert!(matches!(
        store.paired_host("bad-address"),
        Err(StoreError::Corrupt(_))
    ));
}

#[test]
fn opening_creates_missing_parent_directories() {
    let dir = TempDir::new();
    let path = dir.0.join("nested").join("deeper").join("rs.db");
    let store = Store::open(&path).expect("create store in missing directories");
    drop(store);
    assert!(path.is_file());
    assert_eq!(
        path.parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str()),
        Some("deeper")
    );
}
