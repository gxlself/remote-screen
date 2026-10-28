//! File control messages and request/reply routing (03 §8.2, 07).
//!
//! For a FILES session, advertise [`FILES_FEATURES`] and require
//! [`features::FILE_BROWSE`] in `SessionParams::required_features`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use rsp_core::{features, pb};
use tokio::sync::oneshot;

use crate::session::SessionEvent;

pub const FILES_FEATURES: u64 =
    features::FILE_TRANSFER | features::FILE_BROWSE | features::TCP_TRANSPORT;

// 03 §8.2: stream-not-allowed wire code for rejecting file-stream opens.
pub(crate) const STREAM_NOT_ALLOWED: u32 = 6;

/// Outbound file control messages. Deliberately omits Debug: payloads contain
/// paths and file names which must not be logged.
pub enum FileCtl {
    List(pb::FileList),
    Request(pb::FileRequest),
    Query(pb::FileQuery),
    Status(pb::FileStatus),
    Cancel(pb::FileCancel),
}

impl From<FileCtl> for pb::Envelope {
    fn from(value: FileCtl) -> Self {
        match value {
            FileCtl::List(message) => message.into(),
            FileCtl::Request(message) => message.into(),
            FileCtl::Query(message) => message.into(),
            FileCtl::Status(message) => message.into(),
            FileCtl::Cancel(message) => message.into(),
        }
    }
}

pub enum FileReply {
    Listing(pb::FileListing),
    Status(pb::FileStatus),
}

#[derive(Default)]
struct Pending {
    last_id: u32,
    replies: HashMap<u32, oneshot::Sender<FileReply>>,
}

/// Shared correlation helper. Register `wait(id)` before sending a request,
/// feed received session events to `route`, and call `fail_all` on session end.
#[derive(Clone, Default)]
pub struct FileReplies {
    pending: Arc<Mutex<Pending>>,
}

impl FileReplies {
    fn lock(&self) -> MutexGuard<'_, Pending> {
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// Allocate a nonzero request id, skipping pending ids on wraparound.
    pub fn next_id(&self) -> u32 {
        let mut pending = self.lock();
        loop {
            pending.last_id = pending.last_id.wrapping_add(1).max(1);
            if !pending.replies.contains_key(&pending.last_id) {
                return pending.last_id;
            }
        }
    }

    /// Register one waiter per id. Replacing a waiter cancels the old receiver.
    pub fn wait(&self, id: u32) -> oneshot::Receiver<FileReply> {
        let (send, recv) = oneshot::channel();
        self.lock().replies.insert(id, send);
        recv
    }

    /// Deliver a pending reply and return true. Unsolicited status notifications
    /// (`request_id == 0`) and unrelated events remain available to the caller.
    pub fn route(&self, ev: &SessionEvent) -> bool {
        let id = match ev {
            SessionEvent::FileListing(message) => message.request_id,
            SessionEvent::FileStatus(message) if message.request_id != 0 => message.request_id,
            _ => return false,
        };
        let Some(send) = self.lock().replies.remove(&id) else {
            return false;
        };
        let reply = match ev {
            SessionEvent::FileListing(message) => FileReply::Listing(message.clone()),
            SessionEvent::FileStatus(message) => FileReply::Status(message.clone()),
            _ => return false,
        };
        send.send(reply).is_ok()
    }

    /// Drop all senders so pending receivers immediately receive `RecvError`.
    pub fn fail_all(&self) {
        self.lock().replies.clear();
    }
}
