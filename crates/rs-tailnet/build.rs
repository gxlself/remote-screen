use std::{env, path::PathBuf};

fn main() {
    if env::var_os("CARGO_FEATURE_STATIC_LINK").is_none()
        || env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("ios")
    {
        return;
    }
    let root = env::var_os("RS_TAILNET_IOS")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            panic!("static-link on iOS needs RS_TAILNET_IOS=<path>/RsTailnet.xcframework")
        });
    let slice = match env::var("TARGET").as_deref() {
        Ok("aarch64-apple-ios") => "ios-arm64",
        Ok("aarch64-apple-ios-sim") => "ios-arm64-simulator",
        target => panic!(
            "unsupported target {target:?}; static-link supports aarch64-apple-ios and aarch64-apple-ios-sim"
        ),
    };
    let dir = PathBuf::from(root).join(slice);
    let archive = dir.join("librs_tailnet.a");
    assert!(archive.is_file(), "missing archive {}", archive.display());
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-lib=static=rs_tailnet");
    println!("cargo:rerun-if-env-changed=RS_TAILNET_IOS");
    println!("cargo:rerun-if-changed={}", archive.display());
}
