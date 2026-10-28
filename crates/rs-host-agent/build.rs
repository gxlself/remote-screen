use std::{env, error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return Ok(());
    }

    println!("cargo:rerun-if-changed=macos/Info.plist.in");
    let template = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("missing manifest dir")?)
        .join("macos/Info.plist.in");
    let plist = fs::read_to_string(template)?.replace("@VERSION@", &env::var("CARGO_PKG_VERSION")?);
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("missing OUT_DIR")?).join("Info.plist");
    fs::write(&output, plist)?;
    println!(
        "cargo:rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        output.display()
    );
    Ok(())
}
