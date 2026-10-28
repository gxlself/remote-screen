//! Shared SSH connections, terminal authentication, and persistent host-key trust.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use rs_ssh::{Credential, HostKeyEntry, HostKeyStore, Prompter, SshError, SshOptions, SshSession};
use tokio::sync::{mpsc, oneshot};
use zeroize::{Zeroize, Zeroizing};

use super::dto_core::{
    id_str, CoreEvent, HostKeyPromptKind, SshAuth, SshServer, SshTarget as SshTargetDto,
    TrustedHostKey,
};
use super::error::CoreError;
use super::runtime::{core, Core};
use super::sshbook::{self, entry_to_row, row_to_entry, ResolvedTarget};
use super::tables::{SshEntry, TermTap};

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

pub struct CoreHostKeyStore {
    pub core: Arc<Core>,
    pub server_id: String,
}

impl HostKeyStore for CoreHostKeyStore {
    fn lookup(&self, host: &str, port: u16) -> Option<HostKeyEntry> {
        match self.core.store().known_host(host, port) {
            Ok(row) => row.as_ref().map(row_to_entry),
            Err(error) => {
                tracing::error!(%error, "SSH host-key lookup failed");
                None
            }
        }
    }

    fn save(&self, host: &str, port: u16, entry: HostKeyEntry) {
        let row = entry_to_row(&self.server_id, host, port, &entry, self.core.now_us());
        if let Err(error) = self.core.store().save_known_host(&row) {
            tracing::error!(%error, "SSH host-key save failed");
        }
    }
}

pub type FirstHostKeyPrompt = (String, String, oneshot::Sender<bool>);
pub type ChangedHostKey = (String, u16, String, String);

#[derive(Default)]
pub struct HostKeyPrompts {
    pub first: Mutex<HashMap<u64, FirstHostKeyPrompt>>,
    pub changed: Mutex<HashMap<String, Vec<ChangedHostKey>>>,
    pub approved: Mutex<HashSet<(String, String)>>,
}

/// Input is delivered only while a native authentication line is pending.
#[derive(Default)]
pub struct PromptRelay {
    pub pending: Mutex<Option<LineInput>>,
    pub reader: tokio::sync::Mutex<()>,
    pub keyboard_asked: AtomicBool,
}

pub struct LineInput {
    pub bytes: mpsc::UnboundedSender<Zeroizing<Vec<u8>>>,
    pub cancel: oneshot::Sender<()>,
}

/// Clears a pending line even if its read future is dropped during disconnect.
pub struct PendingLine<'a>(pub &'a PromptRelay);

impl Drop for PendingLine<'_> {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl PromptRelay {
    pub fn feed(&self, bytes: &[u8]) -> bool {
        lock(&self.pending)
            .as_ref()
            .is_some_and(|input| input.bytes.send(Zeroizing::new(bytes.to_vec())).is_ok())
    }

    pub fn cancel(&self) {
        lock(&self.pending).take();
    }

    pub async fn read_line(
        &self,
        term: &TermTap,
        prompt: &str,
        echo: bool,
        timeout: Duration,
    ) -> Option<Zeroizing<String>> {
        let _reader = self.reader.lock().await;
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let (cancel, mut cancelled) = oneshot::channel();
        *lock(&self.pending) = Some(LineInput {
            bytes: sender,
            cancel,
        });
        let _pending = PendingLine(self);
        term.push_output(prompt.as_bytes());
        tokio::time::timeout(timeout, async {
            let mut line = Zeroizing::new(String::with_capacity(1024));
            let mut utf8 = Zeroizing::new(Vec::with_capacity(4));
            loop {
                let bytes = tokio::select! {
                    biased;
                    _ = &mut cancelled => return None,
                    bytes = receiver.recv() => bytes?,
                };
                for &byte in bytes.iter() {
                    match byte {
                        b'\r' | b'\n' => {
                            term.push_output(b"\r\n");
                            return Some(line);
                        }
                        3 => return None,
                        8 | 127 => {
                            utf8.zeroize();
                            if line.pop().is_some() && echo {
                                term.push_output(b"\x08 \x08");
                            }
                        }
                        32..=126 | 128..=255 => {
                            if line.len() + utf8.len() >= 1024 {
                                continue;
                            }
                            utf8.push(byte);
                            match std::str::from_utf8(&utf8) {
                                Ok(text) => {
                                    for ch in text.chars().filter(|ch| !ch.is_control()) {
                                        line.push(ch);
                                        if echo {
                                            let mut encoded = [0; 4];
                                            term.push_output(
                                                ch.encode_utf8(&mut encoded).as_bytes(),
                                            );
                                            encoded.zeroize();
                                        }
                                    }
                                    utf8.zeroize();
                                }
                                Err(error) if error.error_len().is_some() => utf8.zeroize(),
                                Err(_) => {}
                            }
                        }
                        _ => {}
                    }
                }
            }
        })
        .await
        .ok()
        .flatten()
    }
}

