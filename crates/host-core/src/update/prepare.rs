//! 检查、下载与校验。

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;

use crate::audit::{AuditEvent, AuditQueue};
use crate::clock::Clock;

use super::manifest::{
    check_applicable, verify_manifest, verify_package_file, ManifestError, PackageError,
    TrustedKeys, VerifiedManifest,
};
use super::status::{Phase, StatusTx, UpdateStatus};
use super::types::{Arch, Channel, HostOs};

/// Default per-package download bound. The protocol does not prescribe one.
pub const DEFAULT_MAX_PACKAGE_BYTES: u64 = 1024 * 1024 * 1024;

const DOWNLOAD_ATTEMPTS: usize = 3;
const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(10), Duration::from_secs(30)];
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    Periodic,
}

#[async_trait]
pub trait ReleaseSource: Send + Sync {
    /// Return the exact bytes of the signed release manifest; signature
    /// verification covers those bytes. No available release is `Ok(None)`.
    async fn fetch(
        &self,
        channel: Channel,
        os: HostOs,
        arch: Arch,
        from: &str,
    ) -> Result<Option<Vec<u8>>, String>;
}

#[async_trait]
pub trait Downloader: Send + Sync {
    /// Write to `dest`, stopping with `Err` once more than `max_bytes` would
    /// be written.
    async fn download(&self, url: &str, dest: &Path, max_bytes: u64) -> Result<(), String>;
}

/// Step 4: verify the platform code signature and signing identity.
pub trait PackageVerifier: Send + Sync {
    fn verify_platform(&self, package: &Path) -> Result<(), String>;
}

pub struct PrepareContext {
    pub current_version: String,
    pub os: HostOs,
    pub arch: Arch,
    /// Usually `<state dir>/update`.
    pub work_dir: PathBuf,
    pub keys: TrustedKeys,
    pub max_package_bytes: u64,
}

