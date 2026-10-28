//!  在 macOS 目标上编译 `mac/*.swift`；其他目标什么都不做。

use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by Cargo"));
    let mac_dir = manifest_dir.join("mac");
    println!("cargo:rerun-if-changed={}", mac_dir.display());
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
    println!("cargo:rerun-if-env-changed=RS_DESKTOP_SWIFTC");

    let mut swift_files = match fs::read_dir(&mac_dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "swift")
            })
            .collect::<Vec<_>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("cannot read {}: {error}", mac_dir.display()),
    };
    swift_files.sort();
    for path in &swift_files {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    if swift_files.is_empty() {
        return;
    }

    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        Ok(other) => panic!("unsupported macOS target architecture: {other}"),
        Err(_) => panic!("CARGO_CFG_TARGET_ARCH is set by Cargo"),
    };
    let deployment_target = env::var("MACOSX_DEPLOYMENT_TARGET")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "13.0".into());
    let target = format!("{arch}-apple-macos{deployment_target}");
    let sdk = command_output(
        Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]),
        "xcrun --sdk macosx --show-sdk-path",
    );
    let sdk = sdk.trim();
    if sdk.is_empty() {
        panic!("xcrun returned an empty macOS SDK path");
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let profile = env::var("PROFILE").unwrap_or_default();
    let optimization = if profile == "debug" {
        ["-Onone", "-g"].as_slice()
    } else {
        ["-O", "-wmo"].as_slice()
    };

    let mut swiftc = Command::new("xcrun");
    if let Ok(path) = env::var("RS_DESKTOP_SWIFTC") {
        swiftc = Command::new(path);
    } else {
        swiftc.arg("swiftc");
    }
    swiftc
        .args([
            "-parse-as-library",
            "-emit-library",
            "-static",
            "-module-name",
            "RsDesktopMac",
            "-target",
            &target,
            "-sdk",
            sdk,
        ])
        .args(optimization)
        .args(&swift_files)
        .arg("-o")
        .arg(out_dir.join("libRsDesktopMac.a"));
    let output = swiftc
        .output()
        .unwrap_or_else(|error| panic!("failed to execute swiftc: {error}"));
    if !output.status.success() {
        // Keep the compiler's diagnostics intact; Swift errors are much more useful
        // to the caller than a generic build-script error.
        let _ = std::io::stderr().write_all(&output.stderr);
        panic!("swiftc failed with status {}", output.status);
    }

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=RsDesktopMac");
    println!("cargo:rustc-link-search=native=/usr/lib/swift");
    println!("cargo:rustc-link-search=native={sdk}/usr/lib/swift");
    let xcode = command_output(Command::new("xcode-select").arg("-p"), "xcode-select -p");
    let xcode_swift =
        Path::new(xcode.trim()).join("Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx");
    println!("cargo:rustc-link-search=native={}", xcode_swift.display());
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    for framework in [
        "Foundation",
        "AppKit",
        "CryptoKit",
        "Security",
        "VideoToolbox",
        "CoreMedia",
        "CoreVideo",
        "AVFoundation",
        "QuartzCore",
        "CoreGraphics",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}

fn command_output(command: &mut Command, description: &str) -> String {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("failed to run {description}: {error}"));
    if !output.status.success() {
        let _ = std::io::stderr().write_all(&output.stderr);
        panic!("{description} failed with status {}", output.status);
    }
    String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("{description} returned non-UTF-8 output: {error}"))
}
