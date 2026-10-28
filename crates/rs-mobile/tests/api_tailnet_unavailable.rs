//! The unavailable suite is intentionally one test: rs-tailnet's loader is a
//! process-global singleton and this binary must exercise the no-library path
//! in a deterministic order.

use rs_mobile::rscore::api_tailnet::{
    available, login, net_dialer, parse_start_options, start, status_json, stop,
};
use rs_mobile::rscore::error::CoreError;

mod support;

#[test]
fn tailnet_unavailable_is_reported_without_a_library() {
    std::env::remove_var("RS_TAILNET_LIB");
    std::env::remove_var("RS_TAILNET_TESTNET");
    assert!(!available());

    let core = support::test_core();
    let opts = parse_start_options(
        r#"{"controlUrl":"https://hs.example.com","hostname":"rs-app-test","disableNetns":true}"#,
    )
    .expect("options are valid");
    assert!(matches!(
        core.block_on(start(&core, opts)),
        Err(CoreError::Unsupported { .. })
    ));
    assert!(matches!(
        core.block_on(login(&core)),
        Err(CoreError::Unsupported { .. })
    ));
    assert!(core.block_on(stop(&core)).is_ok());

    let status: serde_json::Value =
        serde_json::from_str(&core.block_on(status_json(&core)).expect("status JSON"))
            .expect("valid status JSON");
    assert_eq!(status["state"], "unavailable");
    assert!(net_dialer(&core).is_none());
    core.shutdown();
}
