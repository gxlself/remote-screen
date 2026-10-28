use host_core::platform::Platform;
use host_fake::FakePlatform;

#[test]
fn fake_platform_uses_unknown_platform_queries_by_default() {
    let platform: &dyn Platform = &FakePlatform::new();

    assert_eq!(platform.screen_sharing_enabled(), None);
    assert_eq!(platform.filevault_enabled(), None);
}