pub struct PrepareDeps {
    pub source: Arc<dyn ReleaseSource>,
    pub downloader: Arc<dyn Downloader>,
    pub verifier: Arc<dyn PackageVerifier>,
    pub audit: Option<AuditQueue>,
    pub clock: Arc<dyn Clock>,
    pub status: StatusTx,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub manifest: VerifiedManifest,
    pub package: PathBuf,
    pub trigger: Trigger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailReason {
    Fetch,
    Manifest,
    Signature,
    NotApplicable,
    Download,
    Size,
    Sha256,
    PlatformSignature,
}

impl FailReason {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Fetch => "fetch",
            Self::Manifest => "manifest",
            Self::Signature => "signature",
            Self::NotApplicable => "not_applicable",
            Self::Download => "download",
            Self::Size => "size",
            Self::Sha256 => "sha256",
            Self::PlatformSignature => "platform_signature",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::Fetch => "无法获取更新清单",
            Self::Manifest => "更新清单格式无效",
            Self::Signature => "更新清单签名无效",
            Self::NotApplicable => "更新清单不适用于本机",
            Self::Download => "下载失败",
            Self::Size => "安装包大小不符",
            Self::Sha256 => "安装包校验失败",
            Self::PlatformSignature => "安装包签名不符",
        }
    }

    fn is_failed_phase(self) -> bool {
        matches!(
            self,
            Self::Manifest
                | Self::Signature
                | Self::Download
                | Self::Size
                | Self::Sha256
                | Self::PlatformSignature
        )
    }

    fn audit_phase(self) -> Option<&'static str> {
        match self {
            Self::Manifest
            | Self::Signature
            | Self::Size
            | Self::Sha256
            | Self::PlatformSignature => Some("rejected"),
            Self::Download => Some("failed"),
            Self::Fetch | Self::NotApplicable => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum PrepareOutcome {
    Disabled,
    NoUpdate,
    Skipped { version: String },
    Prepared(Prepared),
    Failed(FailReason),
}

pub struct Preparer {
    ctx: PrepareContext,
    deps: PrepareDeps,
}

impl Preparer {
    pub fn new(ctx: PrepareContext, deps: PrepareDeps) -> Self {
        Self { ctx, deps }
    }

    pub async fn prepare(&self, trigger: &Trigger, channel: Channel) -> PrepareOutcome {
        // Clear any old failure before networking.
        self.set_status(UpdateStatus {
            phase: Phase::Checking,
            channel: channel.clone(),
            pending: None,
            notes: None,
            error: None,
        });
        tracing::info!(
            event = "upgrade.check",
            result = "checking",
            "checking for updates"
        );

        if self.ctx.keys.is_empty() {
            tracing::warn!(
                event = "upgrade.check",
                result = "disabled",
                "release keys are empty"
            );
            self.set_status(UpdateStatus::idle(channel));
            return PrepareOutcome::Disabled;
        }

        let raw = match self
            .deps
            .source
            .fetch(
                channel.clone(),
                self.ctx.os.clone(),
                self.ctx.arch.clone(),
                &self.ctx.current_version,
            )
            .await
        {
            Ok(Some(raw)) => raw,
            Ok(None) => {
                tracing::info!(
                    event = "upgrade.check",
                    result = "no_update",
                    "no release available"
                );
                self.set_status(UpdateStatus::idle(channel));
                return PrepareOutcome::NoUpdate;
            }
            Err(_) => {
                tracing::warn!(
                    event = "upgrade.check",
                    result = "fetch",
                    "could not fetch release manifest"
                );
                return self.failure(FailReason::Fetch, None, channel, None, None);
            }
        };
        if raw.len() > MAX_MANIFEST_BYTES {
            tracing::warn!(
                event = "upgrade.check",
                result = "manifest",
                "release manifest exceeds the size limit"
            );
            return self.failure(FailReason::Manifest, None, channel, None, None);
        }

        let manifest = match verify_manifest(&raw, &self.ctx.keys) {
            Ok(manifest) => manifest,
            Err(ManifestError::Signature) => {
                tracing::warn!(
                    event = "upgrade.check",
                    result = "signature",
                    "release signature rejected"
                );
                return self.failure(FailReason::Signature, None, channel, None, None);
            }
            Err(ManifestError::NoTrustedKeys) => {
                // The earlier empty-key check normally makes this unreachable.
                tracing::warn!(
                    event = "upgrade.check",
                    result = "disabled",
                    "release keys are empty"
                );
                self.set_status(UpdateStatus::idle(channel));
                return PrepareOutcome::Disabled;
            }
            Err(ManifestError::NotApplicable(_)) => {
                tracing::warn!(
                    event = "upgrade.check",
                    result = "not_applicable",
                    "release manifest is not applicable"
                );
                return self.failure(FailReason::NotApplicable, None, channel, None, None);
            }
            Err(ManifestError::Invalid(_)) => {
                tracing::warn!(
                    event = "upgrade.check",
                    result = "manifest",
                    "release manifest is invalid"
                );
                return self.failure(FailReason::Manifest, None, channel, None, None);
            }
        };

        if check_applicable(
            &manifest,
            &self.ctx.current_version,
            &channel,
            &self.ctx.os,
            &self.ctx.arch,
        )
        .is_err()
        {
            tracing::warn!(event = "upgrade.check", version = %manifest.version, result = "not_applicable", "release is not applicable");
            return self.failure(FailReason::NotApplicable, None, channel, None, None);
        }

        if skipped_versions(&self.ctx.work_dir)
            .iter()
            .any(|skipped| skipped == &manifest.version)
        {
            tracing::info!(event = "upgrade.check", version = %manifest.version, result = "skipped", "release version is skipped");
            self.set_status(UpdateStatus::idle(channel));
            return PrepareOutcome::Skipped {
                version: manifest.version,
            };
        }

        self.set_status(UpdateStatus {
            phase: Phase::Checking,
            channel: channel.clone(),
            pending: Some(manifest.version.clone()),
            notes: Some(manifest.notes.clone()),
            error: None,
        });
        tracing::info!(event = "upgrade.check", version = %manifest.version, result = "ok", "release manifest verified");

        if manifest.size > self.ctx.max_package_bytes {
            tracing::warn!(event = "upgrade.verify", version = %manifest.version, result = "size", "release exceeds the package size limit");
            return self.failure(
                FailReason::Size,
                Some(manifest.version.clone()),
                channel,
                Some(manifest.version.clone()),
                None,
            );
        }

        let download_dir = self.ctx.work_dir.join("download");
        let package = download_dir
            .join(&manifest.version)
            .join(format!("package{}", package_extension(&manifest.url)));
        let part = package.with_extension(format!(
            "{}.part",
            package
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("bin")
        ));

        if let Err(error) = self.prepare_download_dir(&download_dir, &manifest.version) {
            tracing::warn!(event = "upgrade.download", version = %manifest.version, result = "io", "could not prepare the download directory: {error}");
            return self.failure(
                FailReason::Download,
                Some(manifest.version.clone()),
                channel,
                Some(manifest.version.clone()),
                None,
            );
        }

        let reused = package.is_file() && verify_package_file(&package, &manifest).is_ok();
        if reused {
            // A stale partial download must not shadow the already verified
            // final package in the verification step below.
            remove_if_exists(&part);
            tracing::info!(event = "upgrade.download", version = %manifest.version, result = "reused", "reusing verified package");
        } else {
            remove_if_exists(&package);
            self.set_status(UpdateStatus {
                phase: Phase::Downloading,
                channel: channel.clone(),
                pending: Some(manifest.version.clone()),
                notes: Some(manifest.notes.clone()),
                error: None,
            });
            if !self.download_package(&manifest, &part, &package).await {
                return self.failure(
                    FailReason::Download,
                    Some(manifest.version.clone()),
                    channel,
                    Some(manifest.version.clone()),
                    None,
                );
            }
        }

        self.set_status(UpdateStatus {
            phase: Phase::Verifying,
            channel: channel.clone(),
            pending: Some(manifest.version.clone()),
            notes: Some(manifest.notes.clone()),
            error: None,
        });

        // Downloads go to `.part`; only a package that also passed the size
        // and digest check is promoted to its final name.
        let verifying_part = part.is_file();
        if verifying_part {
            match verify_package_file(&part, &manifest) {
                Ok(()) => {
                    if let Err(error) = fs::rename(&part, &package) {
                        tracing::warn!(event = "upgrade.verify", version = %manifest.version, result = "io", "could not promote package: {error}");
                        remove_if_exists(&part);
                        return self.failure(
                            FailReason::Download,
                            Some(manifest.version.clone()),
                            channel,
                            Some(manifest.version.clone()),
                            None,
                        );
                    }
                }
                Err(PackageError::Size { .. }) => {
                    remove_if_exists(&part);
                    tracing::warn!(event = "upgrade.verify", version = %manifest.version, result = "size", "package size mismatch");
                    return self.failure(
                        FailReason::Size,
                        Some(manifest.version.clone()),
                        channel,
                        Some(manifest.version.clone()),
                        None,
                    );
                }
                Err(PackageError::Sha256) => {
                    remove_if_exists(&part);
                    tracing::warn!(event = "upgrade.verify", version = %manifest.version, result = "sha256", "package digest mismatch");
                    return self.failure(
                        FailReason::Sha256,
                        Some(manifest.version.clone()),
                        channel,
                        Some(manifest.version.clone()),
                        None,
                    );
                }
                Err(PackageError::Io(_)) => {
                    remove_if_exists(&part);
                    tracing::warn!(event = "upgrade.verify", version = %manifest.version, result = "io", "could not read package");
                    return self.failure(
                        FailReason::Download,
                        Some(manifest.version.clone()),
                        channel,
                        Some(manifest.version.clone()),
                        None,
                    );
                }
            }
        } else if !package.is_file() || verify_package_file(&package, &manifest).is_err() {
            return self.failure(
                FailReason::Download,
                Some(manifest.version.clone()),
                channel,
                Some(manifest.version.clone()),
                None,
            );
        }

        let verifier = Arc::clone(&self.deps.verifier);
        let package_for_verifier = package.clone();
        let verified =
            tokio::task::spawn_blocking(move || verifier.verify_platform(&package_for_verifier))
                .await;
        let platform_ok = matches!(verified, Ok(Ok(())));
        if !platform_ok {
            remove_if_exists(&package);
            tracing::warn!(event = "upgrade.verify", version = %manifest.version, result = "platform_signature", "platform signature rejected");
            return self.failure(
                FailReason::PlatformSignature,
                Some(manifest.version.clone()),
                channel,
                Some(manifest.version.clone()),
                Some(manifest.notes.clone()),
            );
        }

        tracing::info!(event = "upgrade.verify", version = %manifest.version, result = "ok", "package verified");
        PrepareOutcome::Prepared(Prepared {
            manifest,
            package,
            trigger: trigger.clone(),
        })
    }

    fn set_status(&self, status: UpdateStatus) {
        self.deps.status.send_replace(status);
    }

    fn failure(
        &self,
        reason: FailReason,
        pending: Option<String>,
        channel: Channel,
        audit_version: Option<String>,
        notes: Option<String>,
    ) -> PrepareOutcome {
        let phase = if reason.is_failed_phase() {
            Phase::Failed
        } else {
            Phase::Idle
        };
        self.set_status(UpdateStatus {
            phase,
            channel,
            pending: if reason.is_failed_phase() {
                pending
            } else {
                None
            },
            notes: if reason.is_failed_phase() {
                notes
            } else {
                None
            },
            error: if reason.is_failed_phase() {
                Some(reason.message().to_owned())
            } else {
                None
            },
        });
        if let Some(phase) = reason.audit_phase() {
            self.audit(phase, reason.code(), audit_version.as_deref());
        }
        PrepareOutcome::Failed(reason)
    }

    fn audit(&self, phase: &str, reason: &str, version: Option<&str>) {
        let Some(queue) = &self.deps.audit else {
            return;
        };
        let event = AuditEvent {
            ts: self.deps.clock.now_micros(),
            user: None,
            device_id: None,
            host_id: None,
            server: None,
            event: "upgrade".to_owned(),
            path: None,
            duration_s: None,
            detail: Some(json!({
                "phase": phase,
                "version": version,
                "reason": reason,
            })),
        };
        if let Err(error) = queue.push(event) {
            tracing::warn!(
                event = "upgrade.rejected",
                result = "audit_error",
                "could not queue upgrade audit: {error}"
            );
        }
    }

    fn prepare_download_dir(&self, download_dir: &Path, version: &str) -> io::Result<()> {
        fs::create_dir_all(download_dir.join(version))?;
        for entry in fs::read_dir(download_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            if entry.file_name().to_string_lossy() == version {
                continue;
            }
            if let Err(error) = fs::remove_dir_all(entry.path()) {
                tracing::warn!(
                    event = "upgrade.download",
                    result = "cleanup_error",
                    "could not remove an old download: {error}"
                );
            }
        }
        Ok(())
    }

    async fn download_package(
        &self,
        manifest: &VerifiedManifest,
        part: &Path,
        package: &Path,
    ) -> bool {
        for attempt in 0..DOWNLOAD_ATTEMPTS {
            remove_if_exists(part);
            match self
                .deps
                .downloader
                .download(&manifest.url, part, self.ctx.max_package_bytes)
                .await
            {
                Ok(()) => {
                    tracing::info!(event = "upgrade.download", version = %manifest.version, attempt = attempt + 1, result = "ok", "package downloaded");
                    if part.is_file() {
                        return true;
                    }
                }
                Err(_) => {
                    tracing::warn!(event = "upgrade.download", version = %manifest.version, attempt = attempt + 1, result = "failed", "download attempt failed");
                }
            }
            if let Some(delay) = RETRY_DELAYS.get(attempt) {
                tokio::time::sleep(*delay).await;
            }
        }
        remove_if_exists(part);
        remove_if_exists(package);
        false
    }
}

fn package_extension(url: &str) -> &'static str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last = path.rsplit('/').next().unwrap_or_default();
    match Path::new(last)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("dmg") => ".dmg",
        Some("zip") => ".zip",
        Some("pkg") => ".pkg",
        Some("msi") => ".msi",
        _ => ".bin",
    }
}

fn remove_if_exists(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(
                event = "upgrade.download",
                result = "cleanup_error",
                "could not remove a package file: {error}"
            );
        }
    }
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.as_os_str().is_empty() {
        fs::create_dir_all(parent)?;
    }
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let temp = path.with_file_name(format!(
        ".{}.{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub fn skipped_versions(work_dir: &Path) -> Vec<String> {
    let path = work_dir.join("skip_versions.json");
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

pub fn add_skipped_version(work_dir: &Path, version: &str) -> std::io::Result<()> {
    let mut versions = skipped_versions(work_dir);
    if versions.iter().any(|existing| existing == version) {
        return Ok(());
    }
    versions.push(version.to_owned());
    let bytes = serde_json::to_vec(&versions).map_err(io::Error::other)?;
    write_atomic(&work_dir.join("skip_versions.json"), &bytes)
}
