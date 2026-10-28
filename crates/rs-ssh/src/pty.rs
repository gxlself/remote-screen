//! PTY requests and bounded terminal byte streams.

use std::{collections::VecDeque, sync::Arc};

use russh::{client, ChannelMsg, ChannelReadHalf, ChannelWriteHalf};
use tokio::sync::{mpsc, watch, Mutex};

use crate::{ClientChannel, Connection, Shared, SshError, SshEvent};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PtyOpts {
    pub term: String,
    pub cols: u32,
    pub rows: u32,
    pub tmux: Option<String>,
}

impl Default for PtyOpts {
    fn default() -> Self {
        Self {
            term: "xterm-256color".into(),
            cols: 80,
            rows: 24,
            tmux: None,
        }
    }
}

type Writer = Arc<Mutex<ChannelWriteHalf<client::Msg>>>;
type Completion = Result<Option<u32>, SshError>;

/// Consume `output()` while the shell runs, including while awaiting `closed()`.
/// Both stdout and terminal stderr are delivered as uninterpreted byte chunks.
pub struct Shell {
    writer: Writer,
    output: mpsc::Receiver<Vec<u8>>,
    completion: watch::Receiver<Option<Completion>>,
    task: tokio::task::JoinHandle<()>,
    shared: Arc<Shared>,
}

impl Shell {
    fn check_writable(&self) -> Result<(), SshError> {
        self.shared.check()?;
        match &*self.completion.borrow() {
            Some(Err(error)) => Err(error.clone()),
            Some(Ok(_)) => Err(SshError::Closed),
            None => Ok(()),
        }
    }

    pub async fn write(&self, bytes: &[u8]) -> Result<(), SshError> {
        self.check_writable()?;
        self.writer.lock().await.data(bytes).await?;
        Ok(())
    }

    pub async fn resize(&self, cols: u32, rows: u32) -> Result<(), SshError> {
        if cols == 0 || rows == 0 {
            return Err(SshError::InvalidOptions("PTY dimensions must be positive"));
        }
        self.check_writable()?;
        self.writer
            .lock()
            .await
            .window_change(cols, rows, 0, 0)
            .await?;
        Ok(())
    }

    pub fn output(&mut self) -> &mut mpsc::Receiver<Vec<u8>> {
        &mut self.output
    }

    /// Remote exit status, or `None` if no status was sent or the connection failed.
    /// Use `result()` to distinguish a transport failure from missing exit status.
    pub async fn closed(&mut self) -> Option<u32> {
        self.result().await.ok().flatten()
    }

    pub async fn result(&mut self) -> Completion {
        loop {
            if let Some(result) = self.completion.borrow_and_update().clone() {
                return result;
            }
            self.completion
                .changed()
                .await
                .map_err(|_| SshError::Closed)?;
        }
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        self.task.abort();
        // Closing the receiver also releases any pending output backpressure.
        self.output.close();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let writer = self.writer.clone();
            runtime.spawn(async move {
                let _ = tokio::time::timeout(std::time::Duration::from_secs(1), async {
                    let _ = writer.lock().await.close().await;
                })
                .await;
            });
        }
    }
}

pub(crate) async fn open(connection: Arc<Connection>, opts: PtyOpts) -> Result<Shell, SshError> {
    if opts.cols == 0 || opts.rows == 0 || opts.term.is_empty() || opts.term.contains('\0') {
        return Err(SshError::InvalidOptions(
            "PTY term and positive dimensions are required",
        ));
    }
    let command = opts.tmux.as_deref().map(crate::tmux::command).transpose()?;
    connection.shared.check()?;
    let (channel, pending) = prepare(&connection, &opts, command.as_deref()).await?;
    let (reader, writer) = channel.split();
    let writer = Arc::new(Mutex::new(writer));
    let (output_tx, output) = mpsc::channel(connection.options.output_buffer);
    let (completion_tx, completion) = watch::channel(None);
    let shared = connection.shared.clone();
    let task_writer = writer.clone();
    let task = tokio::spawn(async move {
        let result = pump(
            connection,
            opts,
            reader,
            task_writer.clone(),
            pending,
            output_tx,
        )
        .await;
        let _ = task_writer.lock().await.close().await;
        completion_tx.send_replace(Some(result));
    });
    Ok(Shell {
        writer,
        output,
        completion,
        task,
        shared,
    })
}

