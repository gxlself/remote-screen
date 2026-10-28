import type { Quality } from '../../native/rscore/types';
import type { ConnectVia } from '../native/types';

export type HostPrefs = {
  connectVia: ConnectVia;
  routePolicy: 'auto' | 'tailnet' | 'intranet';
  tcpFallback: boolean;
  autoUpdateAddr: boolean;
  lockOnEnd: boolean;
};

export type NetworkEntry = {
  id: string;
  name: string;
  cidrs: string[];
  vpnPools: string[];
};

export type SessionMode = 'trackpad' | 'touch';
export type TailnetMode = 'system' | 'embedded' | 'auto';
export type HostPrefsMap = Record<string, Partial<HostPrefs>>;
export type RecordingCapMb = 1024 | 2048 | 4096 | 8192;
export type RecordingRetentionDays = 7 | 30 | 90 | null;
/** How long a folded-away session keeps its connection. */
export type CollapsedKeep = '10m' | '30m' | '2h' | 'always';

export const settingsSchema = {
  'quality.default': { def: 'auto' as Quality, sync: true, doc: '10' },
  'session.mode.phone': {
    def: 'trackpad' as SessionMode,
    doc: '10',
  },
  'session.mode.tablet': { def: 'touch' as SessionMode, doc: '10' },
  'session.hud': { def: false as boolean, doc: '10' },
  'session.coachShown': { def: false as boolean, doc: '10' },
  /**
   * Tailscale is an opt-in extra (outside access). Off, the app is LAN and
   * company-VPN only: no Tailscale UI, no embedded node, no Tailscale API.
   */
  'tailnet.enabled': { def: false as boolean, sync: true, doc: '10' },
  'tailnet.mode': { def: 'system' as TailnetMode, sync: true, doc: '10' },
  'tailnet.controlUrl': { def: null as string | null, sync: true, doc: '10' },
  'prewarm.enabled': { def: true as boolean, sync: true, doc: '10' },
  'lock.onOpen': { def: true as boolean, doc: '10' },
  'lock.beforeSession': { def: false as boolean, doc: '10' },
  'lock.sensitiveHosts': { def: [] as string[], doc: '10' },
  networks: { def: [] as NetworkEntry[], sync: true, doc: '10' },
  'onboarding.done': { def: false as boolean, doc: 'canvas' },
  'notify.enabled': { def: false as boolean, doc: 'canvas' },
  'session.cursorSpeed': { def: 1.4, doc: 'canvas' },
  'session.naturalScroll': { def: true as boolean, doc: 'canvas' },
  'session.haptics': { def: true as boolean, doc: 'canvas' },
  'session.sound': { def: false as boolean, doc: 'canvas' },
  'session.collapsedKeep': { def: '2h' as CollapsedKeep, doc: 'canvas' },
  'net.preferLocal': { def: true as boolean, doc: 'canvas' },
  'net.warnVpnConflict': { def: true as boolean, doc: 'canvas' },
  'terminal.fontSize': { def: 13, doc: 'canvas' },
  'terminal.tmux': { def: true as boolean, doc: 'canvas' },
  'hosts.prefs': { def: {} as HostPrefsMap, doc: 'canvas' },
  'connect.prefer': { def: 'native' as 'native' | 'host', doc: 'canvas' },
  'clipboard.autoSync': { def: true as boolean, doc: 'canvas' },
  'keyboard.cmdAsCtrl': { def: true as boolean, doc: 'canvas' },
  /** Cmd+V on a hardware keyboard first sends this device's clipboard. */
  'keyboard.pasteSync': { def: true as boolean, doc: 'canvas' },
  'recording.maxTotalMb': { def: 4096 as RecordingCapMb, sync: true, doc: 'canvas' },
  'recording.retentionDays': {
    def: 30 as RecordingRetentionDays,
    sync: true,
    doc: 'canvas',
  },
  'recording.introShown': { def: false as boolean, doc: 'canvas' },
} as const;

export type SettingsKey = keyof typeof settingsSchema;
export type SettingValue<K extends SettingsKey> =
  (typeof settingsSchema)[K]['def'];

export const DEFAULT_HOST_PREFS: HostPrefs = {
  connectVia: 'auto',
  routePolicy: 'auto',
  tcpFallback: true,
  autoUpdateAddr: true,
  lockOnEnd: false,
};

export function toggleSensitive(
  list: string[],
  hostId: string,
  on: boolean,
): string[] {
  const withoutHost = list.filter(item => item !== hostId);
  return on ? [...withoutHost, hostId] : withoutHost;
}
