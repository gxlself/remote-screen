//! Tailscale API exports without network: every path here returns before a
//! request would be sent to the production API.

use rs_mobile::rscore::{
    adapters::secrets::{set_registered, MemSecretStore, SecretStore},
    api_tsapi::{
        tailscale_api_clear_token, tailscale_api_devices, tailscale_api_set_token,
        tailscale_api_status,
    },
    error::CoreError,
    tsapi::{CONTROL_KEY, TOKEN_KEY},
};
use serde_json::{json, Value};

/// The registered store is process-wide.
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn status_json(raw: String) -> Value {
    serde_json::from_str(&raw).unwrap()
}

#[tokio::test]
async fn exports_need_a_secret_store() {
    let _lock = LOCK.lock().await;
    set_registered(None);
    let unavailable = CoreError::Unsupported {
        message: "secret store unavailable".into(),
    };

    assert_eq!(
        tailscale_api_set_token("tskey-api-TEST".into(), None)
            .await
            .unwrap_err(),
        unavailable
    );
    assert_eq!(tailscale_api_clear_token().await.unwrap_err(), unavailable);
    assert_eq!(tailscale_api_status().await.unwrap_err(), unavailable);
    assert_eq!(tailscale_api_devices(false).await.unwrap_err(), unavailable);
}

#[tokio::test]
async fn exports_without_a_token() {
    let _lock = LOCK.lock().await;
    let store = MemSecretStore::new();
    set_registered(Some(store.clone()));

    assert_eq!(
        status_json(tailscale_api_status().await.unwrap()),
        json!({
            "configured": false, "controlUrl": null, "lastError": null,
            "lastOkAt": null, "deviceCount": null
        })
    );
    assert_eq!(
        tailscale_api_devices(false).await.unwrap_err(),
        CoreError::Unsupported {
            message: "tailscale api not configured".into()
        }
    );
    for token in ["", "  ", "tskey-api-A B"] {
        assert_eq!(
            tailscale_api_set_token(token.into(), None)
                .await
                .unwrap_err(),
            CoreError::Internal {
                message: "bad token".into()
            }
        );
    }
    assert_eq!(store.put_count(), 0);

    store.put(TOKEN_KEY, b"tskey-api-TEST").unwrap();
    assert_eq!(
        status_json(tailscale_api_status().await.unwrap())["configured"],
        json!(true)
    );
    tailscale_api_clear_token().await.unwrap();
    assert!(store.raw(TOKEN_KEY).is_none());
    assert_eq!(
        status_json(tailscale_api_status().await.unwrap()),
        json!({
            "configured": false, "controlUrl": null, "lastError": null,
            "lastOkAt": null, "deviceCount": null
        })
    );
    set_registered(None);
}

#[tokio::test]
async fn exports_with_a_headscale_server() {
    let _lock = LOCK.lock().await;
    let store = MemSecretStore::new();
    set_registered(Some(store.clone()));

    for control in [
        "http://hs.example.com",
        "https://hs.example.com/api",
        "https://admin@hs.example.com",
        "https://",
    ] {
        assert_eq!(
            tailscale_api_set_token("hskey-TEST".into(), Some(control.into()))
                .await
                .unwrap_err(),
            CoreError::Internal {
                message: "bad control url".into()
            },
            "{control}"
        );
    }
    assert_eq!(store.put_count(), 0);

    store.put(TOKEN_KEY, b"hskey-TEST").unwrap();
    store.put(CONTROL_KEY, b"https://hs.example.com").unwrap();
    assert_eq!(
        status_json(tailscale_api_status().await.unwrap())["controlUrl"],
        json!("https://hs.example.com")
    );
    tailscale_api_clear_token().await.unwrap();
    assert!(store.is_empty());
    assert_eq!(
        status_json(tailscale_api_status().await.unwrap())["controlUrl"],
        json!(null)
    );
    set_registered(None);
}
