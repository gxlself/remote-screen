use std::{
    ffi::OsString,
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::Duration,
};

use host_core::config::VideoEncoder;
use host_fake::FakePlatform;
use host_win::{
    pipe_proto::{
        encode_frame, input_to_b64, read_frame, write_frame, Ctl, PipeError, PipeFrame,
        PIPE_PROTO_VERSION,
    },
    session_run::{
        args_for, handshake, parse_args, run_session, serve, usage, ArgsError, HandshakeError,
        RunExit, ServeEnd, CONNECT_TIMEOUT, HANDSHAKE_TIMEOUT, WRITER_DRAIN,
    },
};
use rsp_core::hid::InputMsg;
use tokio::io::{duplex, AsyncRead, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{timeout, Instant};

const TEST_TIMEOUT: Duration = Duration::from_secs(2);

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

async fn recv_ctl<R>(reader: &mut R) -> Ctl
where
    R: tokio::io::AsyncRead + Unpin,
{
    match timeout(TEST_TIMEOUT, read_frame(reader))
        .await
        .expect("frame read timeout")
        .expect("read frame")
        .expect("frame")
    {
        PipeFrame::Ctl(ctl) => ctl,
        other => panic!("expected control frame, got {other:?}"),
    }
}

async fn send_ctl<W: AsyncWrite + Unpin>(writer: &mut W, ctl: Ctl) {
    write_frame(writer, &PipeFrame::Ctl(ctl))
        .await
        .expect("write control");
    writer.flush().await.expect("flush control");
}

async fn finish(task: JoinHandle<ServeEnd>) -> ServeEnd {
    timeout(TEST_TIMEOUT, task)
        .await
        .expect("session exit timeout")
        .expect("session task")
}

#[derive(Clone, Copy)]
enum Failure {
    Write,
    Flush,
}

struct FailingStream {
    inner: DuplexStream,
    fail: Arc<AtomicBool>,
    at: Failure,
}

impl AsyncRead for FailingStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for FailingStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.fail.load(Ordering::Relaxed) && matches!(this.at, Failure::Write) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "write failed",
            )));
        }
        Pin::new(&mut this.inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.fail.load(Ordering::Relaxed) && matches!(this.at, Failure::Flush) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "flush failed",
            )));
        }
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[test]
fn parse_command_line() {
    let parsed = parse_args(args(&["--session", "2"])).expect("parse session");
    assert_eq!(parsed.session_id, 2);
    assert_eq!(parsed.pipe, r"\\.\pipe\RemoteHost.session.2");
    assert_eq!(parsed.log_dir, None);
    assert_eq!(parsed.log_level, None);
    assert_eq!(parsed.encoder, None);

    let parsed = parse_args(args(&[
        "--session=3",
        "--pipe",
        "X",
        "--log-dir",
        "D",
        "--log-level",
        "debug",
    ]))
    .expect("parse all options");
    assert_eq!(parsed.session_id, 3);
    assert_eq!(parsed.pipe, "X");
    assert_eq!(
        parsed.log_dir.as_deref().and_then(|path| path.to_str()),
        Some("D")
    );
    assert_eq!(parsed.log_level.as_deref(), Some("debug"));

    assert_eq!(
        parse_args(Vec::<OsString>::new()),
        Err(ArgsError::Missing("--session"))
    );
    assert_eq!(
        parse_args(args(&["--session", "abc"])),
        Err(ArgsError::Invalid {
            flag: "--session".into(),
            value: "abc".into(),
        })
    );
    assert_eq!(
        parse_args(args(&["--session", "0"])),
        Err(ArgsError::Invalid {
            flag: "--session".into(),
            value: "0".into(),
        })
    );
    assert_eq!(
        parse_args(args(&["--log-level", "loud"])),
        Err(ArgsError::Invalid {
            flag: "--log-level".into(),
            value: "loud".into(),
        })
    );
    assert_eq!(
        parse_args(args(&["--pipe"])),
        Err(ArgsError::Invalid {
            flag: "--pipe".into(),
            value: String::new(),
        })
    );
    assert_eq!(
        parse_args(args(&["--session", "2", "--session", "3"])),
        Err(ArgsError::Invalid {
            flag: "--session".into(),
            value: "3".into(),
        })
    );
    assert_eq!(
        parse_args(args(&["--bogus"])),
        Err(ArgsError::Unknown("--bogus".into()))
    );
    for flag in ["-h", "--help"] {
        assert_eq!(parse_args(args(&[flag])), Err(ArgsError::Help));
        assert!(usage().contains(flag));
    }
    assert_eq!(args_for(2, None), vec!["--session", "2"]);
    assert_eq!(
        parse_args(args_for(2, None).into_iter().map(OsString::from))
            .unwrap()
            .session_id,
        2
    );
    let encoded = args_for(2, Some(VideoEncoder::Nvenc));
    assert_eq!(encoded, vec!["--session", "2", "--encoder", "nvenc"]);
    assert_eq!(
        parse_args(encoded.into_iter().map(OsString::from))
            .unwrap()
            .encoder,
        Some(VideoEncoder::Nvenc)
    );
    assert_eq!(
        parse_args(args(&["--session", "2", "--encoder", "bogus"])),
        Err(ArgsError::Invalid {
            flag: "--encoder".into(),
            value: "bogus".into(),
        })
    );
    assert_eq!(
        parse_args(args(&["--session=2", "--encoder=openh264"]))
            .unwrap()
            .encoder,
        Some(VideoEncoder::OpenH264)
    );
    assert_eq!(
        parse_args(args(&["--session", "2", "--encoder", "openh264"]))
            .unwrap()
            .encoder,
        Some(VideoEncoder::OpenH264)
    );
    for flag in ["--session", "--pipe", "--log-dir", "--log-level"] {
        assert!(usage().contains(flag));
    }
    assert!(usage().contains("--encoder"));
}

