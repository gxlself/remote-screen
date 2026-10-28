fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Read the manifest dir at run time: env!() would bake in the path of the
    // checkout that first compiled this script, and a CARGO_TARGET_DIR shared
    // by several worktrees reuses that binary after the checkout is gone.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")?;
    let root = std::path::Path::new(&manifest_dir).join("../../proto");
    let files = ["control.proto", "trust.proto", "streams.proto"]
        .map(|file| root.join("rsp/v1").join(file));
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    let descriptors = protox::compile(files, [root])?;
    // PairPassword 带账户与密码：跳过自动 Debug，pb.rs 里手写脱敏版本（04「其他配对方式」）。
    prost_build::Config::new()
        .skip_debug([".rsp.v1.PairPassword"])
        .compile_fds(descriptors)?;
    Ok(())
}
