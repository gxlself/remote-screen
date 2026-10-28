//! AppImage installation, verification, and update wiring for Linux hosts.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use host_core::update::agent::UpdatePlatform;
use host_core::update::install::Installer;
use host_core::update::prepare::PackageVerifier;
use host_core::update::remote::CurlDownloader;

pub const APP_PREFIX: &str = "app-";
pub const CURRENT_LINK: &str = "current";
pub const VERSION_FILE: &str = "usr/share/remote-host/VERSION";
pub const HOST_UNIT: &str = "remote-host.service";
pub const NOTIFY_UNIT: &str = "remote-host-notify.service";

/// Extract an AppImage into a caller-provided staging directory.
pub trait Extractor: Send + Sync {
    fn extract(&self, appimage: &Path, into: &Path) -> Result<PathBuf, String>;
}

/// AppImage type-2 runtime extractor.
#[derive(Debug, Clone, Copy, Default)]
pub struct RuntimeExtractor;

impl Extractor for RuntimeExtractor {
    fn extract(&self, appimage: &Path, into: &Path) -> Result<PathBuf, String> {
        fs::create_dir_all(into)
            .map_err(|error| format!("could not prepare extractor: {error}"))?;
        let copied = into.join("package.AppImage");
        fs::copy(appimage, &copied)
            .map_err(|error| format!("could not stage AppImage: {error}"))?;
        set_executable(&copied)
            .map_err(|error| format!("could not make staged AppImage executable: {error}"))?;

        let mut child = Command::new("./package.AppImage")
            .arg("--appimage-extract")
            .current_dir(into)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("could not run AppImage extractor: {error}"))?;
        let mut stderr_reader = child
            .stderr
            .take()
            .map(|stderr| std::thread::spawn(move || read_short(stderr)));
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        let stderr = stderr_reader
                            .take()
                            .and_then(|reader| reader.join().ok())
                            .unwrap_or_else(|| String::from("extractor failed"));
                        let reason = format!("AppImage extraction failed: {stderr}");
                        return Err(reason.chars().take(200).collect());
                    }
                    let _ = stderr_reader.take().and_then(|reader| reader.join().ok());
                    let root = into.join("squashfs-root");
                    if !root.is_dir() {
                        return Err("AppImage extraction did not produce squashfs-root".to_owned());
                    }
                    return Ok(root);
                }
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stderr_reader.take().and_then(|reader| reader.join().ok());
                    return Err("AppImage extraction timed out".to_owned());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stderr_reader.take().and_then(|reader| reader.join().ok());
                    return Err(format!("could not wait for AppImage extractor: {error}"));
                }
            }
        }
    }
}

/// Service operation used by the update installer.
pub trait ServiceControl: Send + Sync {
    fn restart_host(&self) -> Result<(), String>;
}

/// Restart the user service through systemd.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemdUser;

impl ServiceControl for SystemdUser {
    fn restart_host(&self) -> Result<(), String> {
        let output = Command::new("systemctl")
            .args(["--user", "--no-block", "restart", HOST_UNIT])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("could not run systemctl: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "systemctl --user restart failed: {}",
                short_utf8(&output.stderr)
            ))
        }
    }
}

/// Install an extracted AppImage and atomically move the `current` link.
pub struct AppImageInstaller<E: Extractor, S: ServiceControl> {
    pub lib_dir: PathBuf,
    pub extractor: E,
    pub service: S,
}

impl<E: Extractor, S: ServiceControl> AppImageInstaller<E, S> {
    pub fn new(lib_dir: PathBuf, extractor: E, service: S) -> Self {
        Self {
            lib_dir,
            extractor,
            service,
        }
    }
}