#[test]
fn parse_rejects_overflow_missing_values_and_all_duplicate_flags() {
    let cases: &[(&[&str], &str, &str)] = &[
        (&["--session", "4294967296"], "--session", "4294967296"),
        (&["--session", "-1"], "--session", "-1"),
        (&["--session="], "--session", ""),
        (&["--session"], "--session", ""),
        (&["--log-dir"], "--log-dir", ""),
        (&["--log-level"], "--log-level", ""),
        (&["--session=2", "--session=3"], "--session", "3"),
        (&["--session", "2", "--session=3"], "--session", "3"),
        (&["--pipe", "X", "--pipe", "Y"], "--pipe", "Y"),
        (&["--log-dir", "D", "--log-dir", "E"], "--log-dir", "E"),
        (
            &["--log-level", "info", "--log-level", "debug"],
            "--log-level",
            "debug",
        ),
    ];
    for (values, flag, value) in cases {
        assert_eq!(
            parse_args(args(values)),
            Err(ArgsError::Invalid {
                flag: (*flag).into(),
                value: (*value).into()
            }),
            "arguments: {values:?}"
        );
    }
    for level in ["trace", "debug", "info", "warn", "error"] {
        let parsed = parse_args(args(&["--session", "2", "--log-level", level])).unwrap();
        assert_eq!(parsed.log_level.as_deref(), Some(level));
    }
    for session_id in [1, 2, 3, u32::MAX] {
        let parsed =
            parse_args(args_for(session_id, None).into_iter().map(OsString::from)).unwrap();
        assert_eq!(parsed.session_id, session_id);
    }
}

#[cfg(unix)]
#[test]
fn parse_rejects_non_utf8_values_and_arguments() {
    use std::os::unix::ffi::OsStringExt;

    let invalid = OsString::from_vec(vec![0xff]);
    let lossy = invalid.to_string_lossy().into_owned();
    for flag in ["--session", "--pipe", "--log-dir", "--log-level"] {
        assert_eq!(
            parse_args([OsString::from(flag), invalid.clone()]),
            Err(ArgsError::Invalid {
                flag: flag.into(),
                value: lossy.clone()
            })
        );
    }
    assert_eq!(
        parse_args([invalid.clone()]),
        Err(ArgsError::Invalid {
            flag: lossy.clone(),
            value: lossy.clone()
        })
    );
    let mut inline = OsString::from("--session=");
    inline.push(invalid);
    assert_eq!(
        parse_args([inline]),
        Err(ArgsError::Invalid {
            flag: "--session".into(),
            value: lossy
        })
    );
}

#[test]
fn exit_codes_are_stable() {
    assert_eq!(RunExit::ServiceClosed.code(), 0);
    assert_eq!(RunExit::BadArgs.code(), 2);
    assert_eq!(RunExit::ConnectFailed.code(), 3);
    assert_eq!(RunExit::HandshakeFailed.code(), 4);
    assert_eq!(RunExit::PipeError.code(), 5);
    assert_eq!(
        CONNECT_TIMEOUT,
        host_win::supervisor::SupervisorConfig::default().connect_timeout
    );
    assert_eq!(HANDSHAKE_TIMEOUT, CONNECT_TIMEOUT);
}

