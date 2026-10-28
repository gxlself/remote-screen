//! 声音数据报的接收端接口。解码和播放由调用方实现（rs-mobile、测试）。

/// 收到并解析好的 0x11 声音包。在会话的接收任务里同步调用：实现方不能阻塞超过 1 ms。
pub trait AudioSink: Send + Sync {
    /// seq / capture_ts_us 原样来自数据报；payload 是一个 Opus 包；recv_us 是本机收包时刻（unix 微秒）。
    fn push(&self, seq: u16, capture_ts_us: u64, payload: &[u8], recv_us: u64);
    /// 新连接开始，seq 空间重新计。实现方应清空缓冲。
    fn reset(&self);
}
