fn main() -> Result<(), Box<dyn std::error::Error>> {
    prepare_dev_sidecar()?;
    tauri_build::build();
    Ok(())
}

// tauri-build checks externalBin even for cargo test / tauri dev. A debug-only,
// ignored placeholder keeps development independent of the release build script.
// Release builds must supply a real agent; never synthesize one in that profile.
fn prepare_dev_sidecar() -> Result<(), Box<dyn std::error::Error>> {
    use std::{env, fs, path::PathBuf};

    if env::var("PROFILE").as_deref() != Ok("debug")
        || env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos")
    {
        return Ok(());
    }
    let target = env::var("TARGET")?;
    let binary = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("missing manifest dir")?)
        .join("binaries")
        .join(format!("rs-host-agent-{target}"));
    if !binary.exists() {
        fs::create_dir_all(binary.parent().ok_or("missing sidecar parent")?)?;
        fs::write(
            &binary,
            "#!/bin/sh\nprintf '%s\\n' 'Development placeholder: run the agent separately with cargo run -p rs-host-agent.' >&2\nexit 1\n",
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(())
}
