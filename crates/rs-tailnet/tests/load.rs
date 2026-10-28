#![cfg(unix)]

use rs_tailnet::{load, LoadError};
use std::path::{Path, PathBuf};

#[test]
fn loading_failures_then_single_library() {
    let missing = Path::new("/nonexistent/librs_tailnet.so");
    let error = load(missing).unwrap_err();
    assert!(matches!(error, LoadError::Open { .. }));
    assert!(error.to_string().contains(missing.to_str().unwrap()));
    #[cfg(target_os = "linux")]
    let system = Path::new("libm.so.6");
    #[cfg(target_os = "macos")]
    let system = Path::new("/usr/lib/libSystem.B.dylib");
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let error = load(system).unwrap_err();
        assert!(matches!(
            error,
            LoadError::Symbol {
                name: "rs_ts_abi_version"
            }
        ));
        assert!(error.to_string().contains("rs_ts_abi_version"));
    }
    let Some(path) = std::env::var_os("RS_TAILNET_LIB").filter(|p| !p.is_empty()) else {
        assert_ne!(std::env::var("RS_REQUIRE_TAILNET").as_deref(), Ok("1"));
        eprintln!("SKIP loading_real_library: RS_TAILNET_LIB not set");
        return;
    };
    let path = PathBuf::from(path);
    let first = load(&path).unwrap();
    assert!(std::ptr::eq(first, load(&path).unwrap()));
    let root = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp"));
    let dir = root.join(format!("rs-tailnet-load-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join("librs_tailnet-copy.so");
    std::fs::copy(&path, &copy).unwrap();
    let error = load(&copy).unwrap_err();
    assert!(
        matches!(&error, LoadError::AlreadyLoaded { loaded, requested } if loaded == &path && requested == &copy)
    );
    assert!(error.to_string().contains(copy.to_str().unwrap()));
    std::fs::remove_dir_all(dir).unwrap();
}
