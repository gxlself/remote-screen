export type WindowLabel =
  'bg' | 'menu' | 'capsule' | 'border' | 'confirm' | 'pair' | 'settings' | 'setup' | 'update';

export type HostOs = 'macos' | 'windows';

export type TrayState = 'idle' | 'controlled' | 'update' | 'offline';

export const WINDOW_LABELS: readonly WindowLabel[] = [
  'bg',
  'menu',
  'capsule',
  'border',
  'confirm',
  'pair',
  'settings',
  'setup',
  'update',
] as const;

export interface Platform {
  readonly os: HostOs;
  showWindow(label: WindowLabel, route?: string): Promise<void>;
  hideWindow(label: WindowLabel): Promise<void>;
  setTrayState(state: TrayState): Promise<void>;
  notify(title: string, body: string): Promise<void>;
  openUrl(url: string): Promise<void>;
  revealPath(path: string): Promise<void>;
  copyText(text: string): Promise<void>;
  quit(): Promise<void>;
}

export const SYSTEM_SETTINGS_URLS = {
  screenRecording: 'x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture',
  accessibility: 'x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility',
} as const;

export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}
