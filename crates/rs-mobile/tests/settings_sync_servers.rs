mod support;

use rs_mobile::rscore::{
    api_diag::set_setting,
    settings::{Settings, SettingsState},
};
use serde_json::json;

#[test]
fn sync_servers_persists_the_pushed_value_and_ignores_malformed_json() {
    let core = support::test_core();
    let value = json!({
        "v": 1,
        "entries": [{"kind": "ssh", "id": "manual", "name": "Manual",
            "host": "10.30.0.7", "port": 22, "user": "ops", "auth": "password"}],
        "tailscaleSsh": {"build": true},
    })
    .to_string();

    set_setting(&core, "sync.servers", &value).unwrap();
    assert_eq!(
        core.store().kv_get("ssh.servers.sync").unwrap().as_deref(),
        Some(value.as_str())
    );

    set_setting(&core, "sync.servers", r#"{"v":1,"entries":["#).unwrap();
    assert_eq!(
        core.store().kv_get("ssh.servers.sync").unwrap().as_deref(),
        Some(value.as_str())
    );
    // The value lives in rs.db only; in-memory native settings are unchanged.
    assert_eq!(
        *core.state::<SettingsState>().0.lock().unwrap(),
        Settings::default()
    );
    core.shutdown();
}

#[test]
fn malformed_first_sync_servers_value_writes_nothing() {
    let core = support::test_core();
    set_setting(&core, "sync.servers", "not json").unwrap();
    assert_eq!(core.store().kv_get("ssh.servers.sync").unwrap(), None);
    core.shutdown();
}
