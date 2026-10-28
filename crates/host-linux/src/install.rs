//! User-level installation and removal of the Linux AppImage.

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use thiserror::Error;

use crate::update_appimage::{
    check_payload, current_target, prune, switch_current, Extractor, HOST_UNIT, NOTIFY_UNIT,
};

#[derive(Debug, Clone)]
pub struct InstallPaths {
    pub lib_dir: PathBuf,
    pub unit_dir: PathBuf,
    pub bin_dir: PathBuf,
    pub state_dir: PathBuf,
}

impl InstallPaths {
    pub fn under(
        home: &Path,
        xdg_config_home: Option<&Path>,
        xdg_state_home: Option<&Path>,
    ) -> Self {
        let config = xdg_config_home
            .filter(|path| !path.as_os_str().is_empty())
            .map(Path::to_owned)
            .unwrap_or_else(|| home.join(".config"));
        let state = xdg_state_home
            .filter(|path| !path.as_os_str().is_empty())
            .map(Path::to_owned)
            .unwrap_or_else(|| home.join(".local/state"));
        Self {
            lib_dir: home.join(".local/lib/remote-host"),
            unit_dir: config.join("systemd/user"),
            bin_dir: home.join(".local/bin"),
            state_dir: state.join("remote-host"),
        }
    }

    pub fn from_env() -> Result<Self, InstallError> {
        let home = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or(InstallError::NoHome)?;
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let state = std::env::var_os("XDG_STATE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        Ok(Self::under(&home, config.as_deref(), state.as_deref()))
    }
}

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("HOME is not set")]
    NoHome,
    #[error("path cannot be used in a systemd unit: {0}")]
    BadPath(String),
    #[error("no AppImage given (run from the AppImage or pass --from)")]
    NoSource,
    #[error("{0}")]
    Payload(String),
    #[error("I/O: {0}")]
    Io(String),
    #[error("systemctl --user {0}")]
    Systemctl(String),
}

pub trait Systemctl {
    fn user(&self, args: &[&str]) -> Result<(), String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemctlCmd;

impl Systemctl for SystemctlCmd {
    fn user(&self, args: &[&str]) -> Result<(), String> {
        let output = Command::new("systemctl")
            .arg("--user")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| error.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(short_utf8(&output.stderr))
        }
    }
}

pub fn unit_path_ok(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    #[cfg(unix)]
    let bytes = std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str());
    #[cfg(not(unix))]
    let bytes = path.to_string_lossy().as_bytes();
    !bytes
        .iter()
        .any(|byte| matches!(*byte, b'"' | b'\\' | b'%' | b'$' | b'\n' | b'\t'))
}

pub fn host_unit(lib_dir: &Path) -> Result<String, InstallError> {
    if !unit_path_ok(lib_dir) {
        return Err(InstallError::BadPath(lib_dir.display().to_string()));
    }
    Ok(include_str!("../packaging/systemd/remote-host.service.in")
        .replace("@LIB_DIR@", &lib_dir.display().to_string()))
}

pub fn notify_unit(lib_dir: &Path) -> Result<String, InstallError> {
    if !unit_path_ok(lib_dir) {
        return Err(InstallError::BadPath(lib_dir.display().to_string()));
    }
    Ok(
        include_str!("../packaging/systemd/remote-host-notify.service.in")
            .replace("@LIB_DIR@", &lib_dir.display().to_string()),
    )
}

