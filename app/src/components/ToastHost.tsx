import { useSyncExternalStore } from 'react';
import { Toast, toastDuration, type ToastState } from './Toast';

// One toast for the whole App, mounted once at the root, so a message can
// outlive the screen that raised it (返回上一页后的「已保存」).

export type ShowToastOptions = ToastState & {
  durationMs?: number;
};

let current: ToastState | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;
const listeners = new Set<() => void>();

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function snapshot(): ToastState | null {
  return current;
}

function publish(next: ToastState | null): void {
  current = next;
  for (const listener of listeners) {
    listener();
  }
}

function clearTimer(): void {
  if (timer != null) {
    clearTimeout(timer);
    timer = null;
  }
}

export function showToast({ durationMs, ...toast }: ShowToastOptions): void {
  clearTimer();
  // A fresh object, so the same message shown twice is announced twice.
  const next: ToastState = { ...toast };
  publish(next);
  timer = setTimeout(() => {
    timer = null;
    publish(null);
  }, toastDuration(next, durationMs));
}

export function hideToast(): void {
  clearTimer();
  if (current != null) {
    publish(null);
  }
}

export function ToastHost() {
  const toast = useSyncExternalStore(subscribe, snapshot, snapshot);
  return <Toast onHide={hideToast} testID="global-toast" toast={toast} />;
}

export default ToastHost;
