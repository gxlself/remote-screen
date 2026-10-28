//! App-side SQLite storage (`rs.db`): identity certificate, paired hosts,
//! resume tokens, last-good paths, SSH known hosts, SSH key metadata and
//! small key/value entries.
//!
//! The connection is intentionally not synchronized here. The caller owns the
//! store and serializes access.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use thiserror::Error;
use tracing::warn;

const SCHEMA_VERSION: u32 = 1;
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

const SCHEMA_V1: &str = r#"
CREATE TABLE identity (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    cert_der        BLOB NOT NULL,
    device_name     TEXT NOT NULL,
    created_at_us   INTEGER NOT NULL,
    registered_at_us INTEGER
);

CREATE TABLE paired_hosts (
    host_id         TEXT PRIMARY KEY,
    fp              BLOB NOT NULL,
    name            TEXT NOT NULL,
    addresses       TEXT NOT NULL,
    paired_at_us    INTEGER NOT NULL,
    permission      TEXT
);

CREATE TABLE resume_tokens (
    host_id         TEXT PRIMARY KEY,
    token           BLOB NOT NULL,
    expires_at_us   INTEGER NOT NULL
);

CREATE TABLE last_good (
    host_id         TEXT PRIMARY KEY,
    addr            TEXT NOT NULL,
    path            TEXT NOT NULL,
    transport       TEXT NOT NULL,
    at_us           INTEGER NOT NULL
);

CREATE TABLE known_hosts (
    host            TEXT NOT NULL,
    port            INTEGER NOT NULL,
    server_id       TEXT NOT NULL,
    algorithm       TEXT NOT NULL,
    key             BLOB NOT NULL,
    fingerprint     TEXT NOT NULL,
    added_at_us     INTEGER NOT NULL,
    PRIMARY KEY (host, port)
);

CREATE TABLE ssh_keys (
    key_id          TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    kind            TEXT NOT NULL,
    fp              TEXT NOT NULL,
    public_key      TEXT NOT NULL,
    created_at_us   INTEGER NOT NULL,
    installed_on    TEXT NOT NULL
);

CREATE TABLE kv (
    key             TEXT PRIMARY KEY,
    value           TEXT NOT NULL
);
"#;

