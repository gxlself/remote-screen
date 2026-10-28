//! 会话录制挂点（M3 V5）。会话泵在每个事件上调用；没在录时每个方法只做一次原子读就返回，
//! 绝不阻塞会话泵。状态类事件（StreamConfig、光标）总是先记进缓存：录制可能在会话中途开始，
//! 开始时用缓存补一段前导。
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, MutexGuard,
};

use rsp_client::{audio::AudioSink, CursorEvent, ReceivedFrame};
use rsp_core::pb;

use super::recorder::{Control, Ingress, REASON_SESSION_ENDED};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

/// 最近一次的状态类事件，取舍同 VideoTap 的缓存。
#[derive(Default)]
struct SlotCache {
    config: Option<pb::StreamConfig>,
    cursor: Option<CursorEvent>,
    shape: Option<pb::CursorMsg>,
    cursor_state: Option<pb::CursorMsg>,
}

#[derive(Default)]
struct SlotShared {
    active: AtomicBool,
    /// 一个会话同一时间只有一份录制：从开始录制到收尾完成都占着。
    claimed: AtomicBool,
    ingress: Mutex<Option<Ingress>>,
    cache: Mutex<SlotCache>,
    control: Mutex<Option<Arc<Control>>>,
    #[cfg(test)]
    ingress_locks: std::sync::atomic::AtomicUsize,
}

impl SlotShared {
    fn ingress(&self) -> MutexGuard<'_, Option<Ingress>> {
        #[cfg(test)]
        self.ingress_locks.fetch_add(1, Ordering::Relaxed);
        lock(&self.ingress)
    }

    fn record(&self, f: impl FnOnce(&mut Ingress)) {
        if !self.active.load(Ordering::Acquire) {
            return;
        }
        if let Some(ingress) = self.ingress().as_mut() {
            f(ingress);
        }
    }
}

#[derive(Default)]
pub struct RecorderSlot {
    shared: Arc<SlotShared>,
}

impl RecorderSlot {
    pub fn is_recording(&self) -> bool {
        self.shared.active.load(Ordering::Acquire)
    }

    pub fn frame(&self, frame: &ReceivedFrame) {
        self.shared.record(|ingress| ingress.frame(frame));
    }

    pub fn stream_config(&self, config: &pb::StreamConfig) {
        lock(&self.shared.cache).config = Some(*config);
        if !self.shared.active.load(Ordering::Acquire) {
            return;
        }
        if let Some(ingress) = self.shared.ingress().as_mut() {
            ingress.config(config);
        }
        // C5：新连接或令牌恢复后会重发 StreamConfig，借此重发录制告知。
        if let Some(control) = self.control() {
            control.notice(true);
        }
    }

    pub fn cursor(&self, cursor: &CursorEvent) {
        lock(&self.shared.cache).cursor = Some(cursor.clone());
        self.shared.record(|ingress| ingress.cursor(cursor));
    }

    pub fn cursor_msg(&self, message: &pb::CursorMsg) {
        {
            let mut cache = lock(&self.shared.cache);
            match &message.msg {
                Some(pb::cursor_msg::Msg::Shape(_)) => {
                    cache.shape = Some(message.clone());
                    cache.cursor_state = None;
                }
                Some(pb::cursor_msg::Msg::Use(_) | pb::cursor_msg::Msg::Hidden(_)) => {
                    cache.cursor_state = Some(message.clone());
                }
                None => {}
            }
        }
        self.shared.record(|ingress| ingress.cursor_msg(message));
    }

    /// 会话进后台暂停（true）/ 回前台（false）。
    pub fn paused(&self, paused: bool) {
        if paused {
            self.shared.record(Ingress::mark_paused);
        }
    }

    /// 会话结束（finish_open）：收尾并关闭文件。
    pub fn closed(&self, _reason: &str) {
        if !self.shared.active.load(Ordering::Acquire) {
            return;
        }
        if let Some(control) = self.control() {
            control.request_stop(REASON_SESSION_ENDED);
        }
    }

    /// 包一层声音接收端：转交给播放的同时录下声音包。会话打开时就包上，录制开始前只多一次原子读。
    pub fn audio_tee(&self, inner: Arc<dyn AudioSink>) -> Arc<dyn AudioSink> {
        Arc::new(TeeSink {
            inner,
            shared: self.shared.clone(),
        })
    }

    /// 占住录制位；已经有一份在录（含正在开始、正在收尾）返回 false。
    pub(crate) fn claim(&self) -> bool {
        self.shared
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// 装上 Ingress 并开始录制，同时把缓存的状态作为前导送进通道。
    pub(crate) fn install(&self, ingress: Ingress, control: Arc<Control>) {
        let mut slot = self.shared.ingress();
        *lock(&self.shared.control) = Some(control);
        let ingress = slot.insert(ingress);
        // 先置 active 再读缓存：读完缓存之后才到的状态事件会排在前导后面，不会丢。
        self.shared.active.store(true, Ordering::Release);
        let cache = lock(&self.shared.cache);
        if let Some(config) = &cache.config {
            ingress.config(config);
        }
        if let Some(cursor) = &cache.cursor {
            ingress.cursor(cursor);
        }
        for message in [&cache.shape, &cache.cursor_state].into_iter().flatten() {
            ingress.cursor_msg(message);
        }
    }

    /// 停止录制并取回 Ingress（写盘线程补写它暂存的状态记录）。返回之后不会再有新消息进通道。
    pub(crate) fn uninstall(&self) -> Option<Ingress> {
        let mut slot = self.shared.ingress();
        self.shared.active.store(false, Ordering::Release);
        slot.take()
    }

    /// 放开录制位：开始失败，或收尾完成之后。
    pub(crate) fn release(&self) {
        *lock(&self.shared.control) = None;
        self.shared.claimed.store(false, Ordering::Release);
    }

    pub(crate) fn control(&self) -> Option<Arc<Control>> {
        lock(&self.shared.control).clone()
    }

    /// 同一个挂点的另一个句柄，给写盘线程收尾用：会话先释放了也能卸下 Ingress。
    pub(crate) fn share(&self) -> RecorderSlot {
        RecorderSlot {
            shared: self.shared.clone(),
        }
    }
}

struct TeeSink {
    inner: Arc<dyn AudioSink>,
    shared: Arc<SlotShared>,
}

impl AudioSink for TeeSink {
    fn push(&self, seq: u16, capture_ts_us: u64, payload: &[u8], recv_us: u64) {
        self.inner.push(seq, capture_ts_us, payload, recv_us);
        self.shared
            .record(|ingress| ingress.audio(seq, capture_ts_us, payload));
    }

