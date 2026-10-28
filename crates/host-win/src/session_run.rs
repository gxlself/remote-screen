//! Platform-independent command line, handshake and session pipe loop.

use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use host_core::{config::VideoEncoder, platform::Platform};
use tokio::{
    io::{split, AsyncRead, AsyncWrite, AsyncWriteExt},
    sync::mpsc::{self, UnboundedReceiver},
};

use crate::{
    pipe_proto::{
        read_frame, session_pipe_name, write_frame, Ctl, PipeError, PipeFrame, PIPE_PROTO_VERSION,
    },
    session_server::SessionServer,
};

/// Maximum time allowed for the session process to receive the service's
/// welcome frame.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum time allowed for the session process to connect its pipe.  The
/// Windows connection code consumes this constant.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum time spent draining frames already queued for the writer.
pub const WRITER_DRAIN: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq)]
pub struct SessionArgs {
    pub session_id: u32,
    /// Defaults to `session_pipe_name(session_id)`.
    pub pipe: String,
    pub log_dir: Option<PathBuf>,
    pub log_level: Option<String>,
    pub encoder: Option<VideoEncoder>,
}

impl Eq for SessionArgs {}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArgsError {
    #[error("missing required argument {0}")]
    Missing(&'static str),
    #[error("invalid value {value:?} for {flag}")]
    Invalid { flag: String, value: String },
    #[error("unknown argument {0:?}")]
    Unknown(String),
    #[error("help requested")]
    Help,
}

fn invalid_value(flag: &str, value: &str) -> ArgsError {
    ArgsError::Invalid {
        flag: flag.to_owned(),
        value: value.to_owned(),
    }
}

/// Parse the command line after argv[0].
pub fn parse_args<I: IntoIterator<Item = OsString>>(args: I) -> Result<SessionArgs, ArgsError> {
    let mut args = args.into_iter();
    let mut session_id = None;
    let mut pipe = None;
    let mut log_dir = None;
    let mut log_level = None;
    let mut encoder = None;

    while let Some(arg) = args.next() {
        let arg = arg.into_string().map_err(|arg| {
            let arg = arg.to_string_lossy();
            let (flag, value) = arg.split_once('=').unwrap_or((&arg, &arg));
            invalid_value(flag, value)
        })?;
        let (flag, inline_value) = match arg.strip_prefix("--session=") {
            Some(value) => ("--session", Some(value.to_owned())),
            None => match arg.strip_prefix("--encoder=") {
                Some(value) => ("--encoder", Some(value.to_owned())),
                None => (arg.as_str(), None),
            },
        };
        match flag {
            "-h" | "--help" => return Err(ArgsError::Help),
            "--session" | "--pipe" | "--log-dir" | "--log-level" | "--encoder" => {}
            _ => return Err(ArgsError::Unknown(arg)),
        }
        let value = match inline_value {
            Some(value) => value,
            None => args
                .next()
                .ok_or_else(|| invalid_value(flag, ""))?
                .into_string()
                .map_err(|value| invalid_value(flag, &value.to_string_lossy()))?,
        };
        match flag {
            "--session" if session_id.is_none() => {
                session_id = Some(
                    value
                        .parse::<u32>()
                        .ok()
                        .filter(|id| *id != 0)
                        .ok_or_else(|| invalid_value(flag, &value))?,
                );
            }
            "--pipe" if pipe.is_none() => pipe = Some(value),
            "--log-dir" if log_dir.is_none() => log_dir = Some(PathBuf::from(value)),
            "--log-level"
                if log_level.is_none()
                    && matches!(
                        value.as_str(),
                        "trace" | "debug" | "info" | "warn" | "error"
                    ) =>
            {
                log_level = Some(value);
            }
            "--encoder" if encoder.is_none() => {
                encoder = Some(match value.as_str() {
                    "auto" => VideoEncoder::Auto,
                    "nvenc" => VideoEncoder::Nvenc,
                    "amf" => VideoEncoder::Amf,
                    "qsv" => VideoEncoder::Qsv,
                    "mf" => VideoEncoder::Mf,
                    "openh264" => VideoEncoder::OpenH264,
                    _ => return Err(invalid_value(flag, &value)),
                });
            }
            _ => return Err(invalid_value(flag, &value)),
        }
    }

    let session_id = session_id.ok_or(ArgsError::Missing("--session"))?;
    Ok(SessionArgs {
        session_id,
        pipe: pipe.unwrap_or_else(|| session_pipe_name(session_id)),
        log_dir,
        log_level,
        encoder,
    })
}

/// Arguments used by the service when it launches a session process.
pub fn args_for(session_id: u32, encoder: Option<VideoEncoder>) -> Vec<String> {
    let mut args = vec!["--session".to_owned(), session_id.to_string()];
    if let Some(encoder) = encoder {
        args.extend(["--encoder".to_owned(), encoder_name(encoder).to_owned()]);
    }
    args
}

fn encoder_name(encoder: VideoEncoder) -> &'static str {
    match encoder {
        VideoEncoder::Auto => "auto",
        VideoEncoder::Nvenc => "nvenc",
        VideoEncoder::Amf => "amf",
        VideoEncoder::Qsv => "qsv",
        VideoEncoder::Mf => "mf",
        VideoEncoder::OpenH264 => "openh264",
    }
}