#[tokio::test(start_paused = true)]
async fn handshake_accepts_matching_welcome() {
    let (mut client, mut service) = duplex(64 * 1024);
    let task =
        tokio::spawn(async move { handshake(&mut client, 2, 1234, Duration::from_secs(1)).await });
    assert_eq!(
        recv_ctl(&mut service).await,
        Ctl::Hello {
            version: PIPE_PROTO_VERSION,
            session_id: 2,
            pid: 1234,
        }
    );
    write_frame(
        &mut service,
        &PipeFrame::Ctl(Ctl::Welcome {
            version: PIPE_PROTO_VERSION,
        }),
    )
    .await
    .expect("write welcome");
    assert!(task.await.expect("handshake task").is_ok());
}

#[tokio::test(start_paused = true)]
async fn handshake_rejects_bad_frames_and_close() {
    let cases = [
        (
            Ctl::Welcome { version: 2 },
            HandshakeError::Version { got: 2 },
        ),
        (Ctl::Ping { n: 1 }, HandshakeError::Unexpected),
    ];
    for (response, expected) in cases {
        let (mut client, mut service) = duplex(64 * 1024);
        let task =
            tokio::spawn(
                async move { handshake(&mut client, 2, 1234, Duration::from_secs(1)).await },
            );
        let _ = recv_ctl(&mut service).await;
        write_frame(&mut service, &PipeFrame::Ctl(response))
            .await
            .expect("write response");
        let result = task.await.expect("handshake task");
        match expected {
            HandshakeError::Version { got } => assert!(matches!(
                result,
                Err(HandshakeError::Version { got: actual }) if actual == got
            )),
            HandshakeError::Unexpected => {
                assert!(matches!(result, Err(HandshakeError::Unexpected)))
            }
            _ => unreachable!("unexpected test case"),
        }
    }

    let (mut client, mut service) = duplex(64 * 1024);
    let task =
        tokio::spawn(async move { handshake(&mut client, 2, 1234, Duration::from_secs(1)).await });
    let _ = recv_ctl(&mut service).await;
    drop(service);
    assert!(matches!(
        task.await.expect("handshake task"),
        Err(HandshakeError::Closed)
    ));
}

#[tokio::test(start_paused = true)]
async fn handshake_times_out() {
    let (mut client, mut service) = duplex(64 * 1024);
    let started = Instant::now();
    let task =
        tokio::spawn(async move { handshake(&mut client, 2, 1234, HANDSHAKE_TIMEOUT).await });
    let _ = recv_ctl(&mut service).await;
    assert!(matches!(
        task.await.expect("handshake task"),
        Err(HandshakeError::Timeout)
    ));
    assert!(started.elapsed() >= HANDSHAKE_TIMEOUT);
    assert!(started.elapsed() < HANDSHAKE_TIMEOUT + Duration::from_secs(1));
}

#[tokio::test(start_paused = true)]
async fn handshake_reports_read_write_and_flush_errors() {
    let (mut client, mut service) = duplex(64 * 1024);
    let task =
        tokio::spawn(async move { handshake(&mut client, 2, 1234, HANDSHAKE_TIMEOUT).await });
    let _ = recv_ctl(&mut service).await;
    service.write_all(&[0, 0, 0, 0]).await.unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(HandshakeError::Pipe(PipeError::BadFrame("frame length")))
    ));

    for at in [Failure::Write, Failure::Flush] {
        let (client, _service) = duplex(64 * 1024);
        let mut stream = FailingStream {
            inner: client,
            fail: Arc::new(AtomicBool::new(true)),
            at,
        };
        assert!(matches!(
            handshake(&mut stream, 2, 1234, HANDSHAKE_TIMEOUT).await,
            Err(HandshakeError::Pipe(PipeError::Io(error)))
                if error.kind() == io::ErrorKind::BrokenPipe
        ));
    }
}

#[tokio::test(start_paused = true)]
async fn handshake_deadline_includes_writing_hello() {
    let (mut client, _service) = duplex(1);
    let started = Instant::now();
    assert!(matches!(
        handshake(&mut client, 2, 1234, HANDSHAKE_TIMEOUT).await,
        Err(HandshakeError::Timeout)
    ));
    assert!(started.elapsed() >= HANDSHAKE_TIMEOUT);
    assert!(started.elapsed() < HANDSHAKE_TIMEOUT + Duration::from_secs(1));
}