pub fn find_in_path(name: &str, path_env: Option<&OsStr>) -> Option<PathBuf> {
    let path_env = path_env?;
    for directory in std::env::split_paths(path_env) {
        let candidate = directory.join(name);
        let Ok(metadata) = fs::metadata(&candidate) else {
            continue;
        };
        if metadata.is_file() && is_executable(&metadata) {
            return Some(candidate);
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    pub version: String,
    pub upgraded_from: Option<String>,
    pub curl_found: bool,
    pub bin_link_skipped: bool,
}

pub fn install(
    appimage: &Path,
    paths: &InstallPaths,
    extractor: &dyn Extractor,
    systemctl: &dyn Systemctl,
) -> Result<InstallReport, InstallError> {
    if !unit_path_ok(&paths.lib_dir) {
        return Err(InstallError::BadPath(paths.lib_dir.display().to_string()));
    }
    fs::create_dir_all(&paths.lib_dir).map_err(io_error)?;
    let staging = paths
        .lib_dir
        .join(format!("staging-{}", std::process::id()));
    remove_path(&staging).map_err(io_error)?;
    fs::create_dir_all(&staging).map_err(io_error)?;
    let root = match extractor.extract(appimage, &staging) {
        Ok(root) => root,
        Err(error) => {
            let _ = remove_path(&staging);
            return Err(InstallError::Payload(error));
        }
    };
    let version = match check_payload(&root) {
        Ok(version) => version,
        Err(error) => {
            let _ = remove_path(&staging);
            return Err(InstallError::Payload(error));
        }
    };
    let target_name = format!("app-{version}");
    let old_target = current_target(&paths.lib_dir);
    let current_entry = paths.lib_dir.join("current");
    if (current_entry.exists() || fs::symlink_metadata(&current_entry).is_ok())
        && old_target.is_none()
    {
        let _ = remove_path(&staging);
        return Err(InstallError::Payload(
            "current is not an installed AppImage layout".to_owned(),
        ));
    }
    let upgraded_from = old_target.as_ref().and_then(|old| {
        fs::read_to_string(
            paths
                .lib_dir
                .join(old)
                .join(crate::update_appimage::VERSION_FILE),
        )
        .ok()
        .map(|version| version.trim().to_owned())
        .or_else(|| old.strip_prefix("app-").map(ToOwned::to_owned))
    });
    if old_target.is_some() {
        let _ = systemctl.user(&["stop", NOTIFY_UNIT, HOST_UNIT]);
    }

    let target = paths.lib_dir.join(&target_name);
    if fs::symlink_metadata(&target).is_ok() {
        remove_path(&target).map_err(io_error)?;
    }
    if let Err(error) = fs::rename(&root, &target) {
        let _ = remove_path(&staging);
        return Err(io_error(error));
    }
    if let Err(error) = switch_current(&paths.lib_dir, &target_name) {
        let _ = remove_path(&target);
        let _ = remove_path(&staging);
        return Err(io_error(error));
    }
    let keep_old = old_target.as_deref().unwrap_or("");
    prune(&paths.lib_dir, &[&target_name, keep_old]);
    let _ = remove_path(&staging);

    let host_text = host_unit(&paths.lib_dir)?;
    let notify_text = notify_unit(&paths.lib_dir)?;
    fs::create_dir_all(&paths.unit_dir).map_err(io_error)?;
    write_unit(&paths.unit_dir.join(HOST_UNIT), &host_text).map_err(io_error)?;
    write_unit(&paths.unit_dir.join(NOTIFY_UNIT), &notify_text).map_err(io_error)?;

    fs::create_dir_all(&paths.bin_dir).map_err(io_error)?;
    let bin_link = paths.bin_dir.join("remote-host-ctl");
    let bin_link_skipped = match fs::symlink_metadata(&bin_link) {
        Ok(metadata) if !metadata.file_type().is_symlink() => true,
        Ok(_) => {
            remove_path(&bin_link).map_err(io_error)?;
            create_link(
                &bin_link,
                &paths.lib_dir.join("current/usr/bin/remote-host-ctl"),
            )
            .map_err(io_error)?;
            false
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_link(
                &bin_link,
                &paths.lib_dir.join("current/usr/bin/remote-host-ctl"),
            )
            .map_err(io_error)?;
            false
        }
        Err(error) => return Err(io_error(error)),
    };

    systemctl
        .user(&["daemon-reload"])
        .map_err(InstallError::Systemctl)?;
    if old_target.is_some() {
        systemctl
            .user(&["enable", HOST_UNIT, NOTIFY_UNIT])
            .map_err(InstallError::Systemctl)?;
        systemctl
            .user(&["restart", HOST_UNIT, NOTIFY_UNIT])
            .map_err(InstallError::Systemctl)?;
    } else {
        systemctl
            .user(&["enable", "--now", HOST_UNIT, NOTIFY_UNIT])
            .map_err(InstallError::Systemctl)?;
    }

    Ok(InstallReport {
        version,
        upgraded_from,
        curl_found: find_in_path("curl", std::env::var_os("PATH").as_deref()).is_some(),
        bin_link_skipped,
    })
}

pub fn uninstall(
    paths: &InstallPaths,
    systemctl: &dyn Systemctl,
    purge: bool,
) -> Result<(), InstallError> {
    let _ = systemctl.user(&["disable", "--now", NOTIFY_UNIT, HOST_UNIT]);
    remove_if_exists(&paths.unit_dir.join(HOST_UNIT)).map_err(io_error)?;
    remove_if_exists(&paths.unit_dir.join(NOTIFY_UNIT)).map_err(io_error)?;
    let _ = systemctl.user(&["daemon-reload"]);
    let link = paths.bin_dir.join("remote-host-ctl");
    if is_link_into(&link, &paths.lib_dir) {
        remove_if_exists(&link).map_err(io_error)?;
    }
    remove_if_exists(&paths.lib_dir).map_err(io_error)?;
    if purge {
        remove_if_exists(&paths.state_dir).map_err(io_error)?;
    }
    Ok(())
}

fn write_unit(path: &Path, contents: &str) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
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
    file.write_all(contents.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o644))?;
    }
    file.sync_all()?;
    fs::rename(temporary, path)
}

fn create_link(link: &Path, target: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(not(unix))]
    {
        let _ = (link, target);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "symbolic links are unsupported",
        ))
    }
}

fn is_link_into(link: &Path, lib_dir: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(link) else {
        return false;
    };
    if !metadata.file_type().is_symlink() {
        return false;
    }
    let Ok(target) = fs::read_link(link) else {
        return false;
    };
    let target = if target.is_absolute() {
        target
    } else {
        link.parent().unwrap_or_else(|| Path::new(".")).join(target)
    };
    let Ok(target) = fs::canonicalize(target) else {
        return false;
    };
    let Ok(lib) = fs::canonicalize(lib_dir) else {
        return false;
    };
    target.starts_with(lib)
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path)
        }
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_path(path: &Path) -> io::Result<()> {
    remove_if_exists(path)
}

fn io_error(error: io::Error) -> InstallError {
    InstallError::Io(error.to_string())
}

fn short_utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).chars().take(200).collect()
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
