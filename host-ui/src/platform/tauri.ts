import { invoke } from '@tauri-apps/api/core';

import { detectHostOs } from './os';
import type { Platform, TrayState, WindowLabel } from './types';

export function createTauriPlatform(): Platform {
  return {
    os: detectHostOs(),
    async showWindow(label: WindowLabel, route?: string): Promise<void> {
      await invoke<null>('window_show', { label, route: route ?? null });
    },
    async hideWindow(label: WindowLabel): Promise<void> {
      await invoke<null>('window_hide', { label });
    },
    async setTrayState(state: TrayState): Promise<void> {
      await invoke<null>('tray_set_state', { state });
    },
    async notify(title: string, body: string): Promise<void> {
      await invoke<null>('notify', { title, body });
    },
    async openUrl(url: string): Promise<void> {
      await invoke<null>('open_url', { url });
    },
    async revealPath(path: string): Promise<void> {
      await invoke<null>('reveal_path', { path });
    },
    async copyText(text: string): Promise<void> {
      await navigator.clipboard.writeText(text);
    },
    async quit(): Promise<void> {
      await invoke<null>('quit_app');
    },
  };
}
