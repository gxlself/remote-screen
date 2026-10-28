//! Manual SYSTEM probe; outputs directory paths intentionally for diagnosis.

#[cfg(not(windows))]
fn main() {
    println!("user_dirs_probe: windows only");
}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{self, Write};
    use std::path::PathBuf;

    use host_core::user_dirs::UserDirsSource;
    use host_win::paths::WinPaths;
    use host_win::user_dirs::WinUserDirs;

    let program_data = std::env::var("ProgramData").ok();
    let mut paths = WinPaths::from_program_data(program_data.as_deref());
    let mut checks = Vec::new();
    let mut args = std::env::args_os().skip(1);
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: user_dirs_probe [--data-dir <dir>] [--can-write <dir>]...",
        )
    };
    while let Some(option) = args.next() {
        match option.to_str() {
            Some("--data-dir") => paths.data_dir = PathBuf::from(args.next().ok_or_else(invalid)?),
            Some("--can-write") => checks.push(PathBuf::from(args.next().ok_or_else(invalid)?)),
            _ => return Err(invalid().into()),
        }
    }
    let source = WinUserDirs::new(&paths);
    let (dirs, fallback) = source.explain();
    let can_write: Vec<_> = checks
        .iter()
        .map(|dir| serde_json::json!({ "dir": dir, "ok": source.user_can_write(dir) }))
        .collect();
    let output = serde_json::json!({
        "owner": format!("{:?}", dirs.owner),
        "downloads": dirs.downloads,
        "home": dirs.home,
        "fallback": fallback.map(|reason| format!("{reason:?}")),
        "can_write": can_write,
    });
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &output)?;
    writeln!(stdout)?;
    Ok(())
}
