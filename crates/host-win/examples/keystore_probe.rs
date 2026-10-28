#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use host_core::platform::KeyStore;
    use host_win::dpapi::{AclSecurer, DpapiProtector};
    use host_win::keyfile::FileKeyStore;
    use host_win::paths::WinPaths;
    use std::path::PathBuf;
    use windows::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };
    use zeroize::Zeroizing;

    let mut args = std::env::args_os().skip(1);
    let mut directory = None;
    while let Some(argument) = args.next() {
        if argument == "--dir" {
            directory = Some(
                args.next()
                    .ok_or("missing value for --dir")
                    .map(PathBuf::from)?,
            );
        } else {
            return Err(format!("unknown argument: {}", argument.to_string_lossy()).into());
        }
    }
    let directory = directory.unwrap_or_else(|| {
        let program_data = std::env::var("ProgramData").ok();
        WinPaths::from_program_data(program_data.as_deref()).keys_dir
    });
    let store = FileKeyStore::new(directory.clone(), DpapiProtector, AclSecurer);

    let mut probe = Zeroizing::new([0u8; 32]);
    // SAFETY: the system-preferred RNG accepts a null algorithm handle and
    // writes only to the supplied 32-byte buffer, which remains alive here.
    unsafe { BCryptGenRandom(None, probe.as_mut_slice(), BCRYPT_USE_SYSTEM_PREFERRED_RNG) }.ok()?;
    store.store("probe-v1", probe.as_slice())?;
    let matches = store.load("probe-v1").map(|value| {
        let loaded = value.map(Zeroizing::new);
        loaded.as_ref().map(|value| value.as_slice()) == Some(probe.as_slice())
    });
    // Delete the probe even when loading fails or the comparison differs.
    store.delete("probe-v1")?;
    if !matches? {
        return Err("probe load did not match".into());
    }
    println!(
        "{}",
        serde_json::json!({
            "store": "ok",
            "load": "match",
            "delete": "ok",
            "path": directory.display().to_string(),
        })
    );
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}
