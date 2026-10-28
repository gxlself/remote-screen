use async_trait::async_trait;
use host_core::{
    pairing::{PairApproval, PairApprovalRequest, PairApprover},
    HostRuntime, RuntimeOptions,
};
use host_fake::FakePlatform;
use std::{process::Command, sync::Arc, time::Duration};

struct AllowPairing;
#[async_trait]
impl PairApprover for AllowPairing {
    async fn request(&self, _: PairApprovalRequest) -> PairApproval {
        PairApproval {
            allow: true,
            permission: rsp_core::trust::Permission::Control,
        }
    }
}

#[async_trait]
impl host_core::session::handshake::ConnectionApprover for AllowPairing {
    async fn request(&self, _: host_core::session::handshake::ConnectionApprovalRequest) -> bool {
        true
    }
}

#[tokio::test]
async fn cli_pairs_persists_identity_and_reconnects_with_certificate() {
    let dir = std::env::temp_dir().join(format!(
        "h3b-probe-{}-{}",
        std::process::id(),
        rsp_core::clock::unix_micros()
    ));
    let mut options = RuntimeOptions {
        bind: Some("127.0.0.1:0".parse().unwrap()),
        store_dir: dir.join("host"),
        ..Default::default()
    };
    options.auth_policy.attended_mode = true;
    let host = HostRuntime::new(FakePlatform::new(), options)
        .with_pairing_approver(Arc::new(AllowPairing))
        .with_connection_approver(Arc::new(AllowPairing))
        .bind()
        .await
        .unwrap();
    let addr = host
        .listener
        .bound()
        .into_iter()
        .find(|(_, _, kind)| *kind == rsp_transport::TransportKind::Quic)
        .unwrap()
        .0;
    let security = host.security.clone();
    let ticket = security
        .pairing
        .start(rsp_core::clock::unix_micros())
        .unwrap();
    let pin = rsp_client::fingerprint_hex(&security.identity.fp());
    let task = tokio::spawn(host.run());
    let identity_dir = dir.join("device");
    let mut previous_cert = None;
    for (code, probe) in [
        (None, true),
        (Some(ticket.code), false),
        (None, true),
        (None, false),
    ] {
        let pairing = code.is_some();
        let mut command = Command::new(env!("CARGO_BIN_EXE_rs-probe"));
        command
            .args([
                "--addr",
                &addr.to_string(),
                "--pin",
                &pin,
                "--seconds",
                "3",
                "--identity",
            ])
            .arg(&identity_dir);
        if let Some(code) = code {
            command.args(["--pair-code", &code]);
        }
        if probe {
            command.arg("--probe");
        }
        let mut child = command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // Allow pairing/approval, the bounded first-frame wait, and sampling
        // to finish even when this process is scheduled infrequently.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if tokio::time::Instant::now() > deadline {
                child.kill().unwrap();
                panic!(
                    "probe CLI timed out: {:?}",
                    child.wait_with_output().unwrap()
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let output = child.wait_with_output().unwrap();
        let output_text = String::from_utf8(output.stdout).unwrap();
        let error_text = String::from_utf8(output.stderr).unwrap();
        eprintln!(
            "probe={probe}, pairing={pairing}, status={}:\n{output_text}{error_text}",
            output.status
        );
        assert!(
            output.status.success(),
            "probe failed: {output_text}\n{error_text}"
        );
        if pairing {
            assert!(output_text.contains("paired with "), "{output_text}");
        }
        let cert = std::fs::read(identity_dir.join("cert.der")).unwrap();
        if let Some(previous_cert) = previous_cert.replace(cert.clone()) {
            assert_eq!(previous_cert, cert);
        }
        if probe {
            assert!(output_text.contains("probe complete:"), "{output_text}");
            assert!(!output_text.contains("first frame"), "{output_text}");
            assert!(!output_text.contains("stream "), "{output_text}");
            assert!(!output_text.contains("frames "), "{output_text}");
        } else {
            assert!(output_text.contains("first frame"), "{output_text}");
            let frames = output_text
                .lines()
                .find_map(|line| {
                    line.strip_prefix("frames ")?
                        .split_whitespace()
                        .next()?
                        .parse::<u64>()
                        .ok()
                })
                .expect("probe summary must include a frame count");
            assert!(frames >= 1, "{output_text}");
            assert_eq!(security.store.paired().unwrap().len(), 1);
        }
    }
    task.abort();
    let _ = task.await;
    std::fs::remove_dir_all(dir).unwrap();
}