pub const PASSWORD_CACHE_TTL_US: u64 = 10 * 60 * 1_000_000;
pub type CachedAnswers = (Vec<Zeroizing<String>>, u64);

#[derive(Default)]
pub struct PasswordCache {
    pub entries: Mutex<HashMap<String, CachedAnswers>>,
}

impl PasswordCache {
    pub fn put(&self, server_id: &str, answers: Vec<Zeroizing<String>>, now: u64) {
        lock(&self.entries).insert(server_id.to_owned(), (answers, now));
    }

    pub fn take_fresh(&self, server_id: &str, now: u64) -> Option<Vec<Zeroizing<String>>> {
        let mut entries = lock(&self.entries);
        let (answers, written) = entries.get(server_id)?;
        if now < *written || now - written >= PASSWORD_CACHE_TTL_US {
            entries.remove(server_id);
            None
        } else {
            Some(answers.clone())
        }
    }

    pub fn clear(&self, server_id: &str) {
        lock(&self.entries).remove(server_id);
    }
}

pub struct CorePrompter {
    pub core: Arc<Core>,
    pub ssh_id: Option<u64>,
    pub server_id: String,
    pub user: String,
    pub term: Option<Arc<SshEntry>>,
    pub relay: Arc<PromptRelay>,
}

impl CorePrompter {
    pub fn new(
        core: Arc<Core>,
        ssh_id: Option<u64>,
        server_id: String,
        user: String,
        term: Option<Arc<SshEntry>>,
        relay: Arc<PromptRelay>,
    ) -> Self {
        Self {
            core,
            ssh_id,
            server_id,
            user,
            term,
            relay,
        }
    }
}

/// An older connection may clean up only its own closed prompt.
pub struct PendingHostKey {
    pub prompts: Arc<HostKeyPrompts>,
    pub ssh_id: u64,
    pub receiver: oneshot::Receiver<bool>,
}

impl Drop for PendingHostKey {
    fn drop(&mut self) {
        self.receiver.close();
        let mut first = lock(&self.prompts.first);
        if first
            .get(&self.ssh_id)
            .is_some_and(|(_, _, tx)| tx.is_closed())
        {
            first.remove(&self.ssh_id);
        }
    }
}

#[async_trait]
impl Prompter for CorePrompter {
    async fn confirm_new_host_key(&self, host: &str, fp: &str, algo: &str) -> bool {
        let prompts = self.core.state::<HostKeyPrompts>();
        if lock(&prompts.approved).remove(&(host.to_owned(), fp.to_owned())) {
            return true;
        }
        let Some(ssh_id) = self.ssh_id else {
            return false;
        };
        let (sender, receiver) = oneshot::channel();
        lock(&prompts.first).insert(ssh_id, (self.server_id.clone(), fp.to_owned(), sender));
        let mut pending = PendingHostKey {
            prompts,
            ssh_id,
            receiver,
        };
        self.core.events().emit(&CoreEvent::SshHostKeyPrompt {
            ssh_id: id_str(ssh_id),
            server_id: self.server_id.clone(),
            key_type: algo.to_owned(),
            fp: fp.to_owned(),
            kind: HostKeyPromptKind::First,
            old_fp: None,
        });
        tokio::time::timeout(self.core.timeouts().hostkey_prompt, &mut pending.receiver)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(false)
    }

