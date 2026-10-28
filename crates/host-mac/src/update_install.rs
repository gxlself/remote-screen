//! Mac 自动更新：替换 Remote Host.app、launchctl 重启与回滚（06「自动更新」第 6、7 步）。

use std::ffi::{CString, OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use host_core::update::install::Installer;

use crate::update_verify::{
    extract_update_zip, team_matches, verify_app, CodeSigning, SecCodeSigning, APP_BUNDLE_NAME,
};

/// launchd agent label registered by the macOS host app.
pub const AGENT_LABEL: &str = "ai.gxlself.remotehost.agent";

/// Process launcher used by [`MacAppInstaller::restart`].
pub trait Relauncher: Send + Sync + 'static {
    fn relaunch(&self) -> Result<(), String>;
}

/// Run `/bin/launchctl kickstart -k gui/<uid>/<label>` in a new process group,
/// returning after spawn without waiting for launchctl to exit.
#[derive(Debug, Clone)]
pub struct LaunchctlRelauncher {
    pub uid: u32,
    pub label: String,
}

impl LaunchctlRelauncher {
    pub fn current() -> Self {
        // SAFETY: `getuid` has no preconditions and does not dereference a
        // pointer. It returns the uid of this process.
        let uid = unsafe { libc::getuid() };
        Self {
            uid,
            label: AGENT_LABEL.to_owned(),
        }
    }
}

impl Relauncher for LaunchctlRelauncher {
    fn relaunch(&self) -> Result<(), String> {
        // launchctl will terminate this agent. Give it its own process group
        // and return as soon as it starts, without waiting for its exit.
        Command::new("/bin/launchctl")
            .args(kickstart_args(self.uid, &self.label))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("could not launch launchctl: {error}"))
    }
}

/// Build the launchctl kickstart arguments for an agent label.
pub fn kickstart_args(uid: u32, label: &str) -> Vec<String> {
    vec![
        "kickstart".to_owned(),
        "-k".to_owned(),
        format!("gui/{uid}/{label}"),
    ]
}

/// Return the app bundle containing an executable path with the conventional
/// `<bundle>.app/Contents/MacOS/<executable>` layout.
pub fn app_bundle_of(exe: &Path) -> Option<PathBuf> {
    exe.file_name()?;
    let macos = exe.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let app = contents.parent()?;
    if app.extension()? != "app" {
        return None;
    }
    Some(app.to_path_buf())
}

/// Atomically exchange two directory entries on the same volume.
pub fn swap_dirs(a: &Path, b: &Path) -> Result<(), String> {
    let from = CString::new(a.as_os_str().as_bytes())
        .map_err(|_| "path contains an embedded NUL byte".to_owned())?;
    let to = CString::new(b.as_os_str().as_bytes())
        .map_err(|_| "path contains an embedded NUL byte".to_owned())?;
    // SAFETY: both C strings are NUL-terminated and remain alive for the
    // duration of the libc call. `RENAME_SWAP` requests an atomic same-volume
    // exchange and does not retain either pointer.
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_SWAP) };
    if result == 0 {
        Ok(())
    } else {
        Err(format!(
            "could not atomically swap {} and {}: {}",
            a.display(),
            b.display(),
            std::io::Error::last_os_error()
        ))
    }
}

/// Replace and recover the app bundle that contains the running agent.
pub struct MacAppInstaller<C: CodeSigning = SecCodeSigning, R: Relauncher = LaunchctlRelauncher> {
    app: PathBuf,
    team_id: Option<String>,
    signing: C,
    relauncher: R,
}

impl MacAppInstaller<SecCodeSigning, LaunchctlRelauncher> {
    /// Use the canonical executable's app bundle and the current signing Team
    /// ID. Development binaries outside a bundle return an error.
    pub fn for_current_app() -> Result<Self, String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("could not find current executable: {error}"))?;
        let executable = fs::canonicalize(executable)
            .map_err(|error| format!("could not canonicalize current executable: {error}"))?;
        let app = app_bundle_of(&executable)
            .ok_or_else(|| "agent is not running from an app bundle".to_owned())?;
        let signing = SecCodeSigning;
        let team_id = signing.current_team_id()?;
        Ok(Self::new(
            app,
            team_id,
            signing,
            LaunchctlRelauncher::current(),
        ))
    }
}

impl<C: CodeSigning, R: Relauncher> MacAppInstaller<C, R> {
    pub fn new(app: PathBuf, team_id: Option<String>, signing: C, relauncher: R) -> Self {
        Self {
            app,
            team_id,
            signing,
            relauncher,
        }
    }