/// Short command-line help text for the session process.
pub fn usage() -> &'static str {
    "Usage: rs-host-session.exe --session <n> [--encoder <auto|nvenc|amf|qsv|mf|openh264>] [--pipe <name>] [--log-dir <path>] [--log-level <trace|debug|info|warn|error>] [-h|--help]\nThe session ID must be nonzero; --session=<n> and --encoder=<value> are also accepted."
}

#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    #[error("pipe closed during handshake")]
    Closed,
    #[error("handshake timed out")]
    Timeout,
    #[error("service speaks pipe protocol {got}")]
    Version { got: u32 },
    #[error("unexpected frame during handshake")]
    Unexpected,
    #[error(transparent)]
    Pipe(#[from] PipeError),
}

/// Send the session hello and wait for the service welcome.
///
/// Discard the connection on timeout: `read_frame` may have consumed part of a
/// frame when the timeout cancels it.
pub async fn handshake<S>(
    stream: &mut S,
    session_id: u32,
    pid: u32,
    timeout: Duration,
) -> Result<(), HandshakeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let result = tokio::time::timeout(timeout, async {
        write_frame(
            stream,
            &PipeFrame::Ctl(Ctl::Hello {
                version: PIPE_PROTO_VERSION,
                session_id,
                pid,
            }),
        )
        .await?;
        stream.flush().await.map_err(PipeError::from)?;
        read_frame(stream).await
    })
    .await;

    let frame = match result {
        Ok(frame) => frame?,
        Err(_) => return Err(HandshakeError::Timeout),
    };
    match frame {
        None => Err(HandshakeError::Closed),
        Some(PipeFrame::Ctl(Ctl::Welcome { version })) if version == PIPE_PROTO_VERSION => Ok(()),
        Some(PipeFrame::Ctl(Ctl::Welcome { version })) => {
            Err(HandshakeError::Version { got: version })
        }
        Some(_) => Err(HandshakeError::Unexpected),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunExit {
    ServiceClosed,
    BadArgs,
    ConnectFailed,
    HandshakeFailed,
    PipeError,
}

impl RunExit {
    /// Process exit code: 0, 2, 3, 4 or 5, respectively.
    pub fn code(self) -> i32 {
        match self {
            Self::ServiceClosed => 0,
            Self::BadArgs => 2,
            Self::ConnectFailed => 3,
            Self::HandshakeFailed => 4,
            Self::PipeError => 5,
        }
    }
}

#[derive(Debug)]
pub struct ServeEnd {
    pub exit: RunExit,
    pub frames_in: u64,
    pub frames_out: u64,
    /// The pipe or handshake error's Display text; `None` on normal closure.
    pub error: Option<String>,
}

enum ReadEvent {
    Frame(PipeFrame),
    Closed,
    Error(String),
}

/// Run the post-handshake session server over an arbitrary async stream.
pub async fn serve<S, P>(
    stream: S,
    platform: Arc<P>,
    mut desktop_rx: UnboundedReceiver<String>,
) -> ServeEnd
where
    S: AsyncRead + AsyncWrite + Send + 'static,
    P: Platform,
{
    let (reader, writer) = split(stream);
    let (read_event_tx, mut read_event_rx) = mpsc::unbounded_channel();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<PipeFrame>();
    let (writer_error_tx, mut writer_error_rx) = tokio::sync::oneshot::channel();
    let frames_out = Arc::new(AtomicU64::new(0));

    // read_frame is not cancel-safe. Only this task reads the stream, and the
    // main loop selects over complete-frame events instead of read futures.
    let reader_task = tokio::spawn(async move {
        let mut reader = reader;
        loop {
            match read_frame(&mut reader).await {
                Ok(Some(frame)) => {
                    if read_event_tx.send(ReadEvent::Frame(frame)).is_err() {
                        break;
                    }
                }
                Ok(None) => {
                    let _ = read_event_tx.send(ReadEvent::Closed);
                    break;
                }
                Err(error) => {
                    let _ = read_event_tx.send(ReadEvent::Error(error.to_string()));
                    break;
                }
            }
        }
    });

    let writer_frames = Arc::clone(&frames_out);
    let writer_task = tokio::spawn(async move {
        let mut writer = writer;
        while let Some(frame) = out_rx.recv().await {
            if let Err(error) = write_frame(&mut writer, &frame).await {
                let _ = writer_error_tx.send(error.to_string());
                return;
            }
            if let Err(error) = writer.flush().await {
                let _ = writer_error_tx.send(PipeError::from(error).to_string());
                return;
            }
            writer_frames.fetch_add(1, Ordering::Relaxed);
        }
    });

    let mut server = SessionServer::new(platform, out_tx.clone());
    server.start();

    let mut desktop_open = true;
    let mut writer_open = true;
    let mut frames_in = 0_u64;
    let (exit, error) = loop {
        tokio::select! {
            event = read_event_rx.recv() => {
                match event {
                    Some(ReadEvent::Frame(frame)) => {
                        frames_in = frames_in.saturating_add(1);
                        server.handle(frame);
                    }
                    Some(ReadEvent::Closed) | None => {
                        break (RunExit::ServiceClosed, None);
                    }
                    Some(ReadEvent::Error(error)) => {
                        break (RunExit::PipeError, Some(error));
                    }
                }
            }
            result = &mut writer_error_rx, if writer_open => {
                match result {
                    Ok(error) => {
                        break (RunExit::PipeError, Some(error));
                    }
                    Err(_) => writer_open = false,
                }
            }
            desktop = desktop_rx.recv(), if desktop_open => {
                match desktop {
                    Some(name) => server.notify_desktop(&name),
                    None => desktop_open = false,
                }
            }
        }
    };

    server.shutdown();
    drop(server);
    drop(out_tx);

    let mut writer_task = writer_task;
    let writer_result = tokio::time::timeout(WRITER_DRAIN, &mut writer_task).await;
    if writer_result.is_err() {
        writer_task.abort();
        let _ = writer_task.await;
    }

    reader_task.abort();
    let _ = reader_task.await;

    let end = ServeEnd {
        exit,
        frames_in,
        frames_out: frames_out.load(Ordering::Relaxed),
        error,
    };
    tracing::info!(
        exit = ?end.exit,
        frames_in = end.frames_in,
        frames_out = end.frames_out,
        error = ?end.error,
        "session pipe closed"
    );
    end
}

/// Perform the handshake and, if it succeeds, run the session server.
pub async fn run_session<S, P>(
    mut stream: S,
    platform: Arc<P>,
    desktop_rx: UnboundedReceiver<String>,
    session_id: u32,
    pid: u32,
    handshake_timeout: Duration,
) -> ServeEnd
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    P: Platform,
{
    if let Err(handshake_error) = handshake(&mut stream, session_id, pid, handshake_timeout).await {
        return ServeEnd {
            exit: RunExit::HandshakeFailed,
            frames_in: 0,
            frames_out: 0,
            error: Some(handshake_error.to_string()),
        };
    }

    serve(stream, platform, desktop_rx).await
}
