import NativeRsBonjour from '../specs/NativeRsBonjour';
import type { BonjourEvent } from '../specs/NativeRsBonjour';

export function bonjourAvailable(): boolean {
  return NativeRsBonjour != null;
}

/** 只在添加电脑页浏览；调用返回的函数停止浏览并移除事件订阅。 */
export function browseScreenSharing(
  onEvent: (event: BonjourEvent) => void,
): () => void {
  const bonjour = NativeRsBonjour;
  if (!bonjour) {
    return () => {};
  }
  const subscription = bonjour.onService(onEvent);
  bonjour.start('_rfb._tcp');
  let stopped = false;
  return () => {
    if (stopped) {
      return;
    }
    stopped = true;
    subscription.remove();
    bonjour.stop();
  };
}