#[tokio::test]
async fn serve_dispatches_ping_inject_and_desktop() {
    let (client, mut service) = duplex(64 * 1024);
    let (desktop_tx, desktop_rx) = mpsc::unbounded_channel();
    let fake = Arc::new(FakePlatform::new());
    let release_before = fake.release_count();
    let task = tokio::spawn(serve(client, Arc::clone(&fake), desktop_rx));

    match recv_ctl(&mut service).await {
        Ctl::Displays { list } => assert!(list.iter().any(|display| display.id == 1)),
        other => panic!("expected displays, got {other:?}"),
    }

    write_frame(&mut service, &PipeFrame::Ctl(Ctl::Ping { n: 7 }))
        .await
        .expect("write ping");
    assert_eq!(recv_ctl(&mut service).await, Ctl::Pong { n: 7 });

    let input = InputMsg::MouseMove { x: 0.25, y: 0.75 };
    write_frame(
        &mut service,
        &PipeFrame::Ctl(Ctl::Inject {
            display_id: 2,
            input: input_to_b64(&input).expect("encode input"),
        }),
    )
    .await
    .expect("write input");
    write_frame(&mut service, &PipeFrame::Ctl(Ctl::Ping { n: 8 }))
        .await
        .expect("write ping");
    assert_eq!(recv_ctl(&mut service).await, Ctl::Pong { n: 8 });
    assert_eq!(fake.injected_with_display().last(), Some(&(input, 1)));

    desktop_tx.send("Winlogon".into()).expect("desktop send");
    assert_eq!(
        recv_ctl(&mut service).await,
        Ctl::Desktop {
            name: "Winlogon".into()
        }
    );

    drop(desktop_tx);
    write_frame(&mut service, &PipeFrame::Ctl(Ctl::Ping { n: 9 }))
        .await
        .expect("write ping");
    assert_eq!(recv_ctl(&mut service).await, Ctl::Pong { n: 9 });

    drop(service);
    let end = finish(task).await;
    assert_eq!(end.exit, RunExit::ServiceClosed);
    assert_eq!(end.error, None);
    assert_eq!(end.frames_in, 4);
    assert_eq!(end.frames_out, 5);
    assert_eq!(fake.release_count(), release_before + 1);
}

#[tokio::test]
async fn serve_reports_bad_frame_and_releases_platform() {
    let (client, mut service) = duplex(64 * 1024);
    let (_desktop_tx, desktop_rx) = mpsc::unbounded_channel();
    let fake = Arc::new(FakePlatform::new());
    let release_before = fake.release_count();
    let task = tokio::spawn(serve(client, Arc::clone(&fake), desktop_rx));
    let _ = recv_ctl(&mut service).await;
    service
        .write_all(&[0, 0, 0, 0])
        .await
        .expect("write bad frame");
    let end = finish(task).await;
    assert_eq!(end.exit, RunExit::PipeError);
    assert!(end
        .error
        .as_deref()
        .is_some_and(|error| error.contains("frame length")));
    assert_eq!(fake.release_count(), release_before + 1);
    assert_eq!(end.frames_in, 0);
    assert_eq!(end.frames_out, 1);
}

#[tokio::test]
async fn closed_desktop_channel_still_allows_ping() {
    let (client, mut service) = duplex(64 * 1024);
    let (desktop_tx, desktop_rx) = mpsc::unbounded_channel();
    drop(desktop_tx);
    let task = tokio::spawn(serve(client, Arc::new(FakePlatform::new()), desktop_rx));
    assert!(matches!(recv_ctl(&mut service).await, Ctl::Displays { .. }));
    send_ctl(&mut service, Ctl::Ping { n: 7 }).await;
    assert_eq!(recv_ctl(&mut service).await, Ctl::Pong { n: 7 });
    service.shutdown().await.unwrap();
    let end = finish(task).await;
    assert_eq!(end.exit, RunExit::ServiceClosed);
    assert_eq!(end.frames_in, 1);
    assert_eq!(end.frames_out, 2);
}

