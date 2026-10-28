import type {
  CoreErrorLike,
  CoreErrorTag,
  EventSinkLike,
  RsMobileBindings,
} from './bindings';

export interface FakeBindings extends RsMobileBindings {
  calls: Array<{ fn: keyof RsMobileBindings; args: unknown[] }>;
  respond(fn: keyof RsMobileBindings, valueOrError: unknown): void;
  emit(payload: object): void;
  subscriberCount(): number;
  coreError(
    tag: CoreErrorTag,
    inner: CoreErrorLike['inner'],
  ): Error & CoreErrorLike;
}

/** One-shot responses mimic the generated module, including synchronous throws. */
export function createFakeBindings(): FakeBindings {
  const calls: FakeBindings['calls'] = [];
  const responses = new Map<keyof RsMobileBindings, unknown>();
  const sinks = new Map<bigint, EventSinkLike>();
  let nextId = 1n;
  let seq = 0n;

  function invoke<T>(
    fn: keyof RsMobileBindings,
    args: unknown[],
    fallback: () => T,
  ): T {
    calls.push({ fn, args });
    if (responses.has(fn)) {
      const response = responses.get(fn);
      responses.delete(fn);
      if (response instanceof Error) {
        throw response;
      }
      return response as T;
    }
    return fallback();
  }

  const id = () => {
    const result = nextId;
    nextId += 1n;
    return result;
  };
  const empty = () => undefined;
  const object = () => '{}';
  const array = () => '[]';
  const fileListing = () =>
    JSON.stringify({
      path: '/',
      parent: null,
      entries: [],
      truncated: false,
      netPath: 'tailnetDirect',
    });
  const transfers = () => '[]';

  return {
    calls,
    respond: (fn, response) => {
      responses.set(fn, response);
    },
    emit: payload => {
      seq += 1n;
      const json = JSON.stringify(payload);
      for (const sink of [...sinks.values()]) {
        sink.onEvent(seq, json);
      }
    },
    subscriberCount: () => sinks.size,
    coreError: (tag, inner) =>
      Object.assign(new Error(inner.message), { tag, inner }),
    coreInit: async (...args) => invoke('coreInit', args, empty),
    coreShutdown: (...args) =>
      invoke('coreShutdown', args, () => sinks.clear()),
    eventsSubscribe: (...args) => {
      const subscriptionId = invoke('eventsSubscribe', args, id);
      sinks.set(subscriptionId, args[0]);
      return subscriptionId;
    },
    eventsUnsubscribe: (...args) => {
      invoke('eventsUnsubscribe', args, empty);
      sinks.delete(args[0]);
    },
    identityEnsure: async (...args) => invoke('identityEnsure', args, object),
    pairingParseQr: (...args) => invoke('pairingParseQr', args, empty),
    pairingStart: async (...args) => invoke('pairingStart', args, object),
    pairingLoadFile: async (...args) => invoke('pairingLoadFile', args, object),
    pairingCancel: (...args) => invoke('pairingCancel', args, empty),
    hostsPaired: async (...args) => invoke('hostsPaired', args, array),
    hostsForget: async (...args) => invoke('hostsForget', args, empty),
    sessionsPrewarm: (...args) => invoke('sessionsPrewarm', args, empty),
    sessionsOpen: async (...args) => invoke('sessionsOpen', args, id),
    sessionsClose: (...args) => invoke('sessionsClose', args, empty),
    sessionsList: (...args) => invoke('sessionsList', args, array),
    sessionsSetQuality: (...args) => invoke('sessionsSetQuality', args, empty),
    sessionsSetAudio: (...args) => invoke('sessionsSetAudio', args, empty),
    sessionsSelectDisplay: (...args) =>
      invoke('sessionsSelectDisplay', args, empty),
    sessionsCreateVirtualDisplay: (...args) =>
      invoke('sessionsCreateVirtualDisplay', args, () => 1),
    sessionsReleaseVirtualDisplay: (...args) =>
      invoke('sessionsReleaseVirtualDisplay', args, empty),
    sessionsRequestControl: (...args) =>
      invoke('sessionsRequestControl', args, empty),
    sessionsRespondControl: (...args) =>
      invoke('sessionsRespondControl', args, empty),
    sessionsAction: (...args) => invoke('sessionsAction', args, empty),
    sessionsKey: (...args) => invoke('sessionsKey', args, empty),
    sessionsText: (...args) => invoke('sessionsText', args, empty),
    sessionsCombo: (...args) => invoke('sessionsCombo', args, empty),
    sessionsClipboardSend: (...args) =>
      invoke('sessionsClipboardSend', args, empty),
    sessionsBackground: (...args) => invoke('sessionsBackground', args, empty),
    diagProbe: async (...args) => invoke('diagProbe', args, id),
    diagProbeAddress: async (...args) => invoke('diagProbeAddress', args, id),
    logsExport: async (...args) => invoke('logsExport', args, () => 'logs.zip'),
    logsWrite: (...args) => invoke('logsWrite', args, empty),
    sshServers: async (...args) => invoke('sshServers', args, array),
    sshOpen: async (...args) => invoke('sshOpen', args, id),
    sshClose: (...args) => invoke('sshClose', args, empty),
    sshKeys: async (...args) => invoke('sshKeys', args, array),
    sshCreateKey: async (...args) => invoke('sshCreateKey', args, object),
    sshInstallKey: async (...args) => invoke('sshInstallKey', args, empty),
    sshTrustHostKey: async (...args) => invoke('sshTrustHostKey', args, empty),
    sshTrustedHostKeys: async (...args) =>
      invoke('sshTrustedHostKeys', args, array),
    sshForgetHostKey: async (...args) =>
      invoke('sshForgetHostKey', args, empty),
    filesList: async (...args) => invoke('filesList', args, fileListing),
    filesDownload: async (...args) =>
      invoke('filesDownload', args, () => 'transfer-1'),
    filesUpload: async (...args) =>
      invoke('filesUpload', args, () => 'transfer-1'),
    sftpList: async (...args) => invoke('sftpList', args, fileListing),
    sftpDownload: async (...args) =>
      invoke('sftpDownload', args, () => 'transfer-1'),
    sftpUpload: async (...args) =>
      invoke('sftpUpload', args, () => 'transfer-1'),
    transfersList: (...args) => invoke('transfersList', args, transfers),
    transfersPause: (...args) => invoke('transfersPause', args, empty),
    transfersResume: async (...args) => invoke('transfersResume', args, empty),
    transfersCancel: (...args) => invoke('transfersCancel', args, empty),
    transfersRemove: (...args) => invoke('transfersRemove', args, empty),
    tailnetAvailable: async (...args) =>
      invoke('tailnetAvailable', args, () => false),
    tailnetStatus: async (...args) =>
      invoke('tailnetStatus', args, () =>
        JSON.stringify({
          state: 'unavailable',
          authUrl: null,
          self: null,
          magicDnsSuffix: '',
          peers: [],
        }),
      ),
    tailnetLogin: async (...args) => invoke('tailnetLogin', args, empty),
    tailnetLogout: async (...args) => invoke('tailnetLogout', args, empty),
    networkStatus: async (...args) => invoke('networkStatus', args, object),
    settingsSet: (...args) => invoke('settingsSet', args, empty),
    crashRecordJs: (...args) => invoke('crashRecordJs', args, () => true),
    recordingStart: async (...args) =>
      invoke('recordingStart', args, object),
    recordingStop: (...args) => invoke('recordingStop', args, empty),
    recordingStatus: (...args) => invoke('recordingStatus', args, object),
    recordingsList: async (...args) => invoke('recordingsList', args, object),
    recordingsDelete: async (...args) =>
      invoke('recordingsDelete', args, empty),
    playbackOpen: async (...args) => invoke('playbackOpen', args, object),
    playbackControl: (...args) => invoke('playbackControl', args, empty),
    playbackClose: (...args) => invoke('playbackClose', args, empty),
    recordingsExport: async (...args) =>
      invoke('recordingsExport', args, object),
    recordingsDiscardExport: (...args) =>
      invoke('recordingsDiscardExport', args, empty),
  };
}
