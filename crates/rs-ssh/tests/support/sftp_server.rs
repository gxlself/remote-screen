//! In-process SFTP server rooted at a temporary directory.
//!
//! `SftpTestServer` is shared by the SFTP browsing tests and can be reused by
//! the transfer tests. It binds loopback on an ephemeral port and accepts one
//! fixed Ed25519 client key.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use rs_ssh::{Credential, MemoryHostKeyStore, Prompter, SshOptions, SshSession, SshTarget};
use russh::{
    keys::PrivateKey,
    server::{self, Auth, Msg},
    Channel, ChannelId, MethodSet,
};
use russh_sftp::{
    protocol::{
        Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Packet, Status, StatusCode,
        Version,
    },
    server::{Handler as _, StatusReply},
};
use ssh_key::private::Ed25519Keypair;
use tokio::net::TcpListener;
use zeroize::Zeroizing;

const HOME: &str = "/home/test";
const READDIR_PAGE_SIZE: usize = 100;

pub const CLIENT_KEY_SEED: u8 = 7;

pub fn private(seed: u8) -> PrivateKey {
    PrivateKey::new(Ed25519Keypair::from_seed(&[seed; 32]).into(), "").unwrap()
}

pub fn credential(seed: u8) -> Credential {
    Credential::Ed25519 {
        secret: Zeroizing::new([seed; 32]),
    }
}

struct TestPrompter;

#[async_trait::async_trait]
impl Prompter for TestPrompter {
    async fn confirm_new_host_key(&self, _: &str, _: &str, _: &str) -> bool {
        true
    }

    async fn keyboard_interactive(
        &self,
        _: &str,
        _: &str,
        _: &[(String, bool)],
    ) -> Option<Vec<String>> {
        None
    }

    fn auth_url(&self, _: &str) {}
}

pub fn ssh_options() -> SshOptions {
    SshOptions {
        connect_timeout: Duration::from_secs(3),
        auth_timeout: Duration::from_secs(3),
        request_timeout: Duration::from_secs(3),
        ..Default::default()
    }
}

