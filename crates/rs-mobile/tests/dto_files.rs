use rs_mobile::rscore::dto_core::{CoreConfig, CoreEvent, NetPath};
use rs_mobile::rscore::dto_files::*;
use serde_json::json;

#[test]
fn file_listing_json_preserves_nulls_and_source_tags() {
    let listing = FileListing {
        path: "/Users/example/Downloads".into(),
        parent: None,
        entries: vec![FileEntry {
            name: "folder".into(),
            kind: FileEntryKind::Dir,
            size: None,
            mtime_us: None,
            child_count: None,
        }],
        truncated: false,
        net_path: Some(NetPath::Lan),
    };
    let value = serde_json::to_value(&listing).unwrap();
    assert_eq!(
        value,
        json!({
            "path": "/Users/example/Downloads", "parent": null,
            "entries": [{"name": "folder", "kind": "dir", "size": null,
                "mtimeUs": null, "childCount": null}],
            "truncated": false, "netPath": "lan"
        })
    );
    assert_eq!(
        serde_json::from_value::<FileListing>(value).unwrap(),
        listing
    );
    for (source, value) in [
        (
            FileSourceRef::Computer {
                host_id: "h1".into(),
            },
            json!({"kind": "computer", "hostId": "h1"}),
        ),
        (
            FileSourceRef::Server {
                server_id: "s1".into(),
            },
            json!({"kind": "server", "serverId": "s1"}),
        ),
    ] {
        assert_eq!(serde_json::to_value(&source).unwrap(), value);
        assert_eq!(
            serde_json::from_value::<FileSourceRef>(value).unwrap(),
            source
        );
    }
}

#[test]
fn transfer_events_use_the_app_json_contract() {
    let value = json!({
        "transferId": "00112233445566778899aabbccddeeff", "direction": "upload",
        "source": {"kind": "computer", "hostId": "h1"}, "name": "hello.txt",
        "remotePath": null, "localPath": "/phone/inbox/hello.txt", "size": 7,
        "done": 3, "bytesPerSec": 5, "state": "interrupted", "fail": "sha256Mismatch",
        "netPath": "tailnetDirect", "savedName": null, "startedAt": 123, "updatedAt": 456
    });
    let transfer: FileTransfer = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(transfer.direction, FileTransferDirection::Upload);
    assert_eq!(transfer.fail, Some(FileTransferFail::Sha256Mismatch));
    let event = CoreEvent::FileTransfer { transfer };
    let event_value = json!({"type": "file.transfer", "transfer": value});
    assert_eq!(event.event_type(), "file.transfer");
    assert_eq!(serde_json::to_value(&event).unwrap(), event_value);
    assert_eq!(
        serde_json::from_value::<CoreEvent>(event_value).unwrap(),
        event
    );
    let removed = CoreEvent::FileTransferRemoved {
        transfer_id: "local-id".into(),
    };
    assert_eq!(removed.event_type(), "file.transferRemoved");
    assert_eq!(
        serde_json::to_value(&removed).unwrap(),
        json!({
            "type": "file.transferRemoved", "transferId": "local-id"
        })
    );
}

#[test]
fn core_config_accepts_optional_files_dir() {
    let mut value = json!({"dataDir": "/phone/data", "hubUrl": "https://hs.example.com",
        "appVersion": "0.4.0", "platform": "ios"});
    assert_eq!(
        serde_json::from_value::<CoreConfig>(value.clone())
            .unwrap()
            .files_dir,
        None
    );
    value["filesDir"] = json!("/phone/Documents/Received");
    let config: CoreConfig = serde_json::from_value(value).unwrap();
    assert_eq!(
        config.files_dir.as_deref(),
        Some("/phone/Documents/Received")
    );
    assert_eq!(
        serde_json::to_value(config).unwrap()["filesDir"],
        "/phone/Documents/Received"
    );
}