#[tokio::test(start_paused = true)]
async fn partial_frame_survives_desktop_notification() {
    let (client, mut service) = duplex(64 * 1024);
    let (desktop_tx, desktop_rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(serve(client, Arc::new(FakePlatform::new()), desktop_rx));
    assert!(matches!(recv_ctl(&mut service).await, Ctl::Displays { .. }));
    let bytes = encode_frame(&PipeFrame::Ctl(Ctl::Ping { n: 42 })).unwrap();
    service.write_all(&bytes[..2]).await.unwrap();
    // Let the dedicated reader consume the first byte of the length prefix.
    tokio::task::yield_now().await;
    let (mut read, mut write) = tokio::io::split(service);
    let started = Instant::now();
    tokio::join!(
        async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            write.write_all(&bytes[2..]).await.unwrap();
        },
        async {
            desktop_tx.send("Default".into()).unwrap();
            assert_eq!(
                recv_ctl(&mut read).await,
                Ctl::Desktop {
                    name: "Default".into()
                }
            );
            assert!(started.elapsed() < Duration::from_millis(50));
            assert_eq!(recv_ctl(&mut read).await, Ctl::Pong { n: 42 });
        }
    );
    assert!(started.elapsed() >= Duration::from_millis(50));
    assert!(started.elapsed() < Duration::from_millis(100));
    write.shutdown().await.unwrap();
    let end = finish(task).await;
    assert_eq!(end.exit, RunExit::ServiceClosed);
    assert_eq!(end.error, None);
    assert_eq!(end.frames_in, 1);
    assert_eq!(end.frames_out, 3);
}

#[tokio::test(start_paused = true)]
async fn eof_drains_queued_replies_and_releases_platform() {
    let (client, mut service) = duplex(64 * 1024);
    let (_desktop_tx, desktop_rx) = mpsc::unbounded_channel();
    let fake = Arc::new(FakePlatform::new());
    let releases = fake.release_count();
    let task = tokio::spawn(serve(client, Arc::clone(&fake), desktop_rx));
    for n in 0..3 {
        send_ctl(&mut service, Ctl::Ping { n }).await;
    }
    service.shutdown().await.unwrap();
    let end = finish(task).await;
    assert_eq!(end.exit, RunExit::ServiceClosed);
    assert_eq!(end.error, None);
    assert_eq!(end.frames_in, 3);
    assert_eq!(end.frames_out, 4);
    assert_eq!(fake.release_count(), releases + 1);
    assert!(matches!(recv_ctl(&mut service).await, Ctl::Displays { .. }));
    for n in 0..3 {
        assert_eq!(recv_ctl(&mut service).await, Ctl::Pong { n });
    }
    assert_eq!(read_frame(&mut service).await.unwrap(), None);
}

#[tokio::test(start_paused = true)]
async fn writer_drain_is_bounded_when_service_never_reads() {
    let (client, mut service) = duplex(64);
    let (_desktop_tx, desktop_rx) = mpsc::unbounded_channel();
    let fake = Arc::new(FakePlatform::new());
    let releases = fake.release_count();
    let started = Instant::now();
    let task = tokio::spawn(serve(client, Arc::clone(&fake), desktop_rx));
    service.shutdown().await.unwrap();
    let end = finish(task).await;
    assert_eq!(end.exit, RunExit::ServiceClosed);
    assert_eq!(end.error, None);
    assert_eq!(end.frames_in, 0);
    assert_eq!(end.frames_out, 0);
    assert_eq!(fake.release_count(), releases + 1);
    assert!(started.elapsed() >= WRITER_DRAIN);
    assert!(started.elapsed() <= WRITER_DRAIN + Duration::from_millis(100));
}

#[tokio::test(start_paused = true)]
async fn writer_errors_end_session_and_only_count_flushed_frames() {
    for (at, expected) in [
        (Failure::Write, "write failed"),
        (Failure::Flush, "flush failed"),
    ] {
        let (client, mut service) = duplex(64 * 1024);
        let (_desktop_tx, desktop_rx) = mpsc::unbounded_channel();
        let fake = Arc::new(FakePlatform::new());
        let releases = fake.release_count();
        let fail = Arc::new(AtomicBool::new(false));
        let stream = FailingStream {
            inner: client,
            fail: Arc::clone(&fail),
            at,
        };
        let task = tokio::spawn(serve(stream, Arc::clone(&fake), desktop_rx));
        assert!(matches!(recv_ctl(&mut service).await, Ctl::Displays { .. }));
        // Ensure the first flush/count completed before enabling failures.
        tokio::task::yield_now().await;
        fail.store(true, Ordering::Relaxed);
        send_ctl(&mut service, Ctl::Ping { n: 7 }).await;
        let end = finish(task).await;
        assert_eq!(end.exit, RunExit::PipeError);
        assert_eq!(end.error.as_deref(), Some(expected));
        assert_eq!(end.frames_in, 1);
        assert_eq!(end.frames_out, 1);
        assert_eq!(fake.release_count(), releases + 1);
    }
}