async fn prepare(
    connection: &Connection,
    opts: &PtyOpts,
    command: Option<&str>,
) -> Result<(ClientChannel, VecDeque<ChannelMsg>), SshError> {
    connection.shared.check()?;
    let mut channel = tokio::time::timeout(
        connection.options.request_timeout,
        connection.handle.channel_open_session(),
    )
    .await
    .map_err(|_| SshError::Timeout)??;
    let mut pending = VecDeque::new();
    let result = tokio::time::timeout(connection.options.request_timeout, async {
        channel
            .request_pty(true, &opts.term, opts.cols, opts.rows, 0, 0, &[])
            .await?;
        request_success(&mut channel, &mut pending).await?;
        if let Some(command) = command {
            channel.exec(true, command.as_bytes()).await?;
        } else {
            channel.request_shell(true).await?;
        }
        request_success(&mut channel, &mut pending).await
    })
    .await
    .map_err(|_| SshError::Timeout)
    .and_then(|r| r);
    if let Err(error) = result {
        let _ = channel.close().await;
        return Err(error);
    }
    Ok((channel, pending))
}

pub(crate) async fn request_success(
    channel: &mut ClientChannel,
    pending: &mut VecDeque<ChannelMsg>,
) -> Result<(), SshError> {
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Success) => return Ok(()),
            Some(ChannelMsg::Failure) => return Err(SshError::RequestRejected),
            Some(ChannelMsg::Close) | None => return Err(SshError::Closed),
            Some(ChannelMsg::WindowAdjusted { .. }) => {}
            Some(msg) => pending.push_back(msg),
        }
    }
}

async fn pump(
    connection: Arc<Connection>,
    opts: PtyOpts,
    mut reader: ChannelReadHalf,
    writer: Writer,
    mut pending: VecDeque<ChannelMsg>,
    output: mpsc::Sender<Vec<u8>>,
) -> Completion {
    let mut state = connection.shared.state.subscribe();
    let mut exit_status = None;
    let mut tmux = opts.tmux.is_some();
    loop {
        connection.shared.check()?;
        let msg = if let Some(msg) = pending.pop_front() {
            Some(msg)
        } else {
            tokio::select! {
                msg = reader.wait() => msg,
                _ = output.closed() => return Ok(exit_status),
                _ = state.changed() => { connection.shared.check()?; continue; }
            }
        };
        match msg {
            Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                // Never try_send or spawn unbounded work: the consumer owns pacing.
                if output.send(data.to_vec()).await.is_err() {
                    return Ok(exit_status);
                }
            }
            Some(ChannelMsg::ExitStatus {
                exit_status: status,
            }) => exit_status = Some(status),
            Some(ChannelMsg::Close) | None => {
                if tmux && exit_status == Some(127) {
                    let _ = connection.shared.events.send(SshEvent::TmuxUnavailable);
                    // An SSH session channel cannot start a second process. Reopen
                    // the PTY and shell on a fresh channel on the same transport.
                    let mut current_writer = writer.lock().await;
                    let _ = current_writer.close().await;
                    let (channel, initial) = prepare(&connection, &opts, None).await?;
                    let (new_reader, new_writer) = channel.split();
                    reader = new_reader;
                    *current_writer = new_writer;
                    pending = initial;
                    exit_status = None;
                    tmux = false;
                } else {
                    connection.shared.check()?;
                    return Ok(exit_status);
                }
            }
            // EOF only closes the byte stream. Exit-status commonly follows EOF.
            _ => {}
        }
    }
}