impl<E: Extractor + 'static, S: ServiceControl + 'static> Installer for AppImageInstaller<E, S> {
    fn install(&self, package: &Path, backup_dir: &Path) -> Result<(), String> {
        let previous = current_target(&self.lib_dir)
            .ok_or_else(|| "not an installed AppImage layout".to_owned())?;
        let staging = self.lib_dir.join(format!("staging-{}", std::process::id()));
        let mut installed_target: Option<PathBuf> = None;
        let mut switched = false;
        let result = (|| {
            remove_path(&staging).map_err(|error| format!("could not clear staging: {error}"))?;
            fs::create_dir_all(&staging)
                .map_err(|error| format!("could not create staging: {error}"))?;
            let root = self.extractor.extract(package, &staging)?;
            let version = check_payload(&root)?;
            let target_name = format!("{APP_PREFIX}{version}");
            if target_name == previous {
                return Err("same version is already installed".to_owned());
            }
            let target = self.lib_dir.join(&target_name);
            if fs::symlink_metadata(&target).is_ok() {
                remove_path(&target)
                    .map_err(|error| format!("could not remove old package: {error}"))?;
            }
            fs::rename(&root, &target)
                .map_err(|error| format!("could not install package: {error}"))?;
            installed_target = Some(target.clone());

            fs::create_dir_all(backup_dir)
                .map_err(|error| format!("could not prepare rollback data: {error}"))?;
            write_atomic(&backup_dir.join("previous"), previous.as_bytes())
                .map_err(|error| format!("could not save rollback data: {error}"))?;
            switch_current(&self.lib_dir, &target_name)
                .map_err(|error| format!("could not switch current package: {error}"))?;
            switched = true;
            remove_path(&staging).map_err(|error| format!("could not clean staging: {error}"))?;
            prune(&self.lib_dir, &[&target_name, &previous]);
            tracing::info!(
                event = "upgrade.install",
                from = %previous,
                to = %version,
                result = "ok",
                "AppImage installed"
            );
            Ok(())
        })();

        if let Err(error) = &result {
            if switched {
                let _ = switch_current(&self.lib_dir, &previous);
            }
            let _ = remove_path(&staging);
            if let Some(target) = installed_target {
                let _ = remove_path(&target);
            }
            tracing::warn!(event = "upgrade.install", result = "failed", reason = %error);
        }
        result
    }

    fn restart(&self) -> Result<(), String> {
        self.service.restart_host()
    }

    fn rollback(&self, backup_dir: &Path) -> Result<(), String> {
        let previous = fs::read_to_string(backup_dir.join("previous"))
            .map_err(|_| "rollback record is missing".to_owned())?;
        let previous = previous.trim();
        if !valid_app_name(previous) || !self.lib_dir.join(previous).is_dir() {
            return Err("rollback package is missing".to_owned());
        }
        switch_current(&self.lib_dir, previous)
            .map_err(|error| format!("could not restore previous package: {error}"))
    }
}

/// Validate a staged AppDir and return its version.
pub fn check_payload(root: &Path) -> Result<String, String> {
    let version = fs::read_to_string(root.join(VERSION_FILE))
        .map_err(|_| "payload VERSION is missing".to_owned())?;
    let version = version.trim().to_owned();
    if version.is_empty()
        || version.len() > 64
        || !version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-'))
    {
        return Err("payload VERSION is invalid".to_owned());
    }
    for relative in ["AppRun", "usr/bin/rs-host", "usr/bin/remote-host-ctl"] {
        let path = root.join(relative);
        let metadata =
            fs::metadata(&path).map_err(|_| format!("payload file is missing: {relative}"))?;
        if !metadata.is_file() || !is_executable(&metadata) {
            return Err(format!("payload file is not executable: {relative}"));
        }
    }
    Ok(version)
}

/// Atomically replace the current link with a relative link to `target_name`.
pub fn switch_current(lib_dir: &Path, target_name: &str) -> io::Result<()> {
    if !valid_app_name(target_name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid AppImage target name",
        ));
    }
    let temporary = lib_dir.join(format!("{CURRENT_LINK}.new"));
    let current = lib_dir.join(CURRENT_LINK);
    let _ = remove_path(&temporary);
    std::os::unix::fs::symlink(target_name, &temporary)?;
    match fs::rename(&temporary, &current) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