    /// Directory on the app's volume used to unpack and exchange updates.
    pub fn staging_dir(&self) -> PathBuf {
        let parent = self.app.parent().unwrap_or_else(|| Path::new("."));
        let mut name = OsString::from(".");
        name.push(self.app.file_name().unwrap_or(OsStr::new(APP_BUNDLE_NAME)));
        name.push(".update");
        parent.join(name)
    }

    fn app_name(&self) -> Result<&OsStr, String> {
        self.app
            .file_name()
            .ok_or_else(|| format!("app path has no bundle name: {}", self.app.display()))
    }

    fn prepare_staging(&self) -> Result<PathBuf, String> {
        let staging = self.staging_dir();
        let parent = self.app.parent().unwrap_or_else(|| Path::new("."));
        remove_path_if_present(&staging)
            .and_then(|()| fs::DirBuilder::new().mode(0o700).create(&staging))
            .map_err(|error| format!("无法在 {} 下准备更新：{error}", parent.display()))?;
        Ok(staging)
    }

    fn remove_staging(&self, event: &'static str) {
        let staging = self.staging_dir();
        if remove_path_if_present(&staging).is_err() {
            tracing::warn!(
                event = event,
                result = "cleanup_error",
                app = %self.app.display()
            );
        }
    }

    fn install_inner(&self, package: &Path, backup_dir: &Path, team: &str) -> Result<(), String> {
        let staging = self.prepare_staging()?;
        let staged_app = extract_update_zip(package, &staging)?;
        let app_name = self.app_name()?;
        if staged_app.file_name() != Some(app_name) {
            return Err("update bundle name does not match installed app".to_owned());
        }

        verify_app(&self.signing, &staged_app, team)?;

        fs::create_dir_all(backup_dir)
            .map_err(|error| format!("could not create backup directory: {error}"))?;
        let backup_app = backup_dir.join(app_name);
        remove_path_if_present(&backup_app)
            .map_err(|error| format!("could not remove previous backup: {error}"))?;
        copy_app(&self.app, &backup_app)?;

        // This is the only operation that changes the installed app. Nothing
        // after a successful exchange may turn the install into an error.
        swap_dirs(&staged_app, &self.app)
    }
}

impl<C: CodeSigning + 'static, R: Relauncher> Installer for MacAppInstaller<C, R> {
    fn install(&self, package: &Path, backup_dir: &Path) -> Result<(), String> {
        let Some(team) = self.team_id.as_deref() else {
            return team_matches(None, None);
        };

        let metadata = fs::symlink_metadata(&self.app).map_err(|error| {
            format!(
                "could not inspect app bundle {}: {error}",
                self.app.display()
            )
        })?;
        if !metadata.file_type().is_dir() {
            return Err(format!(
                "app bundle is not a directory: {}",
                self.app.display()
            ));
        }

        let result = self.install_inner(package, backup_dir, team);
        self.remove_staging("upgrade.install");
        match &result {
            Ok(()) => tracing::info!(
                event = "upgrade.install",
                result = "replaced",
                app = %self.app.display()
            ),
            Err(_) => tracing::warn!(
                event = "upgrade.install",
                result = "error",
                app = %self.app.display()
            ),
        }
        result
    }

    fn restart(&self) -> Result<(), String> {
        self.relauncher.relaunch()
    }

    fn rollback(&self, backup_dir: &Path) -> Result<(), String> {
        let app_name = self.app_name()?;
        let backup_app = backup_dir.join(app_name);
        match fs::symlink_metadata(&backup_app) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            _ => return Err("no previous app to roll back to".to_owned()),
        }

        let result = (|| {
            let staging = self.prepare_staging()?;
            let staged_app = staging.join(app_name);
            // The backup is the previously running app. Recovery keeps it
            // until confirmation; rollback neither re-verifies nor moves it.
            copy_app(&backup_app, &staged_app)?;
            swap_dirs(&staged_app, &self.app)
        })();

        self.remove_staging("upgrade.rollback");
        match &result {
            Ok(()) => tracing::info!(
                event = "upgrade.rollback",
                result = "restored",
                app = %self.app.display()
            ),
            Err(_) => tracing::warn!(
                event = "upgrade.rollback",
                result = "error",
                app = %self.app.display()
            ),
        }
        result
    }
}

fn copy_app(source: &Path, destination: &Path) -> Result<(), String> {
    let status = Command::new("/usr/bin/ditto")
        .arg(source)
        .arg(destination)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("could not start /usr/bin/ditto: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("ditto failed ({status})"))
    }
}

fn remove_path_if_present(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}
