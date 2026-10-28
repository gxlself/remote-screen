use std::path::PathBuf;

/// Default root used when the ProgramData environment value is absent or invalid.
pub const DEFAULT_PROGRAM_DATA: &str = r"C:\ProgramData";

/// Windows paths used by the host service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinPaths {
    pub data_dir: PathBuf,
    pub keys_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub config_file: PathBuf,
    pub db_file: PathBuf,
    pub update_dir: PathBuf,
}

impl WinPaths {
    /// Build host paths from the ProgramData environment value.
    ///
    /// The paths are assembled as Windows strings before being converted to
    /// PathBuf, so tests and path consumers see backslashes on every target.
    pub fn from_program_data(program_data: Option<&str>) -> Self {
        let root = program_data
            .filter(|value| is_absolute_windows_path(value))
            .unwrap_or(DEFAULT_PROGRAM_DATA);
        let root = root.strip_suffix('\\').unwrap_or(root);
        let data_dir = format!(r"{root}\RemoteHost");
        Self {
            keys_dir: PathBuf::from(format!(r"{data_dir}\keys")),
            logs_dir: PathBuf::from(format!(r"{data_dir}\logs")),
            config_file: PathBuf::from(format!(r"{data_dir}\config.toml")),
            db_file: PathBuf::from(format!(r"{data_dir}\host.db")),
            update_dir: PathBuf::from(format!(r"{data_dir}\update")),
            data_dir: PathBuf::from(data_dir),
        }
    }
}

fn is_absolute_windows_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\')
        || value.starts_with(r"\\")
}