/// Return the valid app directory named by `current`.
pub fn current_target(lib_dir: &Path) -> Option<String> {
    let link = fs::read_link(lib_dir.join(CURRENT_LINK)).ok()?;
    if link.components().count() != 1 {
        return None;
    }
    let name = link.to_str()?.to_owned();
    if !valid_app_name(&name) || !lib_dir.join(&name).is_dir() {
        return None;
    }
    Some(name)
}

/// Remove old app directories while retaining the requested names.
pub fn prune(lib_dir: &Path, keep: &[&str]) {
    let entries = match fs::read_dir(lib_dir) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(event = "upgrade.install", result = "prune_failed", reason = %error);
            return;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(APP_PREFIX) || keep.contains(&name) {
            continue;
        }
        let path = entry.path();
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            if let Err(error) = fs::remove_dir_all(path) {
                tracing::warn!(event = "upgrade.install", result = "prune_failed", reason = %error);
            }
        }
    }
}

pub struct AppImageVerifier {
    pub machine: u16,
}

impl AppImageVerifier {
    pub fn current() -> Self {
        #[cfg(target_arch = "x86_64")]
        let machine = 62;
        #[cfg(target_arch = "aarch64")]
        let machine = 183;
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        let machine = 0;
        Self { machine }
    }
}

impl PackageVerifier for AppImageVerifier {
    fn verify_platform(&self, package: &Path) -> Result<(), String> {
        let mut bytes = [0u8; 20];
        let mut file =
            fs::File::open(package).map_err(|error| format!("could not read package: {error}"))?;
        let mut read = 0;
        while read < bytes.len() {
            let count = file
                .read(&mut bytes[read..])
                .map_err(|error| format!("could not read package: {error}"))?;
            if count == 0 {
                break;
            }
            read += count;
        }
        if read < bytes.len() {
            return Err("package too short".to_owned());
        }
        if bytes[..4] != [0x7f, b'E', b'L', b'F'] {
            return Err("not an ELF file".to_owned());
        }
        if bytes[4] != 2 || bytes[5] != 1 {
            return Err("unsupported ELF class".to_owned());
        }
        if &bytes[8..11] != b"AI\x02" {
            return Err("not a type-2 AppImage".to_owned());
        }
        let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
        if machine != self.machine {
            return Err("wrong architecture".to_owned());
        }
        Ok(())
    }
}

pub fn installed_layout(exe: &Path, lib_dir: &Path) -> bool {
    let Ok(exe) = fs::canonicalize(exe) else {
        return false;
    };
    let Ok(lib) = fs::canonicalize(lib_dir) else {
        return false;
    };
    let Ok(entries) = fs::read_dir(&lib) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return false;
        };
        if !valid_app_name(name) || !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            return false;
        }
        let Ok(bin) = fs::canonicalize(entry.path().join("usr/bin")) else {
            return false;
        };
        exe.starts_with(bin)
    })
}

pub fn update_platform(exe: &Path, lib_dir: &Path) -> Option<UpdatePlatform> {
    if !installed_layout(exe, lib_dir) {
        return None;
    }
    Some(UpdatePlatform {
        installer: Arc::new(AppImageInstaller::new(
            lib_dir.to_owned(),
            RuntimeExtractor,
            SystemdUser,
        )),
        verifier: Arc::new(AppImageVerifier::current()),
        downloader: Arc::new(CurlDownloader::default()),
    })
}

pub fn default_lib_dir(home: &Path) -> PathBuf {
    home.join(".local/lib/remote-host")
}

fn valid_app_name(name: &str) -> bool {
    name.starts_with(APP_PREFIX)
        && name.len() > APP_PREFIX.len()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-'))
}

fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.new-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(contents)?;
    file.sync_all()?;
    fs::rename(temporary, path)
}

fn remove_path(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path)
        }
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn set_executable(path: &Path) -> io::Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o700);
    }
    fs::set_permissions(path, permissions)
}

fn is_executable(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

fn short_utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).chars().take(200).collect()
}

fn read_short(reader: impl Read) -> String {
    let mut bytes = Vec::new();
    let _ = reader.take(4096).read_to_end(&mut bytes);
    short_utf8(&bytes)
}
