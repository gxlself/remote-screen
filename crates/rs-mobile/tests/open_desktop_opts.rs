mod support;

use std::sync::Arc;

use host_fake::{
    harness::{TestHost, TestHostOptions},
    FakePlatform,
};
use rs_mobile::rscore::{
    api_sessions,
    dto_core::{Codec, OpenIntent, OpenOpts},
    store::PairedHostRow,
    tables::SessionTable,
};
use rsp_core::control::EnvelopeDecoder;
use rsp_core::trust::Permission;
use rsp_transport::{identity::LocalIdentity, tls::server_config};
use tokio::time::{timeout, Duration};

fn opts(max_width: Option<u32>, codec: Option<Codec>) -> OpenOpts {
    OpenOpts {
        intent: OpenIntent::View,
        display_id: None,
        quality: None,
        mode: None,
        audio: None,
        max_fps: None,
        max_width,
        codec,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_options_are_clamped_and_advertised_in_client_hello() {
    let core = support::test_core_with(|_, options| options.worker_threads = 2);
    support::with_identity(&core, "desktop options test device");
    let host = TestHost::start_with_platform(
        FakePlatform::new(),
        TestHostOptions {
            tcp: false,
            ..Default::default()
        },
    )
    .await;
    host.pair(&core.local_identity().unwrap(), Permission::Control);
    let mut capture = CaptureHello::start().await;
    core.store()
        .upsert_paired_host(&PairedHostRow {
            host_id: host.host_id.clone(),
            fp: host.fp,
            name: "Paired test host".into(),
            addresses: vec![capture.addr],
            paired_at_us: core.now_us(),
            permission: Some("control".into()),
        })
        .unwrap();

    for (width, expected_width) in [
        (Some(2560), 2560),
        (Some(100), 640),
        (Some(10000), 7680),
        (None, 1920),
    ] {
        let entry = core.state::<SessionTable>().insert(
            &host.host_id,
            "Host",
            opts(width, None),
            core.now_us(),
        );
        let params = api_sessions::session_params(&core, &entry).unwrap();
        let hello = capture.hello_for(params).await;
        assert_eq!(hello.caps.as_ref().unwrap().max_width, expected_width);
        assert_eq!(
            hello.caps.as_ref().unwrap().decode,
            vec![rsp_core::pb::Codec::Hevc as i32]
        );
        core.state::<SessionTable>().remove(entry.id);
    }

    let entry = core.state::<SessionTable>().insert(
        &host.host_id,
        "Host",
        opts(None, Some(Codec::H264)),
        core.now_us(),
    );
    let params = api_sessions::session_params(&core, &entry).unwrap();
    let hello = capture.hello_for(params).await;
    assert_eq!(
        hello.caps.unwrap().decode,
        vec![rsp_core::pb::Codec::H264 as i32]
    );
    core.state::<SessionTable>().remove(entry.id);
    core.shutdown();
}

struct CaptureHello {
    addr: std::net::SocketAddr,
    fingerprint: [u8; 32],
    incoming: tokio::sync::mpsc::UnboundedReceiver<quinn::Connection>,
}

impl CaptureHello {
    async fn start() -> Self {
        let identity = LocalIdentity::generate_software(rsp_transport::identity::CertKind::Host)
            .expect("capture identity");
        let fingerprint = identity.fingerprint();
        let tls = server_config(&identity).expect("capture TLS");
        let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls).expect("QUIC TLS");
        let endpoint = quinn::Endpoint::server(
            quinn::ServerConfig::with_crypto(Arc::new(crypto)),
            (std::net::Ipv4Addr::LOCALHOST, 0).into(),
        )
        .expect("capture endpoint");
        let addr = endpoint.local_addr().expect("capture address");
        let (sender, incoming) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(connecting) = endpoint.accept().await {
                let Ok(connection) = connecting.await else {
                    continue;
                };
                if sender.send(connection).is_err() {
                    break;
                }
            }
        });
        Self {
            addr,
            fingerprint,
            incoming,
        }
    }

    async fn hello_for(
        &mut self,
        mut params: rsp_client::SessionParams,
    ) -> rsp_core::pb::ClientHello {
        params.host_fingerprint = self.fingerprint;
        params.candidates[0].addr = self.addr;
        let (handle, _events) = rsp_client::Session::open(params)
            .await
            .expect("open session");
        let connection = timeout(Duration::from_secs(2), self.incoming.recv())
            .await
            .expect("capture connection")
            .expect("capture channel");
        let (mut send, mut recv) = timeout(Duration::from_secs(2), connection.accept_bi())
            .await
            .expect("control stream")
            .expect("control stream");
        let mut decoder = EnvelopeDecoder::new();
        let hello = loop {
            let mut bytes = [0u8; 4096];
            let read = recv
                .read(&mut bytes)
                .await
                .expect("control read")
                .expect("control bytes");
            decoder.push(&bytes[..read]);
            if let Some(rsp_core::pb::Envelope {
                msg: Some(rsp_core::pb::envelope::Msg::ClientHello(hello)),
            }) = decoder.next().expect("hello record")
            {
                break hello;
            }
        };
        let reply = rsp_core::control::encode(&rsp_core::pb::Goodbye::default().into());
        send.write_all(&reply).await.expect("goodbye write");
        handle.close();
        hello
    }
}
