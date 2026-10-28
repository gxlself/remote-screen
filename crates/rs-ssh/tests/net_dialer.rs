mod support;

use std::{
    collections::HashMap, future::Future, io, net::SocketAddr, sync::Arc, sync::Mutex,
    time::Duration,
};

use rs_ssh::{MemoryHostKeyStore, Prompter, SshError, SshOptions, SshSession, SshTarget};
use rsp_transport::{net::NetFuture, NetDialer, NetStream, SystemNetDialer};
use russh::{server, Channel, ChannelId};
use support::sftp_server::{credential, private, ssh_options, SftpTestServer, CLIENT_KEY_SEED};
use tokio::time::{timeout, Instant};

#[derive(Debug, Default)]
enum Mode {
    #[default]
    System,
    Refuse,
    Hang,
}

#[derive(Debug, Default)]
struct RecordingNet {
    system: SystemNetDialer,
    calls: Mutex<Vec<(String, u16)>>,
    mode: Mode,
}

impl NetDialer for RecordingNet {
    fn shared_quic_endpoint(&self, bind: SocketAddr) -> io::Result<Option<quinn::Endpoint>> {
        self.system.shared_quic_endpoint(bind)
    }

    fn dial_udp(&self, peer: SocketAddr) -> NetFuture<'_, Arc<dyn quinn::AsyncUdpSocket>> {
        self.system.dial_udp(peer)
    }

    fn dial_tcp(&self, peer: SocketAddr) -> NetFuture<'_, NetStream> {
        self.system.dial_tcp(peer)
    }

    fn dial_tcp_host<'a>(&'a self, host: &'a str, port: u16) -> NetFuture<'a, NetStream> {
        self.calls.lock().unwrap().push((host.to_owned(), port));
        match self.mode {
            Mode::System => self.system.dial_tcp_host(host, port),
            Mode::Refuse => Box::pin(async { Err(io::ErrorKind::ConnectionRefused.into()) }),
            Mode::Hang => Box::pin(std::future::pending()),
        }
    }
}

struct AcceptHostKey;

#[async_trait::async_trait]
impl Prompter for AcceptHostKey {
    async fn confirm_new_host_key(&self, _: &str, _: &str, _: &str) -> bool {
        true
    }

    async fn keyboard_interactive(
        &self,
        _: &str,
        _: &str,
        _: &[(String, bool)],
    ) -> Option<Vec<String>> {
        None
    }

    fn auth_url(&self, _: &str) {}
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    timeout(Duration::from_secs(5), future)
        .await
        .expect("local SSH operation timed out")
}

async fn connect(target: SshTarget, options: SshOptions) -> Result<SshSession, SshError> {
    SshSession::connect(
        target,
        vec![credential(CLIENT_KEY_SEED)],
        Arc::new(MemoryHostKeyStore::default()),
        Arc::new(AcceptHostKey),
        options,
    )
    .await
}

#[tokio::test]
async fn direct_connect_uses_net() {
    let server = SftpTestServer::start_temporary("net-dialer").await;
    let target = server.target();
    let net = Arc::new(RecordingNet::default());
    let session = bounded(connect(
        target.clone(),
        SshOptions {
            net: net.clone(),
            ..ssh_options()
        },
    ))
    .await
    .unwrap();
    // The shared fixture implements SFTP only; execute a real subsystem request.
    let sftp = bounded(session.sftp()).await.unwrap();
    assert_eq!(bounded(sftp.home()).await.unwrap(), "/home/test");
    assert_eq!(*net.calls.lock().unwrap(), vec![(target.host, target.port)]);
    sftp.close().await;
    session.close().await;
}

#[tokio::test]
async fn net_error_maps_to_transport() {
    let server = SftpTestServer::start_temporary("net-refused").await;
    let target = server.target();
    let net = Arc::new(RecordingNet {
        mode: Mode::Refuse,
        ..Default::default()
    });
    let error = bounded(connect(
        target.clone(),
        SshOptions {
            net: net.clone(),
            ..ssh_options()
        },
    ))
    .await
    .err()
    .unwrap();
    assert_eq!(
        error,
        SshError::Transport(io::Error::from(io::ErrorKind::ConnectionRefused).to_string())
    );
    assert_eq!(*net.calls.lock().unwrap(), vec![(target.host, target.port)]);
}

#[tokio::test]
async fn net_hang_maps_to_timeout() {
    let server = SftpTestServer::start_temporary("net-timeout").await;
    let target = server.target();
    let net = Arc::new(RecordingNet {
        mode: Mode::Hang,
        ..Default::default()
    });
    let started = Instant::now();
    let error = timeout(
        Duration::from_secs(1),
        connect(
            target.clone(),
            SshOptions {
                net: net.clone(),
                connect_timeout: Duration::from_millis(200),
                ..ssh_options()
            },
        ),
    )
    .await
    .expect("network timeout returns within one second")
    .err()
    .unwrap();
    assert_eq!(error, SshError::Timeout);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(*net.calls.lock().unwrap(), vec![(target.host, target.port)]);
}

// Keep exec support local: the shared SFTP fixture deliberately has no exec handler.
#[derive(Default)]
struct ExecHandler {
    channels: HashMap<ChannelId, Channel<server::Msg>>,
}

impl server::Handler for ExecHandler {
    type Error = russh::Error;

    async fn auth_publickey(
        &mut self,
        _: &str,
        key: &ssh_key::PublicKey,
    ) -> Result<server::Auth, Self::Error> {
        Ok(if key == private(CLIENT_KEY_SEED).public_key() {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        cmd: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        assert_eq!(cmd, b"echo net-dialer");
        session.channel_success(channel)?;
        session.data(channel, b"net-dialer\n".to_vec())?;
        session.exit_status_request(channel, 0)?;
        session.eof(channel)?;
        session.close(channel)
    }
}

#[tokio::test]
async fn direct_exec_uses_net() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = async {
        let (stream, _) = listener.accept().await.unwrap();
        let config = Arc::new(server::Config {
            keys: vec![private(1)],
            auth_rejection_time: Duration::from_millis(1),
            auth_rejection_time_initial: Some(Duration::from_millis(1)),
            ..Default::default()
        });
        let peer = server::run_stream(config, stream, ExecHandler::default())
            .await
            .unwrap();
        let _ = peer.await;
    };
    let net = Arc::new(RecordingNet::default());
    let client = async {
        let session = connect(
            SshTarget {
                host: "127.0.0.1".into(),
                port: addr.port(),
                user: "test".into(),
                jump: None,
            },
            SshOptions {
                net: net.clone(),
                ..ssh_options()
            },
        )
        .await
        .unwrap();
        let output = session.exec("echo net-dialer").await.unwrap();
        assert_eq!(output.status, Some(0));
        assert_eq!(output.stdout, b"net-dialer\n");
        assert_eq!(
            *net.calls.lock().unwrap(),
            vec![("127.0.0.1".into(), addr.port())]
        );
        session.close().await;
    };
    bounded(async { tokio::join!(server, client) }).await;
}