const SCHEMA_HOST_SEEN: &str = r#"
CREATE TABLE IF NOT EXISTS host_seen(
    host_id TEXT PRIMARY KEY,
    os TEXT,
    os_version TEXT,
    host_version TEXT,
    features INTEGER,
    displays TEXT,
    seen_at_us INTEGER NOT NULL
);
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredIdentity {
    pub cert_der: Vec<u8>,
    pub device_name: String,
    pub created_at_us: u64,
    pub registered_at_us: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedHostRow {
    pub host_id: String,
    pub fp: [u8; 32],
    pub name: String,
    pub addresses: Vec<SocketAddr>,
    pub paired_at_us: u64,
    pub permission: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSeenRow {
    pub host_id: String,
    pub os: Option<String>,
    pub os_version: Option<String>,
    pub host_version: Option<String>,
    pub features: Option<u64>,
    pub displays: Vec<super::dto_core::HubDisplay>,
    pub seen_at_us: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastGoodRow {
    pub host_id: String,
    pub addr: SocketAddr,
    pub path: String,
    pub transport: String,
    pub at_us: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownHostRow {
    pub server_id: String,
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub key: Vec<u8>,
    pub fingerprint: String,
    pub added_at_us: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshKeyRow {
    pub key_id: String,
    pub name: String,
    pub kind: String,
    pub fp: String,
    pub public_key: String,
    pub created_at_us: u64,
    pub installed_on: Vec<String>,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("corrupt row: {0}")]
    Corrupt(String),
    #[error("schema version {0} is newer than supported")]
    TooNew(u32),
}

pub struct Store {
    conn: Connection,
}

struct RawPairedHost {
    host_id: String,
    fp: Vec<u8>,
    name: String,
    addresses_json: String,
    paired_at_us: i64,
    permission: Option<String>,
}

struct RawKnownHost {
    server_id: String,
    host: String,
    port: i64,
    algorithm: String,
    key: Vec<u8>,
    fingerprint: String,
    added_at_us: i64,
}

struct RawSshKey {
    key_id: String,
    name: String,
    kind: String,
    fp: String,
    public_key: String,
    created_at_us: i64,
    installed_on_json: String,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store, StoreError> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        configure_connection(&conn, true)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Store, StoreError> {
        let conn = Connection::open_in_memory()?;
        configure_connection(&conn, false)?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn identity(&self) -> Result<Option<StoredIdentity>, StoreError> {
        let row = self
            .conn
            .query_row(
                "SELECT cert_der, device_name, created_at_us, registered_at_us
                 FROM identity WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(cert_der, device_name, created_at_us, registered_at_us)| {
            Ok(StoredIdentity {
                cert_der,
                device_name,
                created_at_us: timestamp_from_sql(created_at_us)?,
                registered_at_us: registered_at_us.map(timestamp_from_sql).transpose()?,
            })
        })
        .transpose()
    }

    pub fn set_identity(&self, identity: &StoredIdentity) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO identity (id, cert_der, device_name, created_at_us, registered_at_us)
             VALUES (1, ?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 cert_der = excluded.cert_der,
                 device_name = excluded.device_name,
                 created_at_us = excluded.created_at_us,
                 registered_at_us = excluded.registered_at_us",
            params![
                identity.cert_der,
                identity.device_name,
                timestamp_to_sql(identity.created_at_us)?,
                identity
                    .registered_at_us
                    .map(timestamp_to_sql)
                    .transpose()?,
            ],
        )?;
        Ok(())
    }

    pub fn clear_identity(&self) -> Result<(), StoreError> {
        self.conn.execute("DELETE FROM identity", [])?;
        Ok(())
    }

    pub fn mark_registered(&self, at_us: u64) -> Result<bool, StoreError> {
        let changed = self.conn.execute(
            "UPDATE identity SET registered_at_us = ?1 WHERE id = 1",
            [timestamp_to_sql(at_us)?],
        )?;
        Ok(changed > 0)
    }

    pub fn paired_hosts(&self) -> Result<Vec<PairedHostRow>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT host_id, fp, name, addresses, paired_at_us, permission
             FROM paired_hosts ORDER BY paired_at_us ASC",
        )?;
        let rows = stmt
            .query_map([], raw_paired_host)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|raw| match decode_paired_host(raw) {
                Ok(row) => Some(row),
                Err(error) => {
                    warn!(error = %error, "skipping corrupt paired host row");
                    None
                }
            })
            .collect())
    }

    pub fn paired_host(&self, host_id: &str) -> Result<Option<PairedHostRow>, StoreError> {
        let raw = self
            .conn
            .query_row(
                "SELECT host_id, fp, name, addresses, paired_at_us, permission
                 FROM paired_hosts WHERE host_id = ?1",
                [host_id],
                raw_paired_host,
            )
            .optional()?;
        raw.map(decode_paired_host).transpose()
    }

    pub fn upsert_paired_host(&self, row: &PairedHostRow) -> Result<(), StoreError> {
        let addresses = serde_json::to_string(
            &row.addresses
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        )
        .map_err(|error| StoreError::Corrupt(format!("serialize addresses: {error}")))?;
        self.conn.execute(
            "INSERT INTO paired_hosts
                 (host_id, fp, name, addresses, paired_at_us, permission)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(host_id) DO UPDATE SET
                 fp = excluded.fp,
                 name = excluded.name,
                 addresses = excluded.addresses,
                 paired_at_us = excluded.paired_at_us,
                 permission = excluded.permission",
            params![
                row.host_id,
                row.fp.as_slice(),
                row.name,
                addresses,
                timestamp_to_sql(row.paired_at_us)?,
                row.permission,
            ],
        )?;
        Ok(())
    }

    pub fn forget_host(&self, host_id: &str) -> Result<bool, StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM resume_tokens WHERE host_id = ?1", [host_id])?;
        tx.execute("DELETE FROM last_good WHERE host_id = ?1", [host_id])?;
        tx.execute("DELETE FROM host_seen WHERE host_id = ?1", [host_id])?;
        let removed = tx.execute("DELETE FROM paired_hosts WHERE host_id = ?1", [host_id])?;
        tx.commit()?;
        Ok(removed > 0)
    }

    pub fn host_seen(&self, host_id: &str) -> Result<Option<HostSeenRow>, StoreError> {
        let row = self
            .conn
            .query_row(
                "SELECT host_id, os, os_version, host_version, features, displays, seen_at_us
                 FROM host_seen WHERE host_id = ?1",
                [host_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                },
            )
            .optional()?;
        row.map(
            |(host_id, os, os_version, host_version, features, displays, seen_at_us)| {
                Ok(HostSeenRow {
                    host_id,
                    os,
                    os_version,
                    host_version,
                    features: features
                        .map(|value| {
                            u64::try_from(value)
                                .map_err(|_| StoreError::Corrupt("negative host features".into()))
                        })
                        .transpose()?,
                    displays: serde_json::from_str(displays.as_deref().unwrap_or("[]")).map_err(
                        |error| StoreError::Corrupt(format!("host seen displays: {error}")),
                    )?,
                    seen_at_us: timestamp_from_sql(seen_at_us)?,
                })
            },
        )
        .transpose()
    }

    pub fn upsert_host_seen(&self, row: &HostSeenRow) -> Result<(), StoreError> {
        let displays = serde_json::to_string(&row.displays)
            .map_err(|error| StoreError::Corrupt(format!("serialize host displays: {error}")))?;
        let features = row
            .features
            .map(i64::try_from)
            .transpose()
            .map_err(|_| StoreError::Corrupt("host features overflow".into()))?;
        self.conn.execute(
            "INSERT INTO host_seen(host_id, os, os_version, host_version, features, displays, seen_at_us)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(host_id) DO UPDATE SET os=excluded.os, os_version=excluded.os_version, host_version=excluded.host_version, features=excluded.features, displays=excluded.displays, seen_at_us=excluded.seen_at_us",
            params![row.host_id, row.os, row.os_version, row.host_version, features, displays, timestamp_to_sql(row.seen_at_us)?],
        )?;
        Ok(())
    }

    pub fn save_resume_token(
        &self,
        host_id: &str,
        token: &[u8],
        expires_at_us: u64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO resume_tokens (host_id, token, expires_at_us)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(host_id) DO UPDATE SET
                 token = excluded.token,
                 expires_at_us = excluded.expires_at_us",
            params![host_id, token, timestamp_to_sql(expires_at_us)?],
        )?;
        Ok(())
    }

    pub fn take_resume_token(
        &self,
        host_id: &str,
        now_us: u64,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        let row = tx
            .query_row(
                "SELECT token, expires_at_us FROM resume_tokens WHERE host_id = ?1",
                [host_id],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        let row = row
            .map(|(token, expires_at_us)| {
                Ok::<(Vec<u8>, u64), StoreError>((token, timestamp_from_sql(expires_at_us)?))
            })
            .transpose()?;
        let result = match row.as_ref() {
            Some((token, expires_at_us)) if now_us < *expires_at_us => Some(token.clone()),
            _ => None,
        };
        if row.is_some() {
            tx.execute("DELETE FROM resume_tokens WHERE host_id = ?1", [host_id])?;
        }
        tx.commit()?;
        Ok(result)
    }

    pub fn set_last_good(&self, row: &LastGoodRow) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO last_good (host_id, addr, path, transport, at_us)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(host_id) DO UPDATE SET
                 addr = excluded.addr,
                 path = excluded.path,
                 transport = excluded.transport,
                 at_us = excluded.at_us",
            params![
                row.host_id,
                row.addr.to_string(),
                row.path,
                row.transport,
                timestamp_to_sql(row.at_us)?,
            ],
        )?;
        Ok(())
    }

    pub fn last_good(&self, host_id: &str) -> Result<Option<LastGoodRow>, StoreError> {
        let raw = self
            .conn
            .query_row(
                "SELECT host_id, addr, path, transport, at_us
                 FROM last_good WHERE host_id = ?1",
                [host_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()?;
        raw.map(|(host_id, addr, path, transport, at_us)| {
            Ok(LastGoodRow {
                host_id,
                addr: parse_address(&addr)?,
                path,
                transport,
                at_us: timestamp_from_sql(at_us)?,
            })
        })
        .transpose()
    }

    pub fn known_host(&self, host: &str, port: u16) -> Result<Option<KnownHostRow>, StoreError> {
        let raw = self
            .conn
            .query_row(
                "SELECT server_id, host, port, algorithm, key, fingerprint, added_at_us
                 FROM known_hosts WHERE host = ?1 AND port = ?2",
                params![host, i64::from(port)],
                raw_known_host,
            )
            .optional()?;
        raw.map(decode_known_host).transpose()
    }

    pub fn save_known_host(&self, row: &KnownHostRow) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO known_hosts
                 (host, port, server_id, algorithm, key, fingerprint, added_at_us)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(host, port) DO UPDATE SET
                 server_id = excluded.server_id,
                 algorithm = excluded.algorithm,
                 key = excluded.key,
                 fingerprint = excluded.fingerprint,
                 added_at_us = excluded.added_at_us",
            params![
                row.host,
                i64::from(row.port),
                row.server_id,
                row.algorithm,
                row.key,
                row.fingerprint,
                timestamp_to_sql(row.added_at_us)?,
            ],
        )?;
        Ok(())
    }

    pub fn known_hosts(&self) -> Result<Vec<KnownHostRow>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT server_id, host, port, algorithm, key, fingerprint, added_at_us
             FROM known_hosts ORDER BY server_id ASC, host ASC, port ASC",
        )?;
        let rows = stmt
            .query_map([], raw_known_host)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|raw| match decode_known_host(raw) {
                Ok(row) => Some(row),
                Err(error) => {
                    warn!(error = %error, "skipping corrupt known host row");
                    None
                }
            })
            .collect())
    }

    pub fn known_hosts_for_server(&self, server_id: &str) -> Result<Vec<KnownHostRow>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT server_id, host, port, algorithm, key, fingerprint, added_at_us
             FROM known_hosts WHERE server_id = ?1
             ORDER BY server_id ASC, host ASC, port ASC",
        )?;
        let rows = stmt
            .query_map([server_id], raw_known_host)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|raw| match decode_known_host(raw) {
                Ok(row) => Some(row),
                Err(error) => {
                    warn!(error = %error, "skipping corrupt known host row");
                    None
                }
            })
            .collect())
    }

    pub fn forget_known_host(&self, host: &str, port: u16) -> Result<bool, StoreError> {
        let removed = self.conn.execute(
            "DELETE FROM known_hosts WHERE host = ?1 AND port = ?2",
            params![host, i64::from(port)],
        )?;
        Ok(removed > 0)
    }

    pub fn forget_known_hosts_for_server(&self, server_id: &str) -> Result<usize, StoreError> {
        let removed = self
            .conn
            .execute("DELETE FROM known_hosts WHERE server_id = ?1", [server_id])?;
        Ok(removed)
    }

    pub fn ssh_keys(&self) -> Result<Vec<SshKeyRow>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT key_id, name, kind, fp, public_key, created_at_us, installed_on
             FROM ssh_keys ORDER BY created_at_us ASC, key_id ASC",
        )?;
        let rows = stmt
            .query_map([], raw_ssh_key)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|raw| match decode_ssh_key(raw) {
                Ok(row) => Some(row),
                Err(error) => {
                    warn!(error = %error, "skipping corrupt ssh key row");
                    None
                }
            })
            .collect())
    }

    pub fn ssh_key(&self, key_id: &str) -> Result<Option<SshKeyRow>, StoreError> {
        let raw = self
            .conn
            .query_row(
                "SELECT key_id, name, kind, fp, public_key, created_at_us, installed_on
                 FROM ssh_keys WHERE key_id = ?1",
                [key_id],
                raw_ssh_key,
            )
            .optional()?;
        raw.map(decode_ssh_key).transpose()
    }

    pub fn upsert_ssh_key(&self, row: &SshKeyRow) -> Result<(), StoreError> {
        let installed_on = serde_json::to_string(&row.installed_on)
            .map_err(|error| StoreError::Corrupt(format!("serialize installed_on: {error}")))?;
        self.conn.execute(
            "INSERT INTO ssh_keys
                 (key_id, name, kind, fp, public_key, created_at_us, installed_on)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(key_id) DO UPDATE SET
                 name = excluded.name,
                 kind = excluded.kind,
                 fp = excluded.fp,
                 public_key = excluded.public_key,
                 created_at_us = excluded.created_at_us,
                 installed_on = excluded.installed_on",
            params![
                row.key_id,
                row.name,
                row.kind,
                row.fp,
                row.public_key,
                timestamp_to_sql(row.created_at_us)?,
                installed_on,
            ],
        )?;
        Ok(())
    }

    pub fn add_key_installed_on(&self, key_id: &str, server_id: &str) -> Result<bool, StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        let installed_on = tx
            .query_row(
                "SELECT installed_on FROM ssh_keys WHERE key_id = ?1",
                [key_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(installed_on) = installed_on else {
            tx.commit()?;
            return Ok(false);
        };
        let mut servers: Vec<String> = serde_json::from_str(&installed_on)
            .map_err(|error| StoreError::Corrupt(format!("installed_on: {error}")))?;
        if !servers.iter().any(|server| server == server_id) {
            servers.push(server_id.to_owned());
            let encoded = serde_json::to_string(&servers)
                .map_err(|error| StoreError::Corrupt(format!("installed_on: {error}")))?;
            tx.execute(
                "UPDATE ssh_keys SET installed_on = ?1 WHERE key_id = ?2",
                params![encoded, key_id],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    pub fn kv_get(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn
            .query_row("SELECT value FROM kv WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    pub fn kv_set(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn kv_delete(&self, key: &str) -> Result<bool, StoreError> {
        let removed = self.conn.execute("DELETE FROM kv WHERE key = ?1", [key])?;
        Ok(removed > 0)
    }

    fn migrate(&self) -> Result<(), StoreError> {
        let version = schema_version(&self.conn)?;
        match version {
            0 => self.apply_v1()?,
            1 => {}
            version => return Err(StoreError::TooNew(version)),
        }
        self.conn.execute_batch(SCHEMA_HOST_SEEN)?;
        Ok(())
    }

    fn apply_v1(&self) -> Result<(), StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch(SCHEMA_V1)?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(())
    }
}

fn configure_connection(conn: &Connection, file_backed: bool) -> Result<(), StoreError> {
    if file_backed {
        conn.pragma_update(None, "journal_mode", "WAL")?;
    }
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(())
}

fn schema_version(conn: &Connection) -> Result<u32, StoreError> {
    let version = conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    u32::try_from(version).map_err(|_| StoreError::Corrupt(format!("user_version {version}")))
}

fn raw_paired_host(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawPairedHost> {
    Ok(RawPairedHost {
        host_id: row.get(0)?,
        fp: row.get(1)?,
        name: row.get(2)?,
        addresses_json: row.get(3)?,
        paired_at_us: row.get(4)?,
        permission: row.get(5)?,
    })
}

fn decode_paired_host(raw: RawPairedHost) -> Result<PairedHostRow, StoreError> {
    let fp: [u8; 32] = raw.fp.as_slice().try_into().map_err(|_| {
        StoreError::Corrupt(format!(
            "paired host {} fingerprint has {} bytes",
            raw.host_id,
            raw.fp.len()
        ))
    })?;
    let addresses: Vec<String> = serde_json::from_str(&raw.addresses_json).map_err(|error| {
        StoreError::Corrupt(format!("paired host {} addresses: {error}", raw.host_id))
    })?;
    let addresses = addresses
        .into_iter()
        .map(|address| {
            parse_address(&address).map_err(|error| {
                StoreError::Corrupt(format!(
                    "paired host {} address {address:?}: {error}",
                    raw.host_id
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let paired_at_us = u64::try_from(raw.paired_at_us).map_err(|_| {
        StoreError::Corrupt(format!(
            "paired host {} has invalid paired_at_us",
            raw.host_id
        ))
    })?;
    Ok(PairedHostRow {
        host_id: raw.host_id,
        fp,
        name: raw.name,
        addresses,
        paired_at_us,
        permission: raw.permission,
    })
}

fn raw_known_host(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawKnownHost> {
    Ok(RawKnownHost {
        server_id: row.get(0)?,
        host: row.get(1)?,
        port: row.get(2)?,
        algorithm: row.get(3)?,
        key: row.get(4)?,
        fingerprint: row.get(5)?,
        added_at_us: row.get(6)?,
    })
}

fn decode_known_host(raw: RawKnownHost) -> Result<KnownHostRow, StoreError> {
    let port = u16::try_from(raw.port).map_err(|_| {
        StoreError::Corrupt(format!(
            "known host {}:{} has invalid port",
            raw.host, raw.port
        ))
    })?;
    let added_at_us = u64::try_from(raw.added_at_us).map_err(|_| {
        StoreError::Corrupt(format!(
            "known host {}:{} has invalid added_at_us",
            raw.host, raw.port
        ))
    })?;
    Ok(KnownHostRow {
        server_id: raw.server_id,
        host: raw.host,
        port,
        algorithm: raw.algorithm,
        key: raw.key,
        fingerprint: raw.fingerprint,
        added_at_us,
    })
}

fn raw_ssh_key(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawSshKey> {
    Ok(RawSshKey {
        key_id: row.get(0)?,
        name: row.get(1)?,
        kind: row.get(2)?,
        fp: row.get(3)?,
        public_key: row.get(4)?,
        created_at_us: row.get(5)?,
        installed_on_json: row.get(6)?,
    })
}

fn decode_ssh_key(raw: RawSshKey) -> Result<SshKeyRow, StoreError> {
    let installed_on: Vec<String> =
        serde_json::from_str(&raw.installed_on_json).map_err(|error| {
            StoreError::Corrupt(format!("ssh key {} installed_on: {error}", raw.key_id))
        })?;
    let created_at_us = u64::try_from(raw.created_at_us).map_err(|_| {
        StoreError::Corrupt(format!("ssh key {} has invalid created_at_us", raw.key_id))
    })?;
    Ok(SshKeyRow {
        key_id: raw.key_id,
        name: raw.name,
        kind: raw.kind,
        fp: raw.fp,
        public_key: raw.public_key,
        created_at_us,
        installed_on,
    })
}

fn parse_address(value: &str) -> Result<SocketAddr, StoreError> {
    value
        .parse()
        .map_err(|error| StoreError::Corrupt(format!("address {value:?}: {error}")))
}

fn timestamp_to_sql(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value)
        .map_err(|_| StoreError::Corrupt(format!("timestamp {value} is out of range")))
}

fn timestamp_from_sql(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value)
        .map_err(|_| StoreError::Corrupt(format!("timestamp {value} is out of range")))
}
