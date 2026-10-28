fn main() {
    println!("cargo:rerun-if-changed=src/capture_mac.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new()
        .file("src/capture_mac.m")
        .flag("-fobjc-arc")
        .flag("-fmodules")
        .flag("-Wall")
        .flag("-Wno-unused-parameter")
        .compile("rs_bench_capture_mac");
    for fw in [
        "Foundation",
        "AppKit",
        "CoreGraphics",
        "CoreMedia",
        "CoreVideo",
        "ScreenCaptureKit",
    ] {
        println!("cargo:rustc-link-lib=framework={fw}");
    }
}
