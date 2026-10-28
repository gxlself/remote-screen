import type { SessionId } from '../rscore/types';
import NativeRsAudio from '../specs/NativeRsAudio';

/** 开始播放某会话的远端声音。没有原生模块（jest、Android）时返回 false。 */
export async function startAudio(sessionId: SessionId): Promise<boolean> {
  try {
    return (await NativeRsAudio?.start(sessionId)) ?? false;
  } catch {
    return false;
  }
}
/** 停止播放并释放音频会话。 */
export function stopAudio(): void {
  NativeRsAudio?.stop();
}
/** 本地静音（5 ms 淡出 / 淡入），不影响 Host 发送。 */
export function setAudioMuted(muted: boolean): void {
  NativeRsAudio?.setMuted(muted);
}