    async fn keyboard_interactive(
        &self,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<String>> {
        let cache = self.core.state::<PasswordCache>();
        if !self.relay.keyboard_asked.swap(true, Ordering::AcqRel) {
            if let Some(answers) = cache.take_fresh(&self.server_id, self.core.now_us()) {
                if answers.len() == prompts.len() {
                    return Some(answers.iter().map(|answer| answer.to_string()).collect());
                }
            }
        }
        let entry = self.term.as_ref()?;
        for text in [name, instructions]
            .into_iter()
            .filter(|text| !text.is_empty())
        {
            entry.term.push_output(text.as_bytes());
            entry.term.push_output(b"\r\n");
        }
        let mut answers = Vec::with_capacity(prompts.len());
        for (prompt, echo) in prompts {
            answers.push(
                self.relay
                    .read_line(
                        &entry.term,
                        prompt,
                        *echo,
                        self.core.timeouts().hostkey_prompt,
                    )
                    .await?,
            );
        }
        let result = answers.iter().map(|answer| answer.to_string()).collect();
        cache.put(&self.server_id, answers, self.core.now_us());
        Some(result)
    }

    fn auth_url(&self, url: &str) {
        if let Some(ssh_id) = self.ssh_id {
            self.core.events().emit(&CoreEvent::SshAuthUrl {
                ssh_id: id_str(ssh_id),
                server_id: self.server_id.clone(),
                url: url.to_owned(),
                user: self.user.clone(),
            });
        }
    }
}

#[derive(Default)]
pub struct SshBookCache(pub Mutex<Vec<SshServer>>);

/// The last `servers` namespace value the App pushed through the
/// `sync.servers` setting.
pub const SERVERS_SYNC_KV: &str = "ssh.servers.sync";

pub async fn servers(core: &Arc<Core>) -> Result<Vec<SshServer>, CoreError> {
    let value = core
        .store()
        .kv_get(SERVERS_SYNC_KV)?
        .and_then(|json| serde_json::from_str(&json).ok());
    let book = sshbook::ssh_servers(&sshbook::parse_servers_value(value.as_ref()));
    *lock(&core.state::<SshBookCache>().0) = book.clone();
    Ok(book)
}

pub async fn resolve_server(
    core: &Arc<Core>,
    server_id: &str,
    override_target: Option<&SshTargetDto>,
) -> Result<ResolvedTarget, CoreError> {
    let book = lock(&core.state::<SshBookCache>().0).clone();
    if let Ok(resolved) = sshbook::resolve(server_id, &book, override_target) {
        return Ok(resolved);
    }
    sshbook::resolve(server_id, &servers(core).await?, override_target).map_err(CoreError::internal)
}

pub fn credentials(core: &Arc<Core>, auth: SshAuth, server_id: &str) -> Vec<Credential> {
    match auth {
        SshAuth::Tailscale => Vec::new(),
        SshAuth::DeviceKey => core
            .signer()
            .map(|signer| {
                vec![
                    Credential::External(signer),
                    Credential::KeyboardInteractive,
                ]
            })
            .unwrap_or_default(),
        SshAuth::Password => {
            let mut creds = Vec::new();
            if let Some(answer) = core
                .state::<PasswordCache>()
                .take_fresh(server_id, core.now_us())
                .and_then(|answers| answers.into_iter().next())
            {
                creds.push(Credential::Password(answer));
            }
            creds.push(Credential::KeyboardInteractive);
            creds
        }
    }
}

pub async fn connect_once(
    core: &Arc<Core>,
    resolved: &ResolvedTarget,
    prompter: Arc<CorePrompter>,
) -> Result<SshSession, SshError> {
    let creds = credentials(core, resolved.auth, &resolved.server_id);
    connect_with(core, resolved, prompter, creds).await
}

/// Connect once with exactly `creds`; same options and host-key handling as `connect_once`.
pub async fn connect_with(
    core: &Arc<Core>,
    resolved: &ResolvedTarget,
    prompter: Arc<CorePrompter>,
    creds: Vec<Credential>,
) -> Result<SshSession, SshError> {
    prompter
        .relay
        .keyboard_asked
        .store(false, Ordering::Release);
    let options = SshOptions {
        net: core.net(),
        connect_timeout: Duration::from_secs(30).max(core.timeouts().hostkey_prompt),
        headscale_domains: super::api_sessions::settings_snapshot(core)
            .tailnet_control_url
            .and_then(|url| url::Url::parse(&url).ok())
            // rs-ssh accepts DNS hostnames here; IP URLs add no domain.
            .and_then(|url| match url.host() {
                Some(url::Host::Domain(host)) => Some(host.to_ascii_lowercase()),
                _ => None,
            })
            .into_iter()
            .collect(),
        ..Default::default()
    };
    let store = Arc::new(CoreHostKeyStore {
        core: core.clone(),
        server_id: resolved.server_id.clone(),
    });
    let result = SshSession::connect(
        resolved.target.clone(),
        creds,
        store.clone(),
        prompter.clone(),
        options,
    )
    .await;
    if let Err(SshError::HostKeyChanged { expected, got }) = &result {
        for target in std::iter::once(&resolved.target).chain(resolved.target.jump.as_deref()) {
            let Some(entry) = store
                .lookup(&target.host, target.port)
                .filter(|entry| &entry.fingerprint == expected)
            else {
                continue;
            };
            let prompts = core.state::<HostKeyPrompts>();
            let changed = (
                target.host.clone(),
                target.port,
                expected.clone(),
                got.clone(),
            );
            let mut records = lock(&prompts.changed);
            let records = records.entry(resolved.server_id.clone()).or_default();
            if !records.contains(&changed) {
                records.push(changed);
            }
            if let Some(ssh_id) = prompter.ssh_id {
                core.events().emit(&CoreEvent::SshHostKeyPrompt {
                    ssh_id: id_str(ssh_id),
                    server_id: resolved.server_id.clone(),
                    key_type: entry.algorithm,
                    fp: got.clone(),
                    kind: HostKeyPromptKind::Changed,
                    old_fp: Some(expected.clone()),
                });
            }
            break;
        }
    }
    result
}

pub fn trust_host_key(core: &Arc<Core>, server_id: &str, fp: &str) -> Result<(), CoreError> {
    let prompts = core.state::<HostKeyPrompts>();
    let mut first = lock(&prompts.first);
    let id = first.iter().find_map(|(id, (server, pending_fp, sender))| {
        (server == server_id && pending_fp == fp && !sender.is_closed()).then_some(*id)
    });
    if let Some(id) = id {
        if let Some((_, _, sender)) = first.remove(&id) {
            return sender
                .send(true)
                .map_err(|_| CoreError::internal("no pending host key"));
        }
    }
    drop(first);
    let mut changed = lock(&prompts.changed);
    if let Some(records) = changed.get_mut(server_id) {
        if let Some(index) = records.iter().position(|(_, _, _, new)| new == fp) {
            let (host, port, _, _) = &records[index];
            core.store().forget_known_host(host, *port)?;
            lock(&prompts.approved).insert((host.clone(), fp.to_owned()));
            records.remove(index);
            return Ok(());
        }
    }
    Err(CoreError::internal("no pending host key"))
}

pub fn trusted_host_keys(core: &Arc<Core>) -> Result<Vec<TrustedHostKey>, CoreError> {
    Ok(sshbook::trusted_host_keys(&core.store().known_hosts()?))
}

pub fn forget_host_key(core: &Arc<Core>, server_id: &str) -> Result<(), CoreError> {
    core.store().forget_known_hosts_for_server(server_id)?;
    lock(&core.state::<HostKeyPrompts>().changed).remove(server_id);
    core.state::<PasswordCache>().clear(server_id);
    Ok(())
}

pub fn reject_prompts(core: &Arc<Core>, ssh_id: u64) {
    lock(&core.state::<HostKeyPrompts>().first).remove(&ssh_id);
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn ssh_servers() -> Result<String, CoreError> {
    let c = core()?;
    c.clone()
        .run(async move { Ok(serde_json::to_string(&servers(&c).await?)?) })
        .await
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn ssh_trust_host_key(server_id: String, fp: String) -> Result<(), CoreError> {
    let c = core()?;
    c.clone()
        .run(async move { trust_host_key(&c, &server_id, &fp) })
        .await
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn ssh_trusted_host_keys() -> Result<String, CoreError> {
    let c = core()?;
    c.clone()
        .run(async move { Ok(serde_json::to_string(&trusted_host_keys(&c)?)?) })
        .await
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn ssh_forget_host_key(server_id: String) -> Result<(), CoreError> {
    let c = core()?;
    c.clone()
        .run(async move { forget_host_key(&c, &server_id) })
        .await
}
