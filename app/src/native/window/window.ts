import { Platform } from 'react-native';
import NativeRsWindow from '../specs/NativeRsWindow';
import type { WindowStateEvent } from '../specs/NativeRsWindow';

export type { WindowStateEvent } from '../specs/NativeRsWindow';

/** every computer can have a system window of its own (iPad). */
export function windowsSupported(): boolean {
  if (Platform.OS !== 'ios') {
    return false;
  }
  return NativeRsWindow?.getConstants().supported ?? false;
}

export function openWindow(
  windowId: string,
  payload: string,
  title: string,
): void {
  NativeRsWindow?.open(windowId, payload, title);
}

export function activateWindow(windowId: string): void {
  NativeRsWindow?.activate(windowId);
}

export function closeWindow(windowId: string): void {
  NativeRsWindow?.close(windowId);
}

export function refreshWindows(): void {
  NativeRsWindow?.refresh();
}

export function subscribeWindowState(
  listener: (event: WindowStateEvent) => void,
): () => void {
  const module = NativeRsWindow;
  if (module == null) {
    return () => undefined;
  }
  const subscription = module.onWindowState(listener);
  return () => subscription.remove();
}
