fn main() {
    println!("cargo:rerun-if-changed=src/shim.m");
    println!("cargo:rerun-if-changed=src/audio_shim.m");
    println!("cargo:rerun-if-changed=src/sysauth.m");
    println!("cargo:rerun-if-changed=src/codesign.m");
    println!("cargo:rerun-if-changed=src/vdisplay.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new()
        .file("src/shim.m")
        .file("src/audio_shim.m")
        .file("src/sysauth.m")
        .file("src/codesign.m")
        .file("src/thumbnail.m")
        .file("src/vdisplay.m")
        .flag("-fobjc-arc")
        .flag("-fmodules")
        .flag("-Wall")
        .flag("-Wno-unused-parameter")
        .compile("rs_host_shim");
    for fw in [
        "Foundation",
        "AppKit",
        "CoreGraphics",
        "CoreMedia",
        "CoreVideo",
        "VideoToolbox",
        "ScreenCaptureKit",
        "ApplicationServices",
        "IOKit",
        "Security",
        "Carbon",
        "OpenDirectory",
        "SystemConfiguration",
    ] {
        println!("cargo:rustc-link-lib=framework={fw}");
    }
}
