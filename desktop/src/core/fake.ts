// Browser stand-in for the Tauri shell (`pnpm dev` without Tauri, and tests).
// Addresses stay in 100.64.0.x / 192.168.31.x.
import type { CoreErrorLike } from '@app/native/rscore/bindings';
import {
  HOST_FEATURE_PAIR_FILE,
  HOST_FEATURE_PAIR_PASSWORD,
  type Identity,
  type PairedHost,
  type PairFileSummary,
  type PairOutcome,
} from '@app/native/rscore/types';

import { RSCORE_CALL, RSCORE_EVENT, type RsCoreEventPayload } from './bindings';
import {
  DESKTOP_SESSION_EVENT,
  type DesktopInfo,
  type DesktopSessionEvent,
  type DesktopSettings,
} from './desktopApi';
import type { Transport } from './tauri';

export type FakeOptions = {
  info?: Partial<DesktopInfo>;
  paired?: PairedHost[];
  /** The computer a pairing (by address or file) reaches; a successful one adds it to `paired`. */
  pairTarget?: PairedHost;
  /** Answer to pairingStart. */
  pairOutcome?: PairOutcome;
  /** What the file dialog returns; default null (cancelled). */
  pickFile?: string | null;
  /** Per-name `rscore_call` answers; a throw or rejection becomes the call's rejection. */
  rscore?: Record<string, (args: unknown[]) => unknown>;
};

export type FakeTransport = Transport & {
  /** Delivers one `rscore://event`. */
  emit(seq: number, payload: object): void;
  emitSession(event: DesktopSessionEvent): void;
  calls: Array<{ cmd: string; args: unknown }>;
};

const MINUTE_US = 60_000_000;
const DAY_US = 24 * 60 * MINUTE_US;
const FEATURES = 32767 + HOST_FEATURE_PAIR_PASSWORD + HOST_FEATURE_PAIR_FILE;

const STUDIO_ID = 'h7v2m9q3k8x4tb1a';
const STUDIO_FP = 'WppxNkhgT1YNIAQGznjWy3LyuBNcRwVPIlBvhQcqy1w';
const OFFICE_ID = 'w3k8p2x7m4q9tb6c';
const OFFICE_FP = '9o5xYm-IeATETQkkzOt1uxYegCsEjASBlpyGgnaYfks';
const SELF: Identity = {
  deviceId: 'd5k2m8q4x7v1tb3n',
  fp: 'eT3wGbTXY2SXZDXGe5k2EXROTPkRuweeBGS24rqHk1w',
};

export const FAKE_INFO: DesktopInfo = {
  version: '0.1.0',
  platform: 'macos',
  deviceName: '我的 MacBook Pro',
  dataDir: '/Users/demo/Library/Application Support/ai.gxlself.remotescreen.desktop/core',
};

export const FAKE_SETTINGS: DesktopSettings = {
  macCmdToCtrl: true,
  winSwapCtrlMeta: false,
  macKeyboard: 'ansi',
  defaultQuality: 'auto',
};

/** Not paired yet; the fake pairs with it. */
export function fakePairTarget(): PairedHost {
  return {
    hostId: OFFICE_ID,
    fp: OFFICE_FP,
    name: '工位 Windows',
    addresses: ['100.64.0.12:7400', '192.168.31.48:7400'],
    pairedAt: 0,
    os: 'windows',
    osVersion: 'Windows 11',
    hostVersion: '0.3.1',
    features: FEATURES,
    displays: [{ id: 1, name: 'Dell U2723QE', widthPx: 2560, heightPx: 1440, primary: true }],
    lastSeenAt: null,
  };
}

export function fakePaired(nowUs: number): PairedHost[] {
  return [
    {
      hostId: STUDIO_ID,
      fp: STUDIO_FP,
      name: 'Studio Mac mini',
      addresses: ['100.64.0.7:7400', '192.168.31.20:7400'],
      pairedAt: nowUs - 30 * DAY_US,
      os: 'macos',
      osVersion: 'macOS 26.1',
      hostVersion: '0.3.1',
      features: FEATURES,
      displays: [{ id: 1, name: 'Studio Display', widthPx: 5120, heightPx: 2880, primary: true }],
      lastSeenAt: nowUs - 10 * MINUTE_US,
    },
  ];
}

function unsupported(name: string): CoreErrorLike {
  return { tag: 'Unsupported', inner: { message: `not available on desktop: ${name}` } };
}

