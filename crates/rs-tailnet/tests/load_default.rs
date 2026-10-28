#![cfg(target_os = "linux")]

#[test]
fn default_requires_library_path() {
    // This integration binary has one test and never starts other threads.
    std::env::remove_var("RS_TAILNET_LIB");
    assert!(matches!(
        rs_tailnet::load_default(),
        Err(rs_tailnet::LoadError::Unavailable("set RS_TAILNET_LIB"))
    ));
}
