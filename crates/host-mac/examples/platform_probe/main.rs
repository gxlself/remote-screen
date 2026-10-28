//! Manual probe for the macOS platform layer (see mac.rs). Other platforms
//! build a stub so `cargo check --workspace --all-targets` passes on Linux.

#[cfg(target_os = "macos")]
mod encode;
#[cfg(target_os = "macos")]
mod input;
#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "macos")]
fn main() {
    if encode::requested() {
        encode::main();
    } else {
        mac::main();
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("platform_probe only runs on macOS");
}
