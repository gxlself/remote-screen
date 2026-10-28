// The only module that imports '@tauri-apps/*' (importGuard.test.ts checks this).
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';

export interface Transport {
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(event: string, cb: (payload: T) => void): Promise<() => void>;
  /** 选一个本地文件，返回绝对路径；取消返回 null。Tauri 实现用 @tauri-apps/plugin-dialog 的 open()。 */
  pickFile(title: string, extensions: string[]): Promise<string | null>;
}

export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export function tauriTransport(): Transport {
  return {
    invoke<T>(cmd: string, args?: Record<string, unknown>) {
      return invoke<T>(cmd, args);
    },
    listen<T>(event: string, cb: (payload: T) => void) {
      return listen<T>(event, (e) => cb(e.payload));
    },
    async pickFile(title, extensions) {
      const path = await open({
        title,
        multiple: false,
        directory: false,
        filters: [{ name: title, extensions }],
      });
      return typeof path === 'string' ? path : null;
    },
  };
}
