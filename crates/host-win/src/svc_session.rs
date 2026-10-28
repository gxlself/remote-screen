//! Windows adapters and the dedicated service pipe runtime.

use host_core::config::{ConfigStore, VideoEncoder};

use crate::session_run::args_for;

/// Build the command line used to launch a session process.
///
/// Auto deliberately omits `--encoder`, preserving the session process's
/// `RS_VIDEO_ENCODER` environment override.
pub fn session_args(session_id: u32, encoder: VideoEncoder) -> Vec<String> {
    let encoder = match encoder {
        VideoEncoder::Auto => None,
        encoder => Some(encoder),
    };
    args_for(session_id, encoder)
}

/// Read the encoder preference that applies to the next session process.
pub fn configured_encoder(config: &ConfigStore) -> VideoEncoder {
    config.get().video.encoder
}

#[cfg(windows)]
use std::io;
#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use std::sync::{mpsc as sync_mpsc, Arc};
#[cfg(windows)]
use std::thread::{self, JoinHandle};
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
use tokio::net::windows::named_pipe::NamedPipeServer;
#[cfg(windows)]
use tokio::sync::mpsc;

#[cfg(windows)]
use crate::pipe::PipeServer;
#[cfg(windows)]
use crate::pipe_proto::session_pipe_name;
#[cfg(windows)]
use crate::spawn::{spawn_in_session, ChildProcess};
#[cfg(windows)]
use crate::supervisor::{SessionChangeKind, SupervisorConfig};
#[cfg(windows)]
use crate::svc_session_model::{supervise, AttachTarget, ChildOps, DriverStats, SvcCommand};
#[cfg(windows)]
use crate::winproc_model::SESSION_PIPE_SDDL;

#[cfg(windows)]
pub struct WinChildOps {
    exe: PathBuf,
    config: ConfigStore,
}

#[cfg(windows)]
impl WinChildOps {
    pub fn new(session_exe: PathBuf, config: ConfigStore) -> Self {
        Self {
            exe: session_exe,
            config,
        }
    }
}

#[cfg(windows)]
impl ChildOps for WinChildOps {
    type Conn = NamedPipeServer;
    type Listener = PipeServer;
    type Child = ChildProcess;

    fn console_session(&self) -> Option<u32> {
        crate::wts::active_console_session()
    }

    fn listen(&self, session_id: u32) -> io::Result<Self::Listener> {
        PipeServer::new(&session_pipe_name(session_id), SESSION_PIPE_SDDL)
    }

    async fn accept(listener: &mut Self::Listener) -> io::Result<(Self::Conn, u32)> {
        listener.accept().await
    }

    fn spawn(&self, session_id: u32) -> Result<(u32, Self::Child), String> {
        let args = session_args(session_id, configured_encoder(&self.config));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        spawn_in_session(session_id, &self.exe, &args)
            .map(|child| (child.pid, child))
            .map_err(|error| error.to_string())
    }

    async fn wait(child: Arc<Self::Child>) -> u32 {
        child.wait().await
    }

    fn kill(&self, child: &Self::Child) {
        if child.kill().is_err() {
            tracing::warn!(pid = child.pid, "cannot terminate session process");
        }
    }
}

#[cfg(windows)]
pub struct SessionSupervisorHandle {
    commands: mpsc::UnboundedSender<SvcCommand>,
    thread: Option<JoinHandle<DriverStats>>,
}

#[cfg(windows)]
impl SessionSupervisorHandle {
    pub fn session_change(&self, kind: SessionChangeKind, session_id: u32) {
        let _ = self
            .commands
            .send(SvcCommand::SessionChange { kind, session_id });
    }

    /// Send Shutdown and join; a panicked thread returns None.
    pub fn shutdown(mut self) -> Option<DriverStats> {
        let _ = self.commands.send(SvcCommand::Shutdown);
        self.thread.take()?.join().ok()
    }
}

/// Run the pipe pump separately from synchronous `PipePlatform` callers.
///
/// Updating the configuration does not restart a running session process; the
/// new encoder preference is read when the supervisor launches the next one.
#[cfg(windows)]
pub fn start_supervisor<T: AttachTarget>(
    target: Arc<T>,
    session_exe: PathBuf,
    cfg: SupervisorConfig,
    config: ConfigStore,
) -> io::Result<SessionSupervisorHandle> {
    if !session_exe.exists() {
        tracing::warn!("session executable missing; supervision will retry spawning");
    }
    let (commands, rx) = mpsc::unbounded_channel();
    let (ready, started) = sync_mpsc::sync_channel(1);
    let thread = thread::Builder::new()
        .name("rs-svc-session".to_owned())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = ready.send(Err(error));
                    return DriverStats::default();
                }
            };
            let _ = ready.send(Ok(()));
            let stats = runtime.block_on(supervise(
                Arc::new(WinChildOps::new(session_exe, config)),
                target,
                cfg,
                rx,
            ));
            // ChildProcess::wait uses an uninterruptible spawn_blocking OS wait.
            // A child ignoring termination must not make Runtime::drop block join.
            runtime.shutdown_timeout(Duration::ZERO);
            stats
        })?;
    match started.recv() {
        Ok(Ok(())) => Ok(SessionSupervisorHandle {
            commands,
            thread: Some(thread),
        }),
        result => {
            let _ = thread.join();
            Err(match result {
                Ok(Err(error)) => error,
                _ => io::Error::other("session supervisor failed to initialize"),
            })
        }
    }
}
