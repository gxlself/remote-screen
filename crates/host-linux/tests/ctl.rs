#![cfg(target_os = "linux")]

use std::time::Duration;

use host_core::ipc::{PairingStartResult, Permission, StatusGetResult};
use host_linux::ctl::*;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

fn prompt(kind: &str, intent: Option<&str>, permission_choice: bool) -> ApprovalPrompt {
    ApprovalPrompt {
        request_id: "r1".into(),
        kind: kind.into(),
        device_name: "测试设备".into(),
        platform: Some("Linux".into()),
        intent: intent.map(str::to_owned),
        whois_login: Some("alice".into()),
        timeout_s: 30,
        permission_choice,
    }
}

#[test]
fn wire_format_and_choices() {
    let line = request_line(7, "status.get", None);
    assert!(line.ends_with('\n'));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(line.trim()).unwrap(),
        json!({
            "jsonrpc": "2.0", "id": 7, "method": "status.get"
        })
    );
    assert_eq!(
        parse_line(r#"{"jsonrpc":"2.0","id":7,"result":{"ok":true}}"#).unwrap(),
        Incoming::Response {
            id: 7,
            result: Ok(json!({"ok": true})),
        }
    );
    assert_eq!(parse_line(r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32000,"message":"wait","data":{"code":"not_ready"}}}"#).unwrap(), Incoming::Response {
        id: 7,
        result: Err(("not_ready".into(), "wait".into())),
    });
    assert_eq!(
        parse_line(r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32601,"message":"missing"}}"#)
            .unwrap(),
        Incoming::Response {
            id: 7,
            result: Err(("-32601".into(), "missing".into())),
        }
    );
    assert!(matches!(parse_line("[1,2]"), Err(CtlError::Protocol(_))));
    assert_eq!(parse_choice(" YES ", false), Some(Choice::Allow(None)));
    assert_eq!(
        parse_choice("2", true),
        Some(Choice::Allow(Some(Permission::Control)))
    );
    assert_eq!(parse_choice("", true), Some(Choice::Deny));
    assert_eq!(parse_choice("x", false), None);
}

#[tokio::test]
async fn call_queues_events() {
    let path =
        std::env::temp_dir().join(format!("remote-host-ctl-{}-ctl.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        writer
            .write_all(
                br#"{"jsonrpc":"2.0","method":"event.first","params":{"n":1}}
{"jsonrpc":"2.0","method":"event.second","params":{"n":2}}
{"jsonrpc":"2.0","id":1,"result":{"ok":true}}
"#,
            )
            .await
            .unwrap();
    });
    let mut client = IpcClient::connect(&path).await.unwrap();
    assert_eq!(
        client
            .call("status.get", None, Duration::from_secs(1))
            .await
            .unwrap(),
        json!({"ok": true})
    );
    assert_eq!(client.next_event().await.unwrap().method, "event.first");
    assert_eq!(client.next_event().await.unwrap().method, "event.second");
    task.await.unwrap();
    let _ = std::fs::remove_file(path);
}

#[test]
fn approvals_qr_and_book() {
    let connection = prompt("connection", Some("control"), false);
    assert_eq!(approval_title(&connection), "测试设备 请求控制这台电脑");
    assert!(approval_body(&connection).contains("30 秒"));
    assert!(approval_from_value(&json!({"kind":"pairing"})).is_none());
    assert_eq!(format_code("123456"), "123 456");
    assert_eq!(format_code("12"), "12");
    let qr = qr_text("rsp://pair?x=1").unwrap();
    assert!(!qr.is_empty());
    assert!(qr
        .lines()
        .all(|line| line.chars().all(|c| " █▀▄".contains(c))));
    let pairing = PairingStartResult {
        qr_url: "rsp://pair".into(),
        code: "123456".into(),
        expires_at: 90_500_000,
    };
    assert!(pairing_text(&pairing, 0).contains("90 秒"));
    let mut book = ApprovalBook::default();
    book.add(connection.clone(), 9);
    let response = book.on_action(9, "allow").unwrap();
    assert!(response.allow && response.permission.is_none());
    book.add(prompt("pairing", None, true), 10);
    let response = book.on_action(10, "allow_control").unwrap();
    assert_eq!(response.permission, Some(Permission::Control));
    book.add(prompt("connection", None, false), 11);
    assert!(!book.on_closed(11, 2).unwrap().allow);
    assert!(book.is_empty());
}

#[test]
fn status_and_pairing_results() {
    let status = StatusGetResult {
        online: true,
        version: "1".into(),
        listening: true,
        ..Default::default()
    };
    assert!(status_text(&status).contains("在线"));
    assert!(pairing_result_text(&json!({"status":1})).contains("成功"));
    assert!(pairing_result_text(&json!({"status":4,"attempts_left":2})).contains("还可尝试 2 次"));
    assert!(pairing_result_text(&json!({"status":6})).contains("锁定"));
    assert!(pairing_result_text(&json!({"status":99})).contains("失败"));
}
