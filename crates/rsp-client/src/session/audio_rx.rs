//! Decode audio datagrams before handing them to the caller's sink.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use crate::audio::AudioSink;

pub(super) struct AudioRx {
    sink: Option<Arc<dyn AudioSink>>,
    bad: u64,
    last_log: Option<Instant>,
}

impl AudioRx {
    pub fn new(sink: Option<Arc<dyn AudioSink>>) -> Self {
        Self {
            sink,
            bad: 0,
            last_log: None,
        }
    }

    pub fn on_datagram(&mut self, wire: &[u8], recv_us: u64) {
        let Some(sink) = &self.sink else {
            return;
        };
        match rsp_core::audio::decode(wire) {
            Ok((header, payload)) => {
                sink.push(header.seq, header.capture_ts_us, payload, recv_us);
            }
            Err(error) => {
                self.bad = self.bad.saturating_add(1);
                let now = Instant::now();
                if self
                    .last_log
                    .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(10))
                {
                    tracing::debug!(%error, bad = self.bad, "invalid audio datagram");
                    self.last_log = Some(now);
                }
            }
        }
    }

    pub fn reset(&self) {
        if let Some(sink) = &self.sink {
            sink.reset();
        }
    }
}
