# rs-ssh

App SSH transport built on russh 0.64. Terminal data stays in Rust as byte chunks;
no local `ssh`, `sshd`, or `tmux` process is used. The integration suite starts
russh peers on `127.0.0.1:0` and does not access external networks or databases.

## Public API

All of these types are exported at the crate root. `ExternalSigner` is the
unchanged trait from `rsp_transport::identity`.

```rust,ignore
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub jump: Option<Box<SshTarget>>,
}

pub enum Credential {
    External(Arc<dyn ExternalSigner>),
    Ed25519 { secret: Zeroizing<[u8; 32]> },
    Password(Zeroizing<String>),
    KeyboardInteractive,
}
impl Credential {
    pub fn from_openssh(pem: &str) -> Result<Self, SshError>;
}
pub fn public_key_openssh(cred: &Credential, comment: &str) -> Result<String, SshError>;

pub struct HostKeyEntry {
    pub algorithm: String,
    pub key: Vec<u8>, // complete SSH wire public key
    pub fingerprint: String, // SHA256: + unpadded base64
}
pub trait HostKeyStore: Send + Sync {
    fn lookup(&self, host: &str, port: u16) -> Option<HostKeyEntry>;
    fn save(&self, host: &str, port: u16, entry: HostKeyEntry);
}

#[async_trait]
pub trait Prompter: Send + Sync {
    async fn confirm_new_host_key(&self, host: &str, fp_sha256: &str, algo: &str) -> bool;
    async fn keyboard_interactive(
        &self, name: &str, instructions: &str, prompts: &[(String, bool)],
    ) -> Option<Vec<String>>;
    fn auth_url(&self, url: &str);
}

impl SshSession {
    pub async fn connect(
        target: SshTarget, creds: Vec<Credential>,
        store: Arc<dyn HostKeyStore>, prompter: Arc<dyn Prompter>, opts: SshOptions,
    ) -> Result<Self, SshError>;
    pub async fn open_shell(&self, opts: PtyOpts) -> Result<Shell, SshError>;
    pub async fn exec(&self, cmd: &str) -> Result<ExecOutput, SshError>;
    pub async fn close(self);
    pub fn state(&self) -> SshState;
    pub fn subscribe_state(&self) -> watch::Receiver<SshState>;
    pub fn events(&self) -> broadcast::Receiver<SshEvent>;
    pub async fn closed(&self) -> Result<(), SshError>;
}
pub struct PtyOpts {
    pub term: String, pub cols: u32, pub rows: u32, pub tmux: Option<String>,
}
pub struct ExecOutput {
    pub status: Option<u32>, pub stdout: Vec<u8>, pub stderr: Vec<u8>,
}
impl Shell {
    pub async fn write(&self, bytes: &[u8]) -> Result<(), SshError>;
    pub async fn resize(&self, cols: u32, rows: u32) -> Result<(), SshError>;
    pub fn output(&mut self) -> &mut mpsc::Receiver<Vec<u8>>;
    pub async fn closed(&mut self) -> Option<u32>;
    pub async fn result(&mut self) -> Result<Option<u32>, SshError>;
}
```

`write` and `resize` return errors so the terminal bridge can report disconnects.
Missing exit status is represented by `None`. `Shell::result` preserves transport
errors; `SshSession::closed` reports keepalive `SshError::Timeout` for either leg.
`SshState` is `Connecting | Connected | Closed | Failed(SshError)`;
`SshEvent` is `StateChanged(SshState) | TmuxUnavailable`. Subscribe before opening
a shell to observe tmux fallback. State also has a watch channel for consumers
that must always see the latest state.

## Options and integration

`SshOptions::default()` uses 30 s connect/request timeouts, a 5 minute total
authentication timeout per leg, 15 s keepalive intervals, 3 unanswered keepalives,
and 16 output chunks. All durations, keepalive count and output capacity must be
positive. `headscale_domains` adds exact DNS hostnames (e.g. `hs.example.com`),
without wildcards or URL prefixes, to the built-in `login.tailscale.com` allowlist.
Only HTTPS links in authentication banners are forwarded. Banner text, approval
URLs, terminal input, private keys, passwords and challenge answers are not logged.
Credential `Debug` redacts key material and passwords.

Credentials are attempted in caller order using methods the server advertises.
The initial `none` request discovers methods and handles Tailscale SSH browser
approval. Cancellation of a keyboard-interactive prompt disconnects. RSA user
keys and RSA host keys return `Unsupported`; the `rsa` feature is never enabled.
Only unencrypted Ed25519 OpenSSH private keys can be imported; P-256 keys use
`ExternalSigner`, with signing performed on a blocking worker.

`jump_credentials: Some(...)` supplies separate jump-host credentials; `None`
reuses the target's credentials. Both legs perform independent host-key checks.
Nested jumps are rejected before connecting. Credentials are dropped after
connection establishment and are not retained in session options.

A16 supplies the SQLite `HostKeyStore` implementation; `MemoryHostKeyStore` is
available for ephemeral use/tests. Store entries are keyed by the supplied host
and port and compare full wire key bytes. Changed keys fail before authentication
and never invoke the confirmation callback or replace the stored entry.

`PtyOpts::default()` is `xterm-256color`, 80×24, no tmux. Output is a bounded,
lossless byte stream: the App must continue draining `output()` while waiting
for shell completion. An output consumer that stops reading backpressures the
transport. Terminal stderr shares the output stream; `exec` separates stdout and
stderr. Dropping a shell closes its channel. `close(self)` closes both SSH legs.

A tmux ID must match `[A-Za-z0-9_-]{1,32}` and produces exactly
`tmux new -A -s rs-<id>`. Status 127 emits `TmuxUnavailable` and opens a fresh PTY
with a regular shell. tmux errors remain visible in terminal output. `sftp` is a
reserved M2 module.
