use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use host_core::update::install::Installer;

/// Name of the MSI that represents the version currently installed by the
/// update workflow.
pub const INSTALLED_MSI: &str = "installed.msi";
/// Name of the MSI downloaded and waiting for installation.
pub const STAGED_MSI: &str = "staged.msi";
/// Name of the previous MSI kept for recovery.
pub const PREVIOUS_MSI: &str = "previous.msi";

/// Small process launcher abstraction used by the pure installer logic.
pub trait Launch: Send + Sync + 'static {
    /// Start a process and return without waiting for it.
    fn launch(&self, exe: &Path, args: &[String]) -> Result<(), String>;
}

/// Filesystem and command-line part of the Windows MSI installer.
pub struct MsiInstaller<L: Launch> {
    update_dir: PathBuf,
    logs_dir: PathBuf,
    msiexec: PathBuf,
    launcher: L,
    clock: fn() -> u64,
}

impl<L: Launch> MsiInstaller<L> {
    pub fn new(update_dir: PathBuf, logs_dir: PathBuf, msiexec: PathBuf, launcher: L) -> Self {
        Self {
            update_dir,
            logs_dir,
            msiexec,
            launcher,
            clock: unix_secs,
        }
    }

    fn installed_path(&self) -> PathBuf {
        self.update_dir.join(INSTALLED_MSI)
    }

    fn staged_path(&self) -> PathBuf {
        self.update_dir.join(STAGED_MSI)
    }

    fn log_path(&self, kind: &str) -> PathBuf {
        self.logs_dir.join(log_file_name(kind, (self.clock)()))
    }

    fn ensure_logs_dir(&self) -> Result<(), String> {
        fs::create_dir_all(&self.logs_dir)
            .map_err(|error| format!("create logs directory {}: {error}", self.logs_dir.display()))
    }
}

impl<L: Launch> Installer for MsiInstaller<L> {
    fn install(&self, package: &Path, backup_dir: &Path) -> Result<(), String> {
        fs::create_dir_all(&self.update_dir).map_err(|error| {
            format!(
                "create update directory {}: {error}",
                self.update_dir.display()
            )
        })?;
        fs::create_dir_all(backup_dir).map_err(|error| {
            format!("create backup directory {}: {error}", backup_dir.display())
        })?;

        if !package.exists() {
            return Err(format!("package does not exist: {}", package.display()));
        }

        let installed = self.installed_path();
        if installed.exists() {
            let previous = backup_dir.join(PREVIOUS_MSI);
            fs::copy(&installed, &previous).map_err(|error| {
                format!(
                    "backup installed MSI {} to {}: {error}",
                    installed.display(),
                    previous.display()
                )
            })?;
        }

        let staged = self.staged_path();
        let temporary = self.update_dir.join(format!("{STAGED_MSI}.tmp"));
        remove_file_if_exists(&temporary)?;
        if let Err(error) = fs::copy(package, &temporary) {
            let _ = remove_file_if_exists(&temporary);
            return Err(format!(
                "stage package {} at {}: {error}",
                package.display(),
                temporary.display()
            ));
        }

        // Windows does not replace an existing destination with rename, so
        // remove a stale staged file before publishing the completed copy.
        if let Err(error) = remove_file_if_exists(&staged) {
            let _ = remove_file_if_exists(&temporary);
            return Err(error);
        }
        if let Err(error) = fs::rename(&temporary, &staged) {
            let _ = remove_file_if_exists(&temporary);
            return Err(format!("publish staged MSI {}: {error}", staged.display()));
        }
        Ok(())
    }

    fn restart(&self) -> Result<(), String> {
        let staged = self.staged_path();
        if !staged.exists() {
            return Err("no staged installer".to_owned());
        }
        self.ensure_logs_dir()?;

        let installed = self.installed_path();
        remove_file_if_exists(&installed)?;
        fs::rename(&staged, &installed).map_err(|error| {
            format!(
                "promote staged MSI {} to {}: {error}",
                staged.display(),
                installed.display()
            )
        })?;

        let args = install_args(&installed, &self.log_path("install"));
        self.launcher.launch(&self.msiexec, &args)
    }

    fn rollback(&self, backup_dir: &Path) -> Result<(), String> {
        let previous = backup_dir.join(PREVIOUS_MSI);
        if !previous.exists() {
            return Err("no previous installer to roll back to".to_owned());
        }
        fs::create_dir_all(&self.update_dir).map_err(|error| {
            format!(
                "create update directory {}: {error}",
                self.update_dir.display()
            )
        })?;
        self.ensure_logs_dir()?;

        let installed = self.installed_path();
        fs::copy(&previous, &installed).map_err(|error| {
            format!(
                "restore previous MSI {} to {}: {error}",
                previous.display(),
                installed.display()
            )
        })?;
        let args = install_args(&installed, &self.log_path("rollback"));
        self.launcher.launch(&self.msiexec, &args)
    }
}

/// Resolve the standard system `msiexec.exe` path using Windows path syntax.
pub fn msiexec_path(system_root: Option<&str>) -> PathBuf {
    let root = system_root
        .unwrap_or(r"C:\Windows")
        .trim_end_matches(['\\', '/']);
    PathBuf::from(format!(r"{root}\System32\msiexec.exe"))
}

/// Build the quiet, non-restarting MSI invocation and verbose log arguments.
pub fn install_args(msi: &Path, log: &Path) -> Vec<String> {
    vec![
        "/i".to_owned(),
        msi.to_string_lossy().into_owned(),
        "/qn".to_owned(),
        "/norestart".to_owned(),
        "/l*v".to_owned(),
        log.to_string_lossy().into_owned(),
    ]
}

/// Return the conventional update log filename.
pub fn log_file_name(kind: &str, unix_secs: u64) -> String {
    format!("update-{kind}-{unix_secs}.log")
}

/// Normalize certificate subjects for comparison while retaining case and
/// punctuation.
pub fn normalize_subject(subject: &str) -> String {
    subject.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Verify that a candidate package has the same signer subject as the running
/// executable.
pub fn subject_matches(current: Option<&str>, candidate: Option<&str>) -> Result<(), String> {
    let current =
        current.ok_or_else(|| "current executable is not signed; refusing update".to_owned())?;
    let candidate = candidate.ok_or_else(|| "package is not signed".to_owned())?;
    if normalize_subject(current) != normalize_subject(candidate) {
        return Err("signer subject mismatch".to_owned());
    }
    Ok(())
}

fn remove_file_if_exists(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove {}: {error}", path.display())),
    }
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
