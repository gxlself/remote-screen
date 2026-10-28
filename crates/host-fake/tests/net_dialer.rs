use std::{future::Future, io, net::SocketAddr, sync::Arc, sync::Mutex, time::Duration};

use host_core::platform::InputEvent;
use host_fake::{
    faultnet::{Dir, FaultProxy, UdpRule},
    harness::{app_identity, direct_params, params, wait_frames, TestHost, TestHostOptions},
    FakePlatform,
};
use rsp_client::{Session, SessionHandle};
use rsp_core::{hid::InputMsg, pb, trust::Permission};
use rsp_transport::{net::NetFuture, NetDialer, NetStream, SystemNetDialer};
use tokio::time::{sleep, timeout};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    SharedQuicEndpoint(SocketAddr),
    DialUdp(SocketAddr),
    DialTcp(SocketAddr),
    DialTcpHost(String, u16),
}

#[derive(Debug, Default)]
struct RecordingNet {
    system: SystemNetDialer,
    calls: Mutex<Vec<Call>>,
    per_peer: bool,
}

impl RecordingNet {
    fn record(&self, call: Call) {
        self.calls.lock().unwrap().push(call);
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

impl NetDialer for RecordingNet {
    fn shared_quic_endpoint(&self, bind: SocketAddr) -> io::Result<Option<quinn::Endpoint>> {
        self.record(Call::SharedQuicEndpoint(bind));
        if self.per_peer {
            Ok(None)
        } else {
            self.system.shared_quic_endpoint(bind)
        }
    }

    fn dial_udp(&self, peer: SocketAddr) -> NetFuture<'_, Arc<dyn quinn::AsyncUdpSocket>> {
        self.record(Call::DialUdp(peer));
        self.system.dial_udp(peer)
    }

    fn dial_tcp(&self, peer: SocketAddr) -> NetFuture<'_, NetStream> {
        self.record(Call::DialTcp(peer));
        self.system.dial_tcp(peer)
    }

    fn dial_tcp_host<'a>(&'a self, host: &'a str, port: u16) -> NetFuture<'a, NetStream> {
        self.record(Call::DialTcpHost(host.to_owned(), port));
        self.system.dial_tcp_host(host, port)
    }
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    timeout(Duration::from_secs(5), future)
        .await
        .expect("local network operation completes within five seconds")
}

async fn hud_transport(session: &SessionHandle) -> pb::Transport {
    bounded(async {
        loop {
            if let Some(hud) = session.stats().hud {
                return hud.transport;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_quic_goes_through_net() {
    let host = TestHost::start().await;
    let identity = app_identity();
    host.pair(&identity, Permission::Control);
    let net = Arc::new(RecordingNet::default());
    let mut params = direct_params(&host, identity);
    params.net = net.clone();
    let (session, mut events) = bounded(Session::open(params)).await.unwrap();
    let report = wait_frames(&mut events, 1, Duration::from_secs(3)).await;
    assert_eq!(report.frames, 1, "{report:?}");
    assert_eq!(hud_transport(&session).await, pb::Transport::Quic);
    assert_eq!(
        net.calls(),
        vec![Call::SharedQuicEndpoint(([127, 0, 0, 1], 0).into())]
    );
    session.close();
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_per_peer_quic() {
    let host = TestHost::start().await;
    let identity = app_identity();
    host.pair(&identity, Permission::Control);
    let net = Arc::new(RecordingNet {
        per_peer: true,
        ..Default::default()
    });
    let mut params = direct_params(&host, identity);
    params.net = net.clone();
    let (session, mut events) = bounded(Session::open(params)).await.unwrap();
    let report = wait_frames(&mut events, 1, Duration::from_secs(3)).await;
    assert_eq!(report.frames, 1, "{report:?}");
    assert_eq!(hud_transport(&session).await, pb::Transport::Quic);
    assert_eq!(
        net.calls(),
        vec![
            Call::SharedQuicEndpoint(([127, 0, 0, 1], 0).into()),
            Call::DialUdp(host.quic_addr),
        ]
    );
    session.close();
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_tcp_fallback_and_input_go_through_net() {
    let platform = FakePlatform::new();
    let host = TestHost::start_with_platform(
        platform.clone(),
        TestHostOptions {
            tcp: true,
            ..Default::default()
        },
    )
    .await;
    let proxy = FaultProxy::start(host.quic_addr, host.tcp_addr, 0xa17e_0009)
        .await
        .unwrap();
    for dir in [Dir::ToHost, Dir::ToClient] {
        proxy.set_udp(
            dir,
            UdpRule {
                blackhole: true,
                ..Default::default()
            },
        );
    }
    let identity = app_identity();
    host.pair(&identity, Permission::Control);
    let net = Arc::new(RecordingNet::default());
    let mut params = params(&host, identity, &[proxy.addr()]);
    params.net = net.clone();
    params.dial_options.tcp_after = Duration::from_millis(100);
    let (session, mut events) = bounded(Session::open(params)).await.unwrap();
    let report = wait_frames(&mut events, 1, Duration::from_secs(3)).await;
    assert_eq!(report.frames, 1, "{report:?}");
    assert_eq!(hud_transport(&session).await, pb::Transport::Tcp);
    session
        .send_input(InputMsg::MouseMove { x: 0.25, y: 0.75 })
        .unwrap();
    bounded(async {
        while !platform
            .injected_events()
            .contains(&InputEvent::MouseMove { x: 0.25, y: 0.75 })
        {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert_eq!(
        net.calls()
            .iter()
            .filter(|call| matches!(call, Call::DialTcp(_)))
            .cloned()
            .collect::<Vec<_>>(),
        vec![Call::DialTcp(proxy.addr()), Call::DialTcp(proxy.addr())]
    );
    assert!(proxy.stats().to_host.dropped > 0);
    session.close();
    proxy.shutdown().await;
    host.stop().await;
}