#[tokio::test(start_paused = true)]
async fn service_close_stops_active_capture() {
    let (client, mut service) = duplex(64 * 1024);
    let (_desktop_tx, desktop_rx) = mpsc::unbounded_channel();
    let fake = Arc::new(FakePlatform::new());
    let releases = fake.release_count();
    let task = tokio::spawn(serve(client, Arc::clone(&fake), desktop_rx));
    assert!(matches!(recv_ctl(&mut service).await, Ctl::Displays { .. }));
    send_ctl(
        &mut service,
        Ctl::CaptureStart {
            req: 1,
            cap: 1,
            display: 1,
            fps: 30,
            max_width: 640,
            max_height: 360,
            region: None,
        },
    )
    .await;
    assert_eq!(
        recv_ctl(&mut service).await,
        Ctl::Done {
            req: 1,
            error: None
        }
    );
    assert_eq!(fake.active_captures(), 1);
    service.shutdown().await.unwrap();
    let end = finish(task).await;
    assert_eq!(end.exit, RunExit::ServiceClosed);
    assert_eq!(fake.active_captures(), 0);
    assert_eq!(fake.release_count(), releases + 1);
}

#[tokio::test(start_paused = true)]
async fn run_session_rejects_handshake_without_starting_server() {
    let (client, mut service) = duplex(64 * 1024);
    let (_desktop_tx, desktop_rx) = mpsc::unbounded_channel();
    let fake = Arc::new(FakePlatform::new());
    let releases = fake.release_count();
    let task = tokio::spawn(run_session(
        client,
        Arc::clone(&fake),
        desktop_rx,
        2,
        1234,
        HANDSHAKE_TIMEOUT,
    ));
    assert!(matches!(recv_ctl(&mut service).await, Ctl::Hello { .. }));
    send_ctl(&mut service, Ctl::Welcome { version: 2 }).await;
    let end = finish(task).await;
    assert_eq!(end.exit, RunExit::HandshakeFailed);
    assert_eq!(end.exit.code(), 4);
    assert_eq!(end.frames_in, 0);
    assert_eq!(end.frames_out, 0);
    assert_eq!(
        end.error,
        Some(HandshakeError::Version { got: 2 }.to_string())
    );
    assert_eq!(fake.release_count(), releases);
    assert_eq!(read_frame(&mut service).await.unwrap(), None);
}

#[tokio::test(start_paused = true)]
async fn run_session_discards_connection_after_partial_handshake_timeout() {
    let (client, mut service) = duplex(64 * 1024);
    let (_desktop_tx, desktop_rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(run_session(
        client,
        Arc::new(FakePlatform::new()),
        desktop_rx,
        2,
        1234,
        HANDSHAKE_TIMEOUT,
    ));
    assert!(matches!(recv_ctl(&mut service).await, Ctl::Hello { .. }));
    service.write_all(&[10, 0]).await.unwrap();
    let end = timeout(HANDSHAKE_TIMEOUT + TEST_TIMEOUT, task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(end.exit, RunExit::HandshakeFailed);
    assert_eq!(end.error, Some(HandshakeError::Timeout.to_string()));
    assert_eq!(end.frames_in, 0);
    assert_eq!(end.frames_out, 0);
    assert_eq!(read_frame(&mut service).await.unwrap(), None);
}

#[tokio::test(start_paused = true)]
async fn run_session_handles_handshake_then_service_close() {
    let (client, mut service) = duplex(64 * 1024);
    let (_desktop_tx, desktop_rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(run_session(
        client,
        Arc::new(FakePlatform::new()),
        desktop_rx,
        2,
        1234,
        Duration::from_secs(1),
    ));
    assert!(matches!(recv_ctl(&mut service).await, Ctl::Hello { .. }));
    write_frame(
        &mut service,
        &PipeFrame::Ctl(Ctl::Welcome {
            version: PIPE_PROTO_VERSION,
        }),
    )
    .await
    .expect("write welcome");
    assert!(matches!(recv_ctl(&mut service).await, Ctl::Displays { .. }));
    drop(service);
    let end = finish(task).await;
    assert_eq!(end.exit, RunExit::ServiceClosed);
    assert_eq!(end.error, None);
    assert_eq!(end.frames_in, 0);
    assert_eq!(end.frames_out, 1);
}