    fn reset(&self) {
        self.inner.reset();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicU64, AtomicUsize},
        mpsc, Weak,
    };

    use rs_recording::record::Body;
    use rsp_core::video::Frame;

    use super::super::dto::RecordingStatusState;
    use super::*;

    #[derive(Default)]
    struct CountingSink {
        pushes: AtomicUsize,
        resets: AtomicUsize,
    }

    impl AudioSink for CountingSink {
        fn push(&self, _seq: u16, _capture_ts_us: u64, _payload: &[u8], _recv_us: u64) {
            self.pushes.fetch_add(1, Ordering::Relaxed);
        }

        fn reset(&self) {
            self.resets.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn frame(keyframe: bool) -> ReceivedFrame {
        ReceivedFrame {
            frame: Frame {
                frame_id: 1,
                epoch: 1,
                display_id: 1,
                flags: 0,
                keyframe,
                capture_ts_us: 5,
                data: vec![0; 64],
            },
            received_us: 1_000_000,
            capture_local_us: None,
        }
    }

    fn shape(id: u64) -> pb::CursorMsg {
        pb::CursorMsg {
            msg: Some(pb::cursor_msg::Msg::Shape(pb::CursorShape {
                id,
                width: 16,
                height: 16,
                hot_x: 1,
                hot_y: 2,
                scale: 2.0,
                png: vec![7; 32],
            })),
        }
    }

    #[test]
    fn idle_slot_never_locks_ingress_and_tee_always_forwards() {
        let slot = RecorderSlot::default();
        let inner = Arc::new(CountingSink::default());
        let tee = slot.audio_tee(inner.clone());
        let frame = frame(false);
        let config = pb::StreamConfig::default();
        let cursor = CursorEvent {
            x: 0.5,
            y: 0.5,
            visible: true,
        };
        for _ in 0..1000 {
            slot.frame(&frame);
            slot.paused(true);
            slot.paused(false);
            slot.closed("cancelled");
            tee.push(1, 2, &[1, 2, 3], 4);
            slot.stream_config(&config);
            slot.cursor(&cursor);
            slot.cursor_msg(&shape(9));
        }
        tee.reset();
        assert_eq!(slot.shared.ingress_locks.load(Ordering::Relaxed), 0);
        assert_eq!(inner.pushes.load(Ordering::Relaxed), 1000);
        assert_eq!(inner.resets.load(Ordering::Relaxed), 1);
        assert!(!slot.is_recording());
        assert!(slot.control().is_none());
    }

    #[test]
    fn install_replays_cached_state_and_uninstall_stops_recording() {
        let slot = RecorderSlot::default();
        let config = pb::StreamConfig {
            epoch: 3,
            width: 1280,
            height: 720,
            ..Default::default()
        };
        slot.stream_config(&config);
        slot.cursor(&CursorEvent {
            x: 0.25,
            y: 0.75,
            visible: true,
        });
        slot.cursor_msg(&pb::CursorMsg {
            msg: Some(pb::cursor_msg::Msg::Hidden(true)),
        });
        slot.cursor_msg(&shape(9));
        slot.cursor_msg(&pb::CursorMsg {
            msg: Some(pb::cursor_msg::Msg::Use(9)),
        });

        let (tx, rx) = mpsc::sync_channel(16);
        let now = Arc::new(AtomicU64::new(1_000_000));
        let clock = now.clone();
        let ingress = Ingress::new(
            tx,
            Arc::default(),
            usize::MAX,
            Box::new(|| {}),
            Box::new(move || clock.load(Ordering::Relaxed)),
        );
        assert!(slot.claim());
        assert!(!slot.claim());
        let control = Arc::new(Control::new("ab".repeat(16), 1, Weak::new(), 0));
        slot.install(ingress, control);
        assert!(slot.is_recording());
        let prelude: Vec<_> = rx.try_iter().map(|record| record.body).collect();
        assert!(matches!(&prelude[0], Body::Config(c) if c.epoch == 3 && c.width == 1280));
        assert_eq!(
            prelude[1],
            Body::CursorPos {
                x: 0.25,
                y: 0.75,
                visible: true
            }
        );
        assert!(matches!(&prelude[2], Body::CursorShape(s) if s.id == 9 && s.png == [7; 32]));
        assert_eq!(prelude[3], Body::CursorUse(9));
        assert_eq!(prelude.len(), 4);

        slot.frame(&frame(true));
        assert!(matches!(rx.try_recv().unwrap().body, Body::Video(v) if v.keyframe));
        slot.closed("cancelled");
        assert_eq!(
            slot.control().unwrap().status().state,
            RecordingStatusState::Stopping
        );
        assert!(slot.uninstall().is_some());
        assert!(!slot.is_recording());
        slot.frame(&frame(true));
        assert!(rx.try_recv().is_err());
        slot.release();
        assert!(slot.control().is_none());
        assert!(slot.claim());
    }
}