pub fn unique_temp_dir(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("rs-ssh-{label}-{}-{sequence}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IoRecord {
    pub path: String,
    pub offset: u64,
    pub len: u64,
}

/// Fault injection and I/O observations for one SSH connection.
pub struct SftpConnectionFaults {
    fail_after_read_bytes: AtomicU64,
    fail_after_write_bytes: AtomicU64,
    read_bytes: AtomicU64,
    write_bytes: AtomicU64,
    stop: AtomicBool,
    max_in_flight_reads: AtomicU64,
    max_in_flight_writes: AtomicU64,
    drop_write_responses: AtomicU64,
    write_responses_before_drop: AtomicU64,
    write_responses_dropped: AtomicU64,
    io_response_delay_millis: AtomicU64,
    hold_reads_from: AtomicU64,
    reads: Mutex<Vec<IoRecord>>,
    writes: Mutex<Vec<IoRecord>>,
    disconnect: Mutex<Option<server::Handle>>,
}

impl Default for SftpConnectionFaults {
    fn default() -> Self {
        Self {
            fail_after_read_bytes: AtomicU64::new(u64::MAX),
            fail_after_write_bytes: AtomicU64::new(u64::MAX),
            read_bytes: AtomicU64::new(0),
            write_bytes: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            max_in_flight_reads: AtomicU64::new(0),
            max_in_flight_writes: AtomicU64::new(0),
            drop_write_responses: AtomicU64::new(0),
            write_responses_before_drop: AtomicU64::new(0),
            write_responses_dropped: AtomicU64::new(0),
            io_response_delay_millis: AtomicU64::new(0),
            hold_reads_from: AtomicU64::new(u64::MAX),
            reads: Mutex::new(Vec::new()),
            writes: Mutex::new(Vec::new()),
            disconnect: Mutex::new(None),
        }
    }
}

impl SftpConnectionFaults {
    /// Hold reads at and beyond this offset until the connection is closed.
    /// Earlier chunks can reach the client without later replies racing a
    /// test-controlled disconnect.
    pub fn hold_reads_from(&self, offset: u64) {
        self.hold_reads_from.store(offset, Ordering::SeqCst);
    }

    /// Disconnect immediately, independently of the read-ahead window.
    pub fn disconnect(&self) {
        self.disconnect_handle();
    }

    fn read_limit(&self) -> Option<u64> {
        match self.fail_after_read_bytes.load(Ordering::SeqCst) {
            u64::MAX => None,
            value => Some(value),
        }
    }

    fn write_limit(&self) -> Option<u64> {
        match self.fail_after_write_bytes.load(Ordering::SeqCst) {
            u64::MAX => None,
            value => Some(value),
        }
    }

    fn disconnect_handle(&self) -> server::Handle {
        let handle = self
            .disconnect
            .lock()
            .unwrap()
            .clone()
            .expect("SFTP test server is connected");
        let stop = self.stop.swap(true, Ordering::SeqCst);
        if !stop {
            let task = handle.clone();
            tokio::spawn(async move {
                let _ = task
                    .disconnect(
                        russh::Disconnect::ByApplication,
                        "injected SFTP fault".to_owned(),
                        "en-US".to_owned(),
                    )
                    .await;
            });
        }
        handle
    }

    /// Disconnect this connection once it has cumulatively read more than
    /// `bytes` file bytes. `None` disables the fault.
    pub fn fail_after_read_bytes(&self, bytes: Option<u64>) {
        self.fail_after_read_bytes
            .store(bytes.unwrap_or(u64::MAX), Ordering::SeqCst);
        self.read_bytes.store(0, Ordering::SeqCst);
        self.stop.store(false, Ordering::SeqCst);
    }

    /// Disconnect this connection once it has cumulatively written more than
    /// `bytes` file bytes. `None` disables the fault.
    pub fn fail_after_write_bytes(&self, bytes: Option<u64>) {
        self.fail_after_write_bytes
            .store(bytes.unwrap_or(u64::MAX), Ordering::SeqCst);
        self.write_bytes.store(0, Ordering::SeqCst);
        self.stop.store(false, Ordering::SeqCst);
    }

    /// Drop responses for this many subsequent writes, then disconnect. The
    /// writes still reach the backing file, which models acknowledgements lost
    /// after a server-side commit and creates a gap at the client.
    pub fn drop_write_responses_after(&self, before: u64, writes: u64) {
        self.write_responses_before_drop
            .store(before, Ordering::SeqCst);
        self.drop_write_responses.store(writes, Ordering::SeqCst);
    }

    /// Hold read and write replies for `delay`, making request pipelining
    /// observable even on a loopback fixture that would otherwise answer
    /// before the client enqueues its next request.
    pub fn delay_io_responses(&self, delay: Option<Duration>) {
        self.io_response_delay_millis.store(
            delay.map_or(0, |delay| {
                delay.as_millis().min(u128::from(u64::MAX)) as u64
            }),
            Ordering::SeqCst,
        );
    }

    fn io_response_delay(&self) -> Option<Duration> {
        match self.io_response_delay_millis.load(Ordering::SeqCst) {
            0 => None,
            millis => Some(Duration::from_millis(millis)),
        }
    }

    /// Largest number of read requests observed before their responses.
    pub fn max_in_flight_reads(&self) -> u64 {
        self.max_in_flight_reads.load(Ordering::SeqCst)
    }

    /// Largest number of write requests observed before their responses.
    pub fn max_in_flight_writes(&self) -> u64 {
        self.max_in_flight_writes.load(Ordering::SeqCst)
    }

    /// Number of write acknowledgements that were intentionally discarded.
    pub fn write_responses_dropped(&self) -> u64 {
        self.write_responses_dropped.load(Ordering::SeqCst)
    }

    /// Every `read` request handled for this connection, in arrival order.
    pub fn read_requests(&self) -> Vec<IoRecord> {
        self.reads.lock().unwrap().clone()
    }

    /// Every `write` request handled for this connection, in arrival order.
    pub fn write_requests(&self) -> Vec<IoRecord> {
        self.writes.lock().unwrap().clone()
    }
}

/// Tracks SFTP request ids that have been read from the client but not yet
/// answered. The server handler itself is serial; the proxy observes the real
/// client pipeline on both sides of that handler.
#[derive(Default)]
struct PendingRequests {
    reads: HashSet<u32>,
    writes: HashSet<u32>,
}

/// Shared test-server state. Each accepted SSH connection gets its own
/// [`SftpConnectionFaults`]; the server-level helpers address the most recent
/// connection so single-connection tests stay concise.
pub struct SftpFaults {
    connections: Mutex<Vec<Arc<SftpConnectionFaults>>>,
    posix_rename: Arc<AtomicBool>,
}

impl Default for SftpFaults {
    fn default() -> Self {
        Self {
            connections: Mutex::new(Vec::new()),
            posix_rename: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl SftpFaults {
    fn register(&self) -> Arc<SftpConnectionFaults> {
        let connection = Arc::new(SftpConnectionFaults::default());
        self.connections.lock().unwrap().push(connection.clone());
        connection
    }

    fn latest(&self) -> Arc<SftpConnectionFaults> {
        self.connections
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("SFTP test server has no connection")
    }

    /// Connection-scoped fault hooks, ordered by SSH connection acceptance.
    pub fn connections(&self) -> Vec<Arc<SftpConnectionFaults>> {
        self.connections.lock().unwrap().clone()
    }
}

#[derive(Clone)]
enum EntryHandle {
    Dir {
        entries: Arc<Mutex<Vec<File>>>,
    },
    File {
        path: PathBuf,
        file: Arc<Mutex<fs::File>>,
    },
}

struct HandleState {
    next: u64,
    entries: HashMap<String, EntryHandle>,
}

impl HandleState {
    fn new() -> Self {
        Self {
            next: 1,
            entries: HashMap::new(),
        }
    }

    fn insert(&mut self, prefix: &str, handle: EntryHandle) -> String {
        let id = format!("{prefix}-{}", self.next);
        self.next += 1;
        self.entries.insert(id.clone(), handle);
        id
    }

    fn get(&self, id: &str) -> Option<EntryHandle> {
        self.entries.get(id).cloned()
    }

    fn remove(&mut self, id: &str) -> Option<EntryHandle> {
        self.entries.remove(id)
    }
}

#[derive(Clone)]
struct SftpHandler {
    root: PathBuf,
    faults: Arc<SftpConnectionFaults>,
    handles: Arc<Mutex<HandleState>>,
    posix_rename: bool,
}

impl SftpHandler {
    fn new(root: PathBuf, faults: Arc<SftpConnectionFaults>, posix_rename: bool) -> Self {
        Self {
            root,
            faults,
            handles: Arc::new(Mutex::new(HandleState::new())),
            posix_rename,
        }
    }

    fn map_path(&self, path: &str) -> Result<PathBuf, StatusReply> {
        if path == "/root" || path.starts_with("/root/") {
            return Err(status(StatusCode::PermissionDenied));
        }
        let relative = if path == HOME {
            ""
        } else if let Some(rest) = path.strip_prefix(&format!("{HOME}/")) {
            rest
        } else if let Some(rest) = path.strip_prefix('/') {
            rest
        } else {
            path
        };
        let mut mapped = self.root.clone();
        for component in Path::new(relative).components() {
            match component {
                Component::Normal(part) => mapped.push(part),
                Component::CurDir => {}
                Component::ParentDir => {
                    if mapped == self.root {
                        return Err(status(StatusCode::PermissionDenied));
                    }
                    mapped.pop();
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(status(StatusCode::PermissionDenied));
                }
            }
        }
        if !mapped.starts_with(&self.root) {
            return Err(status(StatusCode::PermissionDenied));
        }
        Ok(mapped)
    }

    fn map_path_for_realpath(&self, path: &str) -> Result<PathBuf, StatusReply> {
        let mut components = Vec::new();
        for component in Path::new(path).components() {
            match component {
                Component::Normal(part) => components.push(part.to_owned()),
                Component::CurDir => {}
                Component::ParentDir => {
                    if components.pop().is_none() {
                        return Err(status(StatusCode::PermissionDenied));
                    }
                }
                Component::RootDir | Component::Prefix(_) => {}
            }
        }
        let home: Vec<_> = Path::new(HOME)
            .components()
            .filter_map(|component| match component {
                Component::Normal(part) => Some(part.to_owned()),
                _ => None,
            })
            .collect();
        if components.len() < home.len() || components[..home.len()] != home[..] {
            return Err(status(StatusCode::PermissionDenied));
        }
        let mut mapped = self.root.clone();
        for component in &components[home.len()..] {
            mapped.push(component);
        }
        Ok(mapped)
    }

    fn display_path(&self, path: &Path) -> String {
        match path.strip_prefix(&self.root) {
            Ok(relative) if relative.as_os_str().is_empty() => HOME.to_owned(),
            Ok(relative) => format!("{HOME}/{}", relative.to_string_lossy()),
            Err(_) => path.to_string_lossy().into_owned(),
        }
    }

    fn attrs(&self, path: &Path, follow: bool) -> Result<Attrs, StatusReply> {
        let metadata = if follow {
            fs::metadata(path)
        } else {
            fs::symlink_metadata(path)
        }
        .map_err(io_status)?;
        Ok(Attrs {
            id: 0,
            attrs: FileAttributes::from(&metadata),
        })
    }

    fn file_entry(path: &Path) -> Result<File, StatusReply> {
        let metadata = fs::symlink_metadata(path).map_err(io_status)?;
        Ok(File::new(
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            FileAttributes::from(&metadata),
        ))
    }

    fn record_read(&self, path: String, offset: u64, len: u64) {
        self.faults
            .reads
            .lock()
            .unwrap()
            .push(IoRecord { path, offset, len });
    }

    fn record_write(&self, path: String, offset: u64, len: u64) {
        self.faults
            .writes
            .lock()
            .unwrap()
            .push(IoRecord { path, offset, len });
    }
}

fn status(code: StatusCode) -> StatusReply {
    StatusReply::new(code)
}

fn io_status(error: std::io::Error) -> StatusReply {
    let code = match error.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        std::io::ErrorKind::AlreadyExists => StatusCode::Failure,
        _ => StatusCode::Failure,
    };
    StatusReply::new(code).with_message(error.to_string())
}

fn ok_status(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".to_owned(),
        language_tag: "en-US".to_owned(),
    }
}

impl russh_sftp::server::Handler for SftpHandler {
    type Error = StatusReply;

    fn unimplemented(&self) -> Self::Error {
        status(StatusCode::OpUnsupported)
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        let mut version = Version::new();
        if self.posix_rename {
            version
                .extensions
                .insert("posix-rename@openssh.com".to_owned(), "1".to_owned());
        }
        Ok(version)
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let resolved = if path == "." {
            HOME.to_owned()
        } else {
            let mapped = self.map_path_for_realpath(&path)?;
            self.display_path(&mapped)
        };
        Ok(Name {
            id,
            files: vec![File::dummy(resolved)],
        })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let path = self.map_path(&path)?;
        let mut entries = Vec::new();
        for item in fs::read_dir(&path).map_err(io_status)? {
            entries.push(Self::file_entry(&item.map_err(io_status)?.path())?);
        }
        entries.sort_by(|left, right| left.filename.cmp(&right.filename));
        let handle = self.handles.lock().unwrap().insert(
            "dir",
            EntryHandle::Dir {
                entries: Arc::new(Mutex::new(entries)),
            },
        );
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        let entry = self
            .handles
            .lock()
            .unwrap()
            .get(&handle)
            .ok_or_else(|| status(StatusCode::NoSuchFile))?;
        let EntryHandle::Dir { entries } = entry else {
            return Err(status(StatusCode::Failure));
        };
        let mut entries = entries.lock().unwrap();
        if entries.is_empty() {
            return Err(status(StatusCode::Eof));
        }
        let count = entries.len().min(READDIR_PAGE_SIZE);
        let page = entries.drain(..count).collect();
        Ok(Name { id, files: page })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        let entry = self
            .handles
            .lock()
            .unwrap()
            .remove(&handle)
            .ok_or_else(|| status(StatusCode::NoSuchFile))?;
        match entry {
            EntryHandle::Dir { .. } => Ok(ok_status(id)),
            EntryHandle::File { file, .. } => {
                file.lock().unwrap().flush().map_err(io_status)?;
                Ok(ok_status(id))
            }
        }
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let path = self.map_path(&path)?;
        let mut attrs = self.attrs(&path, true)?;
        attrs.id = id;
        Ok(attrs)
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let path = self.map_path(&path)?;
        let mut attrs = self.attrs(&path, false)?;
        attrs.id = id;
        Ok(attrs)
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let entry = self
            .handles
            .lock()
            .unwrap()
            .get(&handle)
            .ok_or_else(|| status(StatusCode::NoSuchFile))?;
        let EntryHandle::File { file, .. } = entry else {
            return Err(status(StatusCode::Failure));
        };
        let metadata = file.lock().unwrap().metadata().map_err(io_status)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&metadata),
        })
    }

    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let path = self.map_path(&path)?;
        apply_attrs(&path, &attrs)?;
        Ok(ok_status(id))
    }

    async fn fsetstat(
        &mut self,
        id: u32,
        handle: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let entry = self
            .handles
            .lock()
            .unwrap()
            .get(&handle)
            .ok_or_else(|| status(StatusCode::NoSuchFile))?;
        let EntryHandle::File { path, .. } = entry else {
            return Err(status(StatusCode::Failure));
        };
        apply_attrs(&path, &attrs)?;
        Ok(ok_status(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let path = self.map_path(&path)?;
        fs::create_dir(&path).map_err(io_status)?;
        Ok(ok_status(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        let path = self.map_path(&path)?;
        fs::remove_dir(&path).map_err(io_status)?;
        Ok(ok_status(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        let path = self.map_path(&filename)?;
        fs::remove_file(&path).map_err(io_status)?;
        Ok(ok_status(id))
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        let oldpath = self.map_path(&oldpath)?;
        let newpath = self.map_path(&newpath)?;
        fs::rename(&oldpath, &newpath).map_err(io_status)?;
        Ok(ok_status(id))
    }

    async fn extended(
        &mut self,
        id: u32,
        request: String,
        data: Vec<u8>,
    ) -> Result<russh_sftp::protocol::Packet, Self::Error> {
        match request.as_str() {
            "posix-rename@openssh.com" if self.posix_rename => {
                let (oldpath, rest) = read_extension_string(&data)?;
                let (newpath, _) = read_extension_string(rest)?;
                let oldpath = self.map_path(&oldpath)?;
                let newpath = self.map_path(&newpath)?;
                fs::rename(&oldpath, &newpath).map_err(io_status)?;
                Ok(ok_status(id).into())
            }
            _ => Err(status(StatusCode::OpUnsupported)),
        }
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let path = self.map_path(&filename)?;
        let options: fs::OpenOptions = pflags.into();
        let file = options.open(&path).map_err(io_status)?;
        if attrs.size.is_some() || attrs.permissions.is_some() || attrs.mtime.is_some() {
            apply_attrs(&path, &attrs)?;
        }
        let handle = self.handles.lock().unwrap().insert(
            "file",
            EntryHandle::File {
                path,
                file: Arc::new(Mutex::new(file)),
            },
        );
        Ok(Handle { id, handle })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let (path, file) = match self
            .handles
            .lock()
            .unwrap()
            .get(&handle)
            .ok_or_else(|| status(StatusCode::NoSuchFile))?
        {
            EntryHandle::File { path, file } => (path, file),
            EntryHandle::Dir { .. } => return Err(status(StatusCode::Failure)),
        };
        self.record_read(self.display_path(&path), offset, u64::from(len));
        if offset >= self.faults.hold_reads_from.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        if self
            .faults
            .read_limit()
            .is_some_and(|limit| self.faults.read_bytes.load(Ordering::SeqCst) >= limit)
        {
            self.faults.disconnect_handle();
        }
        let mut buffer = vec![0; len as usize];
        // Cloned file descriptors share an offset on Unix. Keep seek and read
        // under one lock so pipelined requests cannot move each other's cursor.
        let mut reader = file.lock().unwrap();
        reader.seek(SeekFrom::Start(offset)).map_err(io_status)?;
        let count = reader.read(&mut buffer).map_err(io_status)?;
        drop(reader);
        buffer.truncate(count);
        self.faults
            .read_bytes
            .fetch_add(count as u64, Ordering::SeqCst);
        if self
            .faults
            .read_limit()
            .is_some_and(|limit| self.faults.read_bytes.load(Ordering::SeqCst) > limit)
        {
            self.faults.disconnect_handle();
        }
        if count == 0 {
            return Err(status(StatusCode::Eof));
        }
        Ok(Data { id, data: buffer })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let (path, file) = match self
            .handles
            .lock()
            .unwrap()
            .get(&handle)
            .ok_or_else(|| status(StatusCode::NoSuchFile))?
        {
            EntryHandle::File { path, file } => (path, file),
            EntryHandle::Dir { .. } => return Err(status(StatusCode::Failure)),
        };
        self.record_write(self.display_path(&path), offset, data.len() as u64);
        if self
            .faults
            .write_limit()
            .is_some_and(|limit| self.faults.write_bytes.load(Ordering::SeqCst) >= limit)
        {
            self.faults.disconnect_handle();
        }
        let mut file = file.lock().unwrap();
        file.seek(SeekFrom::Start(offset)).map_err(io_status)?;
        file.write_all(&data).map_err(io_status)?;
        file.flush().map_err(io_status)?;
        drop(file);
        self.faults
            .write_bytes
            .fetch_add(data.len() as u64, Ordering::SeqCst);
        if self
            .faults
            .write_limit()
            .is_some_and(|limit| self.faults.write_bytes.load(Ordering::SeqCst) > limit)
        {
            self.faults.disconnect_handle();
        }
        Ok(ok_status(id))
    }

    async fn readlink(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let path = self.map_path(&path)?;
        let target = fs::read_link(&path).map_err(io_status)?;
        let metadata = fs::symlink_metadata(&path).map_err(io_status)?;
        Ok(Name {
            id,
            files: vec![File {
                filename: target.to_string_lossy().into_owned(),
                longname: String::new(),
                attrs: FileAttributes::from(&metadata),
            }],
        })
    }

    async fn symlink(
        &mut self,
        id: u32,
        linkpath: String,
        targetpath: String,
    ) -> Result<Status, Self::Error> {
        let linkpath = self.map_path(&linkpath)?;
        std::os::unix::fs::symlink(targetpath, linkpath).map_err(io_status)?;
        Ok(ok_status(id))
    }
}

fn apply_attrs(path: &Path, attrs: &FileAttributes) -> Result<(), StatusReply> {
    use std::os::unix::fs::PermissionsExt as _;

    if let Some(size) = attrs.size {
        let file = fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(io_status)?;
        file.set_len(size).map_err(io_status)?;
    }
    if let Some(mtime) = attrs.mtime {
        let file = fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(io_status)?;
        file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(u64::from(mtime)))
            .map_err(io_status)?;
    }
    if let Some(permissions) = attrs.permissions {
        let mut current = fs::metadata(path).map_err(io_status)?.permissions();
        current.set_mode(permissions & 0o7777);
        fs::set_permissions(path, current).map_err(io_status)?;
    }
    Ok(())
}

fn read_extension_string(data: &[u8]) -> Result<(String, &[u8]), StatusReply> {
    let Some((length, rest)) = data.split_at_checked(4) else {
        return Err(status(StatusCode::BadMessage));
    };
    let length = u32::from_be_bytes(
        length
            .try_into()
            .map_err(|_| status(StatusCode::BadMessage))?,
    );
    let length = usize::try_from(length).map_err(|_| status(StatusCode::BadMessage))?;
    let Some((value, rest)) = rest.split_at_checked(length) else {
        return Err(status(StatusCode::BadMessage));
    };
    let value = std::str::from_utf8(value)
        .map_err(|_| status(StatusCode::BadMessage))?
        .to_owned();
    Ok((value, rest))
}

struct SshHandler {
    accept_sftp: bool,
    posix_rename: bool,
    root: PathBuf,
    faults: Arc<SftpConnectionFaults>,
    channels: HashMap<ChannelId, Channel<Msg>>,
}

impl server::Handler for SshHandler {
    type Error = russh::Error;

    async fn auth_publickey_offered(
        &mut self,
        _user: &str,
        key: &ssh_key::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(
            if key.key_data() == private(CLIENT_KEY_SEED).public_key().key_data() {
                Auth::Accept
            } else {
                Auth::reject()
            },
        )
    }

    async fn auth_publickey(
        &mut self,
        _user: &str,
        key: &ssh_key::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(
            if key.key_data() == private(CLIENT_KEY_SEED).public_key().key_data() {
                Auth::Accept
            } else {
                Auth::reject()
            },
        )
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" || !self.accept_sftp {
            return session.channel_failure(channel);
        }
        let Some(channel) = self.channels.remove(&channel) else {
            return session.channel_failure(channel);
        };
        session.channel_success(channel.id())?;
        *self.faults.disconnect.lock().unwrap() = Some(session.handle());
        let handler = SftpHandler::new(self.root.clone(), self.faults.clone(), self.posix_rename);
        let task = tokio::spawn(run_sftp_server(channel, handler));
        tokio::spawn(async move {
            let _ = task.await;
        });
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.channels.remove(&channel);
        Ok(())
    }
}

/// Runs a small pipelined SFTP front end over the existing handler. The public
/// `russh-sftp` server processes one request at a time, so this dispatcher
/// preserves the same handler semantics while exposing the client's real
/// in-flight window and allowing selected write acknowledgements to be lost.
async fn run_sftp_server(channel: Channel<Msg>, handler: SftpHandler) {
    let (reader, mut writer) = tokio::io::split(channel.into_stream());
    // read_exact is not cancellation safe: a reply or completed handler may
    // win select! after part of a request has already been consumed. Retain
    // the same read future until the entire frame arrives, including across
    // iterations that only send replies or reap handlers.
    let mut incoming = Box::pin(read_next_sftp_packet(reader));
    let (responses, mut response_rx) = tokio::sync::mpsc::unbounded_channel::<Response>();
    let mut tasks = tokio::task::JoinSet::new();
    let mut pending_reads = HashSet::new();
    let mut pending_writes = HashSet::new();

    loop {
        tokio::select! {
            (reader, packet) = &mut incoming => {
                incoming.set(read_next_sftp_packet(reader));
                let Ok(Some(packet)) = packet else { break };
                let Some(request) = decode_request(&packet) else {
                    let _ = write_sftp_packet(&mut writer, 101, 0, &encode_status(0, StatusCode::BadMessage, "malformed request")).await;
                    continue;
                };
                if let Some(id) = request.id {
                    if request.kind == 5 {
                        pending_reads.insert(id);
                        update_max(&handler.faults.max_in_flight_reads, pending_reads.len() as u64);
                    } else if request.kind == 6 {
                        pending_writes.insert(id);
                        update_max(&handler.faults.max_in_flight_writes, pending_writes.len() as u64);
                    }
                }
                let task_handler = handler.clone();
                let task_faults = handler.faults.clone();
                let tx = responses.clone();
                let kind = request.kind;
                tasks.spawn(async move {
                    let response = dispatch_request(task_handler, request).await;
                    if matches!(kind, 5 | 6) {
                        if let Some(delay) = task_faults.io_response_delay() {
                            tokio::time::sleep(delay).await;
                        }
                    }
                    let _ = tx.send(response);
                });
            }
            Some(response) = response_rx.recv() => {
                let is_write_ack = response.kind == 101 && pending_writes.contains(&response.id);
                if response.kind == 103 || (response.kind == 101 && !is_write_ack) {
                    pending_reads.remove(&response.id);
                }
                let before = handler.faults.write_responses_before_drop.load(Ordering::SeqCst);
                let drop_ack = is_write_ack && if before > 0 {
                    let _ = handler.faults.write_responses_before_drop.fetch_update(
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                        |value| value.checked_sub(1),
                    );
                    false
                } else {
                    handler.faults.drop_write_responses.fetch_update(
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                        |value| value.checked_sub(1),
                    ).is_ok()
                };
                if drop_ack {
                    handler.faults.write_responses_dropped.fetch_add(1, Ordering::SeqCst);
                    if handler.faults.drop_write_responses.load(Ordering::SeqCst) == 0 {
                        let handle = handler.faults.disconnect.lock().unwrap().clone();
                        if let Some(handle) = handle {
                            tokio::spawn(async move {
                                let _ = handle.disconnect(
                                    russh::Disconnect::ByApplication,
                                    "dropped SFTP write acknowledgements".to_owned(),
                                    "en-US".to_owned(),
                                ).await;
                            });
                        }
                    }
                    continue;
                }
                if write_sftp_packet(&mut writer, response.kind, response.id, &response.payload).await.is_err() {
                    break;
                }
                // A deliberately dropped acknowledgement is still in flight
                // from the client's perspective. Retire it only after sending
                // the response, so fault observations include those gaps.
                if is_write_ack {
                    pending_writes.remove(&response.id);
                }
            }
            _ = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
    tasks.shutdown().await;
}

async fn read_next_sftp_packet<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
) -> (R, Result<Option<Vec<u8>>, std::io::Error>) {
    let packet = read_sftp_packet(&mut reader).await;
    (reader, packet)
}

async fn read_sftp_packet<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<Option<Vec<u8>>, std::io::Error> {
    use tokio::io::AsyncReadExt as _;
    let mut header = [0u8; 4];
    match reader.read_exact(&mut header).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = u32::from_be_bytes(header) as usize;
    let mut packet = vec![0u8; length];
    reader.read_exact(&mut packet).await?;
    Ok(Some(packet))
}

async fn write_sftp_packet<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    kind: u8,
    _id: u32,
    payload: &[u8],
) -> Result<(), std::io::Error> {
    use tokio::io::AsyncWriteExt as _;
    let length = u32::try_from(payload.len() + 1)
        .map_err(|_| std::io::Error::other("SFTP response is too large"))?;
    writer.write_all(&length.to_be_bytes()).await?;
    writer.write_all(&[kind]).await?;
    writer.write_all(payload).await?;
    writer.flush().await
}

fn update_max(max: &AtomicU64, value: u64) {
    let mut current = max.load(Ordering::SeqCst);
    while value > current {
        match max.compare_exchange(current, value, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

struct Request {
    kind: u8,
    id: Option<u32>,
    body: Vec<u8>,
}

struct Response {
    kind: u8,
    id: u32,
    payload: Vec<u8>,
}

fn decode_request(packet: &[u8]) -> Option<Request> {
    let (&kind, rest) = packet.split_first()?;
    let (id, body) = if kind == 1 {
        (None, rest.to_vec())
    } else {
        let (id, body) = rest.split_first_chunk::<4>()?;
        (Some(u32::from_be_bytes(*id)), body.to_vec())
    };
    Some(Request { kind, id, body })
}

async fn dispatch_request(handler: SftpHandler, request: Request) -> Response {
    let id = request.id.unwrap_or(0);
    let packet = match request.kind {
        1 => match RequestReader::new(&request.body).u32() {
            Some(version) => match handler.clone().init(version, HashMap::new()).await {
                Ok(packet) => packet.into(),
                Err(error) => error_status(id, error),
            },
            None => error_status(id, status(StatusCode::BadMessage)),
        },
        3 => match (|| -> Result<_, StatusReply> {
            let mut reader = RequestReader::new(&request.body);
            let name = reader.string()?;
            let flags = reader.u32().ok_or_else(|| status(StatusCode::BadMessage))?;
            let attrs = reader.attrs()?;
            Ok((name, flags, attrs))
        })() {
            Ok((name, flags, attrs)) => match handler
                .clone()
                .open(id, name, OpenFlags::from_bits_truncate(flags), attrs)
                .await
            {
                Ok(packet) => packet.into(),
                Err(error) => error_status(id, error),
            },
            Err(error) => error_status(id, error),
        },
        4 => match RequestReader::new(&request.body).string() {
            Ok(handle) => match handler.clone().close(id, handle).await {
                Ok(packet) => packet.into(),
                Err(error) => error_status(id, error),
            },
            Err(error) => error_status(id, error),
        },
        5 => match (|| -> Result<_, StatusReply> {
            let mut reader = RequestReader::new(&request.body);
            Ok((
                reader.string()?,
                reader.u64().ok_or_else(|| status(StatusCode::BadMessage))?,
                reader.u32().ok_or_else(|| status(StatusCode::BadMessage))?,
            ))
        })() {
            Ok((handle, offset, len)) => {
                match handler.clone().read(id, handle, offset, len).await {
                    Ok(packet) => packet.into(),
                    Err(error) => error_status(id, error),
                }
            }
            Err(error) => error_status(id, error),
        },
        6 => match (|| -> Result<_, StatusReply> {
            let mut reader = RequestReader::new(&request.body);
            let handle = reader.string()?;
            let offset = reader.u64().ok_or_else(|| status(StatusCode::BadMessage))?;
            let data = reader.string_bytes()?;
            Ok((handle, offset, data))
        })() {
            Ok((handle, offset, data)) => {
                match handler.clone().write(id, handle, offset, data).await {
                    Ok(packet) => packet.into(),
                    Err(error) => error_status(id, error),
                }
            }
            Err(error) => error_status(id, error),
        },
        7 => {
            path_request(
                handler,
                id,
                &request.body,
                |mut handler, id, path| async move { handler.lstat(id, path).await },
            )
            .await
        }
        8 => {
            handle_request(
                handler,
                id,
                &request.body,
                |mut handler, id, handle| async move { handler.fstat(id, handle).await },
            )
            .await
        }
        9 => match path_and_attrs(&request.body) {
            Ok((path, attrs)) => match handler.clone().setstat(id, path, attrs).await {
                Ok(packet) => packet.into(),
                Err(error) => error_status(id, error),
            },
            Err(error) => error_status(id, error),
        },
        10 => match handle_and_attrs(&request.body) {
            Ok((handle, attrs)) => match handler.clone().fsetstat(id, handle, attrs).await {
                Ok(packet) => packet.into(),
                Err(error) => error_status(id, error),
            },
            Err(error) => error_status(id, error),
        },
        11 => {
            path_request(
                handler,
                id,
                &request.body,
                |mut handler, id, path| async move { handler.opendir(id, path).await },
            )
            .await
        }
        12 => {
            handle_request(
                handler,
                id,
                &request.body,
                |mut handler, id, handle| async move { handler.readdir(id, handle).await },
            )
            .await
        }
        13 => {
            path_request(
                handler,
                id,
                &request.body,
                |mut handler, id, path| async move { handler.remove(id, path).await },
            )
            .await
        }
        14 => match path_and_attrs(&request.body) {
            Ok((path, attrs)) => match handler.clone().mkdir(id, path, attrs).await {
                Ok(packet) => packet.into(),
                Err(error) => error_status(id, error),
            },
            Err(error) => error_status(id, error),
        },
        15 => {
            path_request(
                handler,
                id,
                &request.body,
                |mut handler, id, path| async move { handler.rmdir(id, path).await },
            )
            .await
        }
        16 => {
            path_request(
                handler,
                id,
                &request.body,
                |mut handler, id, path| async move { handler.realpath(id, path).await },
            )
            .await
        }
        17 => {
            path_request(
                handler,
                id,
                &request.body,
                |mut handler, id, path| async move { handler.stat(id, path).await },
            )
            .await
        }
        18 => match (|| -> Result<_, StatusReply> {
            let mut reader = RequestReader::new(&request.body);
            Ok((reader.string()?, reader.string()?))
        })() {
            Ok((from, to)) => match handler.clone().rename(id, from, to).await {
                Ok(packet) => packet.into(),
                Err(error) => error_status(id, error),
            },
            Err(error) => error_status(id, error),
        },
        19 => {
            path_request(
                handler,
                id,
                &request.body,
                |mut handler, id, path| async move { handler.readlink(id, path).await },
            )
            .await
        }
        20 => match (|| -> Result<_, StatusReply> {
            let mut reader = RequestReader::new(&request.body);
            Ok((reader.string()?, reader.string()?))
        })() {
            Ok((link, target)) => match handler.clone().symlink(id, link, target).await {
                Ok(packet) => packet.into(),
                Err(error) => error_status(id, error),
            },
            Err(error) => error_status(id, error),
        },
        200 => match (|| -> Result<_, StatusReply> {
            let mut reader = RequestReader::new(&request.body);
            Ok((reader.string()?, reader.remaining().to_vec()))
        })() {
            Ok((request_name, data)) => {
                match handler.clone().extended(id, request_name, data).await {
                    Ok(packet) => packet,
                    Err(error) => error_status(id, error),
                }
            }
            Err(error) => error_status(id, error),
        },
        _ => error_status(id, status(StatusCode::OpUnsupported)),
    };
    encode_response(packet, id)
}

async fn path_request<F, Fut, T>(handler: SftpHandler, id: u32, body: &[u8], call: F) -> Packet
where
    F: FnOnce(SftpHandler, u32, String) -> Fut,
    T: Into<Packet>,
    Fut: std::future::Future<Output = Result<T, StatusReply>>,
{
    match RequestReader::new(body).string() {
        Ok(path) => match call(handler, id, path).await {
            Ok(packet) => packet.into(),
            Err(error) => error_status(id, error),
        },
        Err(error) => error_status(id, error),
    }
}

async fn handle_request<F, Fut, T>(handler: SftpHandler, id: u32, body: &[u8], call: F) -> Packet
where
    F: FnOnce(SftpHandler, u32, String) -> Fut,
    T: Into<Packet>,
    Fut: std::future::Future<Output = Result<T, StatusReply>>,
{
    match RequestReader::new(body).string() {
        Ok(handle) => match call(handler, id, handle).await {
            Ok(packet) => packet.into(),
            Err(error) => error_status(id, error),
        },
        Err(error) => error_status(id, error),
    }
}

fn path_and_attrs(body: &[u8]) -> Result<(String, FileAttributes), StatusReply> {
    let mut reader = RequestReader::new(body);
    let path = reader.string()?;
    let attrs = reader.attrs()?;
    Ok((path, attrs))
}

fn handle_and_attrs(body: &[u8]) -> Result<(String, FileAttributes), StatusReply> {
    let mut reader = RequestReader::new(body);
    let handle = reader.string()?;
    let attrs = reader.attrs()?;
    Ok((handle, attrs))
}

struct RequestReader<'a> {
    body: &'a [u8],
}

impl<'a> RequestReader<'a> {
    fn new(body: &'a [u8]) -> Self {
        Self { body }
    }

    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (head, tail) = self.body.split_at_checked(count)?;
        self.body = tail;
        Some(head)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }

    fn string_bytes(&mut self) -> Result<Vec<u8>, StatusReply> {
        let length = self.u32().ok_or_else(|| status(StatusCode::BadMessage))?;
        Ok(self
            .take(length as usize)
            .ok_or_else(|| status(StatusCode::BadMessage))?
            .to_vec())
    }

    fn string(&mut self) -> Result<String, StatusReply> {
        let bytes = self.string_bytes()?;
        String::from_utf8(bytes).map_err(|_| status(StatusCode::BadMessage))
    }

    fn attrs(&mut self) -> Result<FileAttributes, StatusReply> {
        let flags = self.u32().ok_or_else(|| status(StatusCode::BadMessage))?;
        let mut attrs = FileAttributes::default();
        if flags & 0x0000_0001 != 0 {
            attrs.size = Some(self.u64().ok_or_else(|| status(StatusCode::BadMessage))?);
        }
        if flags & 0x0000_0002 != 0 {
            attrs.uid = Some(self.u32().ok_or_else(|| status(StatusCode::BadMessage))?);
            attrs.gid = Some(self.u32().ok_or_else(|| status(StatusCode::BadMessage))?);
        }
        if flags & 0x0000_0004 != 0 {
            attrs.permissions = Some(self.u32().ok_or_else(|| status(StatusCode::BadMessage))?);
        }
        if flags & 0x0000_0008 != 0 {
            attrs.atime = Some(self.u32().ok_or_else(|| status(StatusCode::BadMessage))?);
            attrs.mtime = Some(self.u32().ok_or_else(|| status(StatusCode::BadMessage))?);
        }
        if flags & 0x8000_0000 != 0 {
            let count = self.u32().ok_or_else(|| status(StatusCode::BadMessage))?;
            for _ in 0..count {
                let _ = self.string_bytes()?;
                let _ = self.string_bytes()?;
            }
        }
        Ok(attrs)
    }

    fn remaining(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.body)
    }
}

fn error_status(id: u32, error: StatusReply) -> Packet {
    Packet::Status(Status {
        id,
        status_code: error.status_code,
        error_message: error
            .error_message
            .unwrap_or_else(|| error.status_code.to_string()),
        language_tag: error.language_tag.unwrap_or_else(|| "en-US".to_owned()),
    })
}

fn encode_response(packet: Packet, id: u32) -> Response {
    let (kind, payload) = match packet {
        Packet::Version(packet) => (
            2,
            russh_sftp::ser::to_bytes(&packet)
                .ok()
                .map(|bytes| bytes.to_vec()),
        ),
        Packet::Status(packet) => (
            101,
            russh_sftp::ser::to_bytes(&packet)
                .ok()
                .map(|bytes| bytes.to_vec()),
        ),
        Packet::Handle(packet) => (
            102,
            russh_sftp::ser::to_bytes(&packet)
                .ok()
                .map(|bytes| bytes.to_vec()),
        ),
        Packet::Data(packet) => (
            103,
            russh_sftp::ser::to_bytes(&packet)
                .ok()
                .map(|bytes| bytes.to_vec()),
        ),
        Packet::Name(packet) => (
            104,
            russh_sftp::ser::to_bytes(&packet)
                .ok()
                .map(|bytes| bytes.to_vec()),
        ),
        Packet::Attrs(packet) => (
            105,
            russh_sftp::ser::to_bytes(&packet)
                .ok()
                .map(|bytes| bytes.to_vec()),
        ),
        Packet::ExtendedReply(packet) => (
            201,
            russh_sftp::ser::to_bytes(&packet)
                .ok()
                .map(|bytes| bytes.to_vec()),
        ),
        _ => (101, None),
    };
    Response {
        kind,
        id,
        payload: payload.unwrap_or_else(|| {
            encode_status(id, StatusCode::Failure, "failed to encode SFTP response")
        }),
    }
}

fn encode_status(id: u32, code: StatusCode, message: &str) -> Vec<u8> {
    let mut payload = Vec::with_capacity(16 + message.len());
    payload.extend_from_slice(&id.to_be_bytes());
    payload.extend_from_slice(&(code as u32).to_be_bytes());
    payload.extend_from_slice(&(message.len() as u32).to_be_bytes());
    payload.extend_from_slice(message.as_bytes());
    payload.extend_from_slice(&4u32.to_be_bytes());
    payload.extend_from_slice(b"en-US");
    payload
}

/// A loopback SSH server with a rooted SFTP subsystem.
pub struct SftpTestServer {
    address: SocketAddr,
    root: PathBuf,
    faults: Arc<SftpFaults>,
    latest_connection: Arc<Mutex<Option<Arc<SftpConnectionFaults>>>>,
    posix_rename: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl SftpTestServer {
    /// Change the mtime of a file under the server root without sending an
    /// SFTP request. Tests use this to simulate a concurrent remote change.
    pub fn set_file_mtime(&self, path: &str, mtime: u64) {
        let path = self.map_fixture_path(path);
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(mtime))
            .unwrap();
    }

    /// Advertise `posix-rename@openssh.com` during SFTP initialization. The
    /// server still implements a regular rename, which is enough for callers
    /// that only test extension detection and the fallback path.
    pub fn set_posix_rename_advertised(&self, advertised: bool) {
        self.posix_rename.store(advertised, Ordering::SeqCst);
    }

    fn map_fixture_path(&self, path: &str) -> PathBuf {
        match path.strip_prefix("/home/test/") {
            Some(relative) => self.root.join(relative),
            None => self.root.join(path.trim_start_matches('/')),
        }
    }
}

impl Drop for SftpTestServer {
    fn drop(&mut self) {
        self.task.abort();
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl SftpTestServer {
    /// Create a server rooted at `root`. `root` is removed when the server
    /// drops.
    pub async fn start(root: PathBuf) -> Self {
        Self::start_with_root(root, true).await
    }

    /// Create a server with a fresh unique temporary directory as its root.
    pub async fn start_temporary(label: &str) -> Self {
        Self::start(unique_temp_dir(label)).await
    }

    /// Start a server that rejects every `sftp` subsystem request.
    pub async fn without_sftp(root: PathBuf) -> Self {
        Self::start_with_root(root, false).await
    }

    async fn start_with_root(root: PathBuf, accept_sftp: bool) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let faults = Arc::new(SftpFaults::default());
        let latest_connection = Arc::new(Mutex::new(None));
        let task_root = root.clone();
        let task_faults = faults.clone();
        let task_latest = latest_connection.clone();
        let task_posix_rename = faults.posix_rename.clone();
        let server_posix_rename = task_posix_rename.clone();
        let task = tokio::spawn(async move {
            let config = Arc::new(server::Config {
                keys: vec![private(1)],
                methods: MethodSet::from(&[russh::MethodKind::PublicKey][..]),
                auth_rejection_time: Duration::from_millis(1),
                auth_rejection_time_initial: Some(Duration::from_millis(1)),
                max_auth_attempts: 20,
                channel_buffer_size: 16,
                event_buffer_size: 16,
                nodelay: true,
                ..Default::default()
            });
            let mut peers = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break; };
                        let connection_faults = task_faults.register();
                        *task_latest.lock().unwrap() = Some(connection_faults.clone());
                        let handler = SshHandler {
                            accept_sftp,
                            posix_rename: task_posix_rename.load(Ordering::SeqCst),
                            root: task_root.clone(),
                            faults: connection_faults,
                            channels: HashMap::new(),
                        };
                        let config = config.clone();
                        peers.spawn(async move {
                            if let Ok(peer) = server::run_stream(config, stream, handler).await {
                                let _ = peer.await;
                            }
                        });
                    }
                    _ = peers.join_next(), if !peers.is_empty() => {}
                }
            }
        });
        Self {
            address,
            root,
            faults,
            latest_connection,
            posix_rename: server_posix_rename,
            task,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn target(&self) -> SshTarget {
        SshTarget {
            host: "127.0.0.1".into(),
            port: self.address.port(),
            user: "test".into(),
            jump: None,
        }
    }

    pub async fn connect(&self) -> SshSession {
        self.try_connect().await.unwrap()
    }

    pub async fn try_connect(&self) -> Result<SshSession, rs_ssh::SshError> {
        SshSession::connect(
            self.target(),
            vec![credential(CLIENT_KEY_SEED)],
            Arc::new(MemoryHostKeyStore::default()),
            Arc::new(TestPrompter),
            ssh_options(),
        )
        .await
    }

    /// Disconnect the connection once the server has cumulatively read more
    /// than this many file bytes. `None` disables the fault.
    pub fn fail_after_read_bytes(&self, bytes: Option<u64>) {
        self.latest_connection_faults().fail_after_read_bytes(bytes);
    }

    /// Disconnect the connection once the server has cumulatively written more
    /// than this many file bytes. `None` disables the fault.
    pub fn fail_after_write_bytes(&self, bytes: Option<u64>) {
        self.latest_connection_faults()
            .fail_after_write_bytes(bytes);
    }

    /// Every server-side `read` request on the most recent connection, in
    /// arrival order.
    pub fn read_requests(&self) -> Vec<IoRecord> {
        self.latest_connection_faults().read_requests()
    }

    /// Every server-side `write` request on the most recent connection, in
    /// arrival order.
    pub fn write_requests(&self) -> Vec<IoRecord> {
        self.latest_connection_faults().write_requests()
    }

    /// Largest observed read request window on the most recent connection.
    pub fn max_in_flight_reads(&self) -> u64 {
        self.latest_connection_faults().max_in_flight_reads()
    }

    /// Largest observed write request window on the most recent connection.
    pub fn max_in_flight_writes(&self) -> u64 {
        self.latest_connection_faults().max_in_flight_writes()
    }

    /// Number of write acknowledgements intentionally discarded on the most
    /// recent connection.
    pub fn write_responses_dropped(&self) -> u64 {
        self.latest_connection_faults().write_responses_dropped()
    }

    /// Pass `before` write acknowledgements, then drop `writes` acknowledgements
    /// and disconnect the connection.
    pub fn drop_write_responses_after(&self, before: u64, writes: u64) {
        self.latest_connection_faults()
            .drop_write_responses_after(before, writes);
    }

    /// Delay read and write replies to make client-side pipelining observable.
    pub fn delay_io_responses(&self, delay: Option<Duration>) {
        self.latest_connection_faults().delay_io_responses(delay);
    }

    /// Fault hooks and request records for an individual SSH connection.
    pub fn connection_faults(&self) -> Vec<Arc<SftpConnectionFaults>> {
        self.faults.connections()
    }

    fn latest_connection_faults(&self) -> Arc<SftpConnectionFaults> {
        self.latest_connection
            .lock()
            .unwrap()
            .clone()
            .expect("SFTP test server has no accepted connection")
    }
}

struct RawClientHandler;

impl russh::client::Handler for RawClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// Raw high-level SFTP client used by fixture self-tests, including file I/O.
/// It opens its own SSH connection, so callers do not need production transfer
/// APIs that belong to.
pub struct RawSftp {
    handle: russh::client::Handle<RawClientHandler>,
    session: russh_sftp::client::SftpSession,
}

impl RawSftp {
    pub async fn connect(server: &SftpTestServer) -> Self {
        Self::connect_raw(server.address).await
    }

    async fn connect_raw(address: SocketAddr) -> Self {
        let config = Arc::new(russh::client::Config {
            channel_buffer_size: 16,
            nodelay: true,
            ..Default::default()
        });
        let mut handle = russh::client::connect(config, address, RawClientHandler)
            .await
            .unwrap();
        let auth = handle
            .authenticate_publickey(
                "test",
                russh::keys::PrivateKeyWithHashAlg::new(Arc::new(private(CLIENT_KEY_SEED)), None),
            )
            .await
            .unwrap();
        assert!(auth.success());

        let mut channel = handle.channel_open_session().await.unwrap();
        channel.request_subsystem(true, "sftp").await.unwrap();
        loop {
            match channel.wait().await {
                Some(russh::ChannelMsg::Success) => break,
                Some(russh::ChannelMsg::Failure) => panic!("sftp subsystem rejected"),
                Some(russh::ChannelMsg::Close) | None => panic!("sftp channel closed"),
                Some(_) => {}
            }
        }
        let session = russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .unwrap();
        Self { handle, session }
    }

    pub async fn create(
        &self,
        path: &str,
    ) -> Result<russh_sftp::client::fs::File, russh_sftp::client::error::Error> {
        self.session.create(path).await
    }

    pub async fn open_file(
        &self,
        path: &str,
    ) -> Result<russh_sftp::client::fs::File, russh_sftp::client::error::Error> {
        self.session.open(path).await
    }

    pub async fn stat(
        &self,
        path: &str,
    ) -> Result<russh_sftp::protocol::FileAttributes, russh_sftp::client::error::Error> {
        self.session.metadata(path).await
    }

    pub async fn lstat(
        &self,
        path: &str,
    ) -> Result<russh_sftp::protocol::FileAttributes, russh_sftp::client::error::Error> {
        self.session.symlink_metadata(path).await
    }

    pub async fn set_attrs(
        &self,
        path: &str,
        attrs: russh_sftp::protocol::FileAttributes,
    ) -> Result<(), russh_sftp::client::error::Error> {
        self.session.set_metadata(path, attrs).await
    }

    pub async fn list_len(&self, path: &str) -> Result<usize, russh_sftp::client::error::Error> {
        Ok(self.session.read_dir(path).await?.count())
    }

    pub async fn close(self) {
        let _ = self.session.close().await;
        let _ = self
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "")
            .await;
    }
}
