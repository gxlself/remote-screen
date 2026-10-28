//! 录制告知 Host（RecordingNotice，方案 A）。
//!
//! 三个时机调用 [`sync`]：录制开始（true）、录制停止（false）、录制中每收到一次
//! StreamConfig（true，新连接或令牌恢复后重发）。不去重，Host 对重复的 true 不做处理。
//! 只往会话的命令通道投一条消息，不阻塞会话泵；Host 没有宣告 `RECORDING_NOTICE`
//! 时 rsp-client 不发送。会话还没有 handle 时什么也不做。
use crate::rscore::tables::SessionEntry;

pub fn sync(entry: &SessionEntry, recording: bool) {
    if let Some(handle) = entry.handle() {
        handle.set_recording(recording);
    }
}
