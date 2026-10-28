import { toRsCoreError } from '@app/native/rscore/errors';

import type { Transport } from './tauri';

export type DesktopPlatform = 'macos' | 'windows';
export type DesktopQuality = 'auto' | 'clear' | 'smooth' | 'eco';
export type DesktopInfo = {
  version: string;
  platform: DesktopPlatform;
  deviceName: string;
  dataDir: string;
};
export type DesktopSettings = {
  /** 默认 true。 */
  macCmdToCtrl: boolean;
  /** 默认 false。 */
  winSwapCtrlMeta: boolean;
  /** 默认 'ansi'。 */
  macKeyboard: 'ansi' | 'iso' | 'jis';
  defaultQuality: DesktopQuality;
};
export type OpenSessionArgs = {
  hostId: string;
  hostName: string;
  hostOs: 'macos' | 'windows' | 'unknown';
  control: boolean;
  displayId?: number;
  quality?: DesktopQuality;
};
export type DesktopSessionEvent =
  | { type: 'ended'; sessionId: string; hostId: string; byUser: boolean }
  | { type: 'focus'; sessionId: string; focused: boolean };

/** Desktop-only shell commands. Rejections are `RsCoreError`. */
export interface DesktopApi {
  info(): Promise<DesktopInfo>;
  /** 会话 id。 */
  openSession(args: OpenSessionArgs): Promise<string>;
  settings(): Promise<DesktopSettings>;
  setSettings(s: DesktopSettings): Promise<void>;
  onSession(cb: (e: DesktopSessionEvent) => void): Promise<() => void>;
  /** 选配对文件，返回 `file://` URI（Windows 为 `file:///C:/…`，路径做百分号编码）；取消返回 null。 */
  pickPairFile(): Promise<string | null>;
}

export const DESKTOP_SESSION_EVENT = 'desktop://session';
export const PAIR_FILE_TITLE = '选择配对文件';
export const PAIR_FILE_EXTENSIONS = ['rspair'];

/** Absolute path → `file://` URI that Rust's `Url::to_file_path` reads back. */
export function fileUri(path: string): string {
  const windows = /^[A-Za-z]:[\\/]/.test(path) || path.startsWith('\\\\');
  const slashed = windows ? path.replaceAll('\\', '/') : path;
  // encodeURI keeps '#' and '?', which would start a fragment or query.
  const encoded = encodeURI(slashed).replaceAll('#', '%23').replaceAll('?', '%3F');
  if (encoded.startsWith('//')) {
    // UNC: //server/share/… → file://server/share/…
    return `file:${encoded}`;
  }
  return encoded.startsWith('/') ? `file://${encoded}` : `file:///${encoded}`;
}

export function desktopApi(t: Transport): DesktopApi {
  const command = async <T>(cmd: string, args?: Record<string, unknown>) => {
    try {
      return await t.invoke<T>(cmd, args);
    } catch (e) {
      throw toRsCoreError(e);
    }
  };

  return {
    info: () => command<DesktopInfo>('desktop_info'),
    openSession: (req) => command<string>('desktop_open_session', { req }),
    settings: () => command<DesktopSettings>('desktop_settings_get'),
    async setSettings(settings) {
      await command<null>('desktop_settings_set', { settings });
    },
    onSession: (cb) => t.listen<DesktopSessionEvent>(DESKTOP_SESSION_EVENT, cb),
    async pickPairFile() {
      const path = await t.pickFile(PAIR_FILE_TITLE, PAIR_FILE_EXTENSIONS);
      return path === null ? null : fileUri(path);
    },
  };
}
