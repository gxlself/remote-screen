//! Transport-independent one-line JSON-RPC stream server.
//!
//! [`IpcListener`] abstracts how connections are accepted. A Unix socket or a
//! Windows named pipe can provide a listener while sharing the same backend
//! and event broadcast.

use std::future::Future;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::broadcast;

use crate::ipc::{handle_client, Event, IpcBackend, IpcError, Notifier};

const NOTIFICATION_CAPACITY: usize = 256;

/// Source of established local IPC connections.
pub trait IpcListener: Send + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    fn accept(&mut self) -> impl Future<Output = std::io::Result<Self::Stream>> + Send;
}

/// Serves JSON-RPC over any stream listener supplied by the platform layer.
pub struct StreamServer<B: IpcBackend> {
    backend: Arc<B>,
    notifier: broadcast::Sender<Event>,
}

impl<B: IpcBackend> StreamServer<B> {
    pub fn new(backend: B) -> Self {
        let (notifier, _) = broadcast::channel(NOTIFICATION_CAPACITY);
        Self {
            backend: Arc::new(backend),
            notifier,
        }
    }

    pub fn notifier(&self) -> Notifier {
        Notifier::from_sender(self.notifier.clone())
    }

    /// Starts an independent task for an already established connection.
    pub fn spawn_connection<S>(&self, stream: S) -> tokio::task::JoinHandle<()>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let backend = Arc::clone(&self.backend);
        let events = self.notifier.subscribe();
        tokio::spawn(async move {
            if let Err(error) = handle_client(stream, backend, events).await {
                tracing::debug!(error = %error, "IPC client disconnected");
            }
        })
    }

    /// Accepts connections until the listener reports an I/O error.
    pub async fn serve<L: IpcListener>(&self, mut listener: L) -> Result<(), IpcError> {
        loop {
            let stream = listener
                .accept()
                .await
                .map_err(|error| IpcError::new("accept_failed", error.to_string()))?;
            self.spawn_connection(stream);
        }
    }
}
