mod support;

use std::time::Duration;

use rs_mobile::rscore::{
    api_diag::set_setting,
    api_ssh::servers,
    dto_core::{SshAuth, SshServer, SshSource},
};
use serde_json::json;
use tokio::time::timeout;

async fn book(core: &std::sync::Arc<rs_mobile::rscore::runtime::Core>) -> Vec<SshServer> {
    timeout(Duration::from_secs(3), servers(core))
        .await
        .expect("local server book returns promptly")
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_hub_the_server_book_reads_the_local_sync_copy() {
    let core = support::test_core();
    assert!(book(&core).await.is_empty());

    let value = json!({
        "v": 1,
        "entries": [{"kind": "ssh", "id": "manual", "name": "Manual",
            "host": "10.30.0.7", "port": 2222, "user": "ops", "auth": "deviceKey"}],
    });
    set_setting(&core, "sync.servers", &value.to_string()).unwrap();
    assert_eq!(
        book(&core).await,
        vec![SshServer {
            server_id: "manual".into(),
            name: "Manual".into(),
            host: "10.30.0.7".into(),
            port: 2222,
            user: "ops".into(),
            jump: None,
            auth: SshAuth::DeviceKey,
            tailscale_ssh: None,
            os: None,
            source: SshSource::Manual,
        }]
    );
    core.shutdown();
}
