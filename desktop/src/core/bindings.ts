import type { CoreErrorLike, EventSinkLike, RsMobileBindings } from '@app/native/rscore/bindings';

import type { Transport } from './tauri';

/** Payload of the `rscore://event` Tauri event; `seq` is a decimal u64. */
export type RsCoreEventPayload = { seq: string; payload: string };

export const RSCORE_CALL = 'rscore_call';
export const RSCORE_EVENT = 'rscore://event';

function encode(arg: unknown): unknown {
  if (typeof arg === 'bigint') {
    return arg.toString();
  }
  return arg === undefined ? null : arg;
}

function unsupported(message: string): CoreErrorLike {
  return { tag: 'Unsupported', inner: { message } };
}

/**
 * `RsMobileBindings` over the Tauri shell: every Rust call goes through
 * `invoke('rscore_call', { name, args })` with args in binding order (07 · 电脑端).
 * Rejections are the shell's `CallError`, already shaped like `CoreErrorLike`, and
 * pass through untouched so `createRsCore` converts them.
 */
export function desktopBindings(t: Transport): RsMobileBindings {
  const call = <T>(name: string, args: unknown[]) =>
    t.invoke<T>(RSCORE_CALL, { name, args: args.map(encode) });
  const text =
    (name: string) =>
    (...args: unknown[]) =>
      call<string>(name, args);
  const done =
    (name: string) =>
    async (...args: unknown[]) => {
      await call<null>(name, args);
    };
  const nativeId =
    (name: string) =>
    async (...args: unknown[]) =>
      BigInt(await call<string>(name, args));
  const fire =
    (name: string) =>
    (...args: unknown[]) => {
      call<null>(name, args).catch(() => {
        // Arguments carry session ids and typed text; only the name is logged.
        console.warn('rscore call failed', name);
      });
    };
  const absent = (name: string) => () =>
    Promise.reject(unsupported(`not available on desktop: ${name}`));

  const sinks = new Map<bigint, EventSinkLike>();
  let nextSinkId = 1n;
  let listening = false;
  const listenOnce = () => {
    if (listening) {
      return;
    }
    listening = true;
    t.listen<RsCoreEventPayload>(RSCORE_EVENT, (event) => {
      let seq: bigint;
      try {
        seq = BigInt(event.seq);
      } catch {
        return;
      }
      for (const sink of [...sinks.values()]) {
        sink.onEvent(seq, event.payload);
      }
    }).catch(() => {
      console.warn('rscore event listen failed');
    });
  };

  return {
    async coreInit(cfgJson) {
      await call<null>('coreInit', [cfgJson]);
    },
    coreShutdown: fire('coreShutdown'),
    eventsSubscribe(sink) {
      listenOnce();
      const id = nextSinkId;
      nextSinkId += 1n;
      sinks.set(id, sink);
      return id;
    },
    eventsUnsubscribe(id) {
      sinks.delete(id);
    },
    identityEnsure: text('identityEnsure'),
    // The desktop does not scan QR codes; the format belongs to rsp_core::qr::parse.
    pairingParseQr: () => undefined,
    pairingStart: text('pairingStart'),
    pairingLoadFile: text('pairingLoadFile'),
    pairingCancel: fire('pairingCancel'),
    hostsPaired: text('hostsPaired'),
    hostsForget: done('hostsForget'),
    sessionsPrewarm: fire('sessionsPrewarm'),
    sessionsOpen: nativeId('sessionsOpen'),
    sessionsClose: fire('sessionsClose'),
    sessionsList() {
      // Sessions live in native windows; pages ask DesktopApi instead.
      throw unsupported('sessions.list: use desktop');
    },
    sessionsSetQuality: fire('sessionsSetQuality'),
    sessionsSelectDisplay: fire('sessionsSelectDisplay'),
    sessionsRequestControl: fire('sessionsRequestControl'),
    sessionsRespondControl: fire('sessionsRespondControl'),
    sessionsAction: fire('sessionsAction'),
    sessionsKey: fire('sessionsKey'),
    sessionsText: fire('sessionsText'),
    sessionsCombo: fire('sessionsCombo'),
    sessionsClipboardSend: fire('sessionsClipboardSend'),
    sessionsBackground: fire('sessionsBackground'),
    diagProbe: nativeId('diagProbe'),
    diagProbeAddress: nativeId('diagProbeAddress'),
    logsExport: text('logsExport'),
    logsWrite: fire('logsWrite'),
    // SSH has no desktop UI in the first version; the shell
    // rejects these names too, so answer locally without a round trip.
    sshServers: absent('sshServers'),
    sshOpen: absent('sshOpen'),
    sshClose() {
      throw unsupported('not available on desktop: sshClose');
    },
    sshKeys: absent('sshKeys'),
    sshCreateKey: absent('sshCreateKey'),
    sshInstallKey: absent('sshInstallKey'),
    sshTrustHostKey: absent('sshTrustHostKey'),
    sshTrustedHostKeys: absent('sshTrustedHostKeys'),
    sshForgetHostKey: absent('sshForgetHostKey'),
    networkStatus: text('networkStatus'),
    settingsSet: fire('settingsSet'),
  };
}