export function fakeTransport(opts: FakeOptions = {}): FakeTransport {
  const nowUs = Date.now() * 1000;
  const info: DesktopInfo = { ...FAKE_INFO, ...opts.info };
  let settings: DesktopSettings = { ...FAKE_SETTINGS };
  const paired = [...(opts.paired ?? fakePaired(nowUs))];
  const pairTarget = opts.pairTarget ?? fakePairTarget();
  const listeners = new Map<string, Set<(payload: unknown) => void>>();
  const calls: Array<{ cmd: string; args: unknown }> = [];
  let nextSession = 1;

  const outcome = (): PairOutcome => {
    const result: PairOutcome = opts.pairOutcome ?? {
      status: 'ok',
      hostId: pairTarget.hostId,
      hostName: pairTarget.name,
      permission: 'control',
      path: 'tailnetDirect',
      latencyMs: 12,
    };
    if (result.status === 'ok' && !paired.some((h) => h.hostId === result.hostId)) {
      paired.push({ ...pairTarget, pairedAt: Date.now() * 1000 });
    }
    return result;
  };
  const pairFile = (): PairFileSummary => ({
    fileId: 'AAAAAAAAAAAAAAAAAAAAAA',
    hostId: pairTarget.hostId,
    hostName: pairTarget.name,
    fp: pairTarget.fp,
    addrs: [...pairTarget.addresses],
    expiresAt: nowUs + DAY_US,
    expired: false,
  });

  const json = (value: unknown) => JSON.stringify(value);
  const canned: Record<string, (args: unknown[]) => unknown> = {
    coreInit: () => null,
    coreShutdown: () => null,
    identityEnsure: () => json(SELF),
    pairingStart: () => json(outcome()),
    pairingLoadFile: () => json(pairFile()),
    pairingCancel: () => null,
    hostsPaired: () => json(paired),
    hostsForget: ([hostId]) => {
      const index = paired.findIndex((h) => h.hostId === hostId);
      if (index >= 0) {
        paired.splice(index, 1);
      }
      return null;
    },
    sessionsPrewarm: () => null,
    sessionsClose: () => null,
    sessionsSetQuality: () => null,
    sessionsSelectDisplay: () => null,
    sessionsRequestControl: () => null,
    sessionsRespondControl: () => null,
    sessionsAction: () => null,
    sessionsCombo: () => null,
    sessionsList: () => json([]),
    diagProbe: () => '1',
    diagProbeAddress: () => '2',
    networkStatus: () => json({ ifaces: [] }),
    settingsSet: () => null,
    logsWrite: () => null,
    logsExport: () => '/Users/demo/Downloads/remote-screen-logs.zip',
  };

  const rscore = async ({ name, args }: { name: string; args: unknown[] }) => {
    const handler = opts.rscore?.[name] ?? canned[name];
    if (handler === undefined) {
      throw unsupported(name);
    }
    return handler(args);
  };

  const commands: Record<string, (args: Record<string, unknown>) => unknown> = {
    [RSCORE_CALL]: (args) => rscore(args as { name: string; args: unknown[] }),
    desktop_info: () => ({ ...info }),
    desktop_open_session: () => String(nextSession++),
    desktop_settings_get: () => ({ ...settings }),
    desktop_settings_set: (args) => {
      settings = { ...(args.settings as DesktopSettings) };
      return null;
    },
  };

  const deliver = (event: string, payload: unknown) => {
    for (const cb of [...(listeners.get(event) ?? [])]) {
      cb(payload);
    }
  };

  return {
    calls,
    async invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
      calls.push({ cmd, args });
      const command = commands[cmd];
      if (command === undefined) {
        throw { tag: 'Internal', inner: { message: `unknown command: ${cmd}` } };
      }
      return (await command(args ?? {})) as T;
    },
    async listen<T>(event: string, cb: (payload: T) => void) {
      const set = listeners.get(event) ?? new Set();
      const listener = cb as (payload: unknown) => void;
      set.add(listener);
      listeners.set(event, set);
      return () => {
        set.delete(listener);
      };
    },
    async pickFile() {
      return opts.pickFile ?? null;
    },
    emit(seq, payload) {
      const event: RsCoreEventPayload = { seq: String(seq), payload: JSON.stringify(payload) };
      deliver(RSCORE_EVENT, event);
    },
    emitSession(event) {
      deliver(DESKTOP_SESSION_EVENT, event);
    },
  };
}
