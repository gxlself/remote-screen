import { RsCoreError } from './RsCoreApi';
import type { RsCoreApi } from './RsCoreApi';
import type { EventSinkLike, RsMobileBindings } from './bindings';
import { fromNativeId, toNativeId, toRsCoreError } from './errors';
import type * as T from './types';

function call<Result>(invoke: () => Result): Result {
  try {
    return invoke();
  } catch (e) {
    throw toRsCoreError(e);
  }
}

async function callAsync<Result>(
  invoke: () => Promise<Result>,
): Promise<Result> {
  try {
    return await invoke();
  } catch (e) {
    throw toRsCoreError(e);
  }
}

function parseJson<Result>(payload: string, method: string): Result {
  try {
    return JSON.parse(payload) as Result;
  } catch {
    throw new RsCoreError('internal', `bad json from core: ${method}`);
  }
}

function readJson<Result>(
  method: string,
  invoke: () => Promise<string>,
): Promise<Result> {
  return callAsync(async () => parseJson<Result>(await invoke(), method));
}

function need<Args extends unknown[], Result>(
  method: string,
  fn: ((...args: Args) => Result) | undefined,
): (...args: Args) => Result {
  if (fn !== undefined) {
    return fn;
  }
  return () => {
    throw new RsCoreError('unsupported', `${method}: binding missing`);
  };
}

export function createRsCore(b: RsMobileBindings): RsCoreApi {
  const subscribers = new Map<symbol, (event: T.CoreEvent) => void>();
  let subscriptionId: bigint | undefined;
  let activeSink: EventSinkLike | undefined;

  const api: RsCoreApi = {
    init: cfg => callAsync(() => b.coreInit(JSON.stringify(cfg))),
    events: {
      subscribe: cb =>
        call(() => {
          const token = Symbol();
          subscribers.set(token, cb);
          if (!activeSink) {
            let lastSeq: bigint | undefined;
            const sink: EventSinkLike = {
              onEvent(seq, payload) {
                if (activeSink !== sink) {
                  return;
                }
                if (lastSeq !== undefined && seq !== lastSeq + 1n) {
                  console.warn('RsCore event sequence gap');
                }
                lastSeq = seq;
                let event: unknown;
                try {
                  event = JSON.parse(payload);
                } catch {
                  return;
                }
                if (
                  typeof event !== 'object' ||
                  event === null ||
                  !('type' in event) ||
                  typeof event.type !== 'string'
                ) {
                  return;
                }
                for (const subscriber of [...subscribers.values()]) {
                  try {
                    subscriber(event as T.CoreEvent);
                  } catch {
                    // Do not log event payloads or subscriber error messages.
                    console.warn('RsCore event subscriber threw');
                  }
                }
              },
            };
            activeSink = sink;
            try {
              subscriptionId = b.eventsSubscribe(sink);
            } catch (e) {
              activeSink = undefined;
              subscribers.delete(token);
              throw e;
            }
          }
          let unsubscribed = false;
          return () =>
            call(() => {
              if (unsubscribed) {
                return;
              }
              unsubscribed = true;
              subscribers.delete(token);
              if (subscribers.size === 0 && subscriptionId !== undefined) {
                const id = subscriptionId;
                subscriptionId = undefined;
                activeSink = undefined;
                b.eventsUnsubscribe(id);
              }
            });
        }),
    },
    identity: {
      ensure: name =>
        readJson<T.Identity>('identity.ensure', () => b.identityEnsure(name)),
    },
    pairing: {
      parseQr: text =>
        call(() => {
          const json = b.pairingParseQr(text);
          return json === undefined
            ? null
            : parseJson<T.PairTarget>(json, 'pairing.parseQr');
        }),
      start: target =>
        readJson<T.PairOutcome>('pairing.start', () =>
          b.pairingStart(JSON.stringify(target)),
        ),
      loadFile: src =>
        readJson<T.PairFileSummary>('pairing.loadFile', () =>
          b.pairingLoadFile(JSON.stringify(src)),
        ),
      cancel: () => call(() => b.pairingCancel()),
    },
    hosts: {
      paired: () =>
        readJson<T.PairedHost[]>('hosts.paired', () => b.hostsPaired()),
      forget: id => callAsync(() => b.hostsForget(id)),
    },
    sessions: {
      prewarm: ids => call(() => b.sessionsPrewarm(ids)),
      open: (id, opts) =>
        callAsync(async () =>
          fromNativeId(await b.sessionsOpen(id, JSON.stringify(opts))),
        ),
      close: id => call(() => b.sessionsClose(toNativeId(id))),
      list: () =>
        call(() =>
          parseJson<T.SessionSummary[]>(b.sessionsList(), 'sessions.list'),
        ),
      setQuality: (id, quality) =>
        call(() => b.sessionsSetQuality(toNativeId(id), quality)),
      setAudio: (id, enabled) =>
        call(() => {
          if (!b.sessionsSetAudio) {
            throw new RsCoreError('unsupported', 'sessions.setAudio');
          }
          b.sessionsSetAudio(toNativeId(id), enabled);
        }),
      selectDisplay: (id, display) =>
        call(() => b.sessionsSelectDisplay(toNativeId(id), display)),
      createVirtualDisplay: (id, opts) =>
        call(() => {
          if (!b.sessionsCreateVirtualDisplay) {
            throw new RsCoreError(
              'unsupported',
              'sessions.createVirtualDisplay',
            );
          }
          return b.sessionsCreateVirtualDisplay(
            toNativeId(id),
            JSON.stringify(opts),
          );
        }),
      releaseVirtualDisplay: (id, displayId) =>
        call(() => {
          if (!b.sessionsReleaseVirtualDisplay) {
            throw new RsCoreError(
              'unsupported',
              'sessions.releaseVirtualDisplay',
            );
          }
          b.sessionsReleaseVirtualDisplay(toNativeId(id), displayId);
        }),
      requestControl: id =>
        call(() => b.sessionsRequestControl(toNativeId(id))),
      respondControl: (id, requester, allow) =>
        call(() => b.sessionsRespondControl(toNativeId(id), requester, allow)),
      action: (id, kind, enable) =>
        call(() => b.sessionsAction(toNativeId(id), kind, enable)),
      key: (id, usage, down, mods) =>
        call(() => b.sessionsKey(toNativeId(id), usage, down, mods)),
      text: (id, text) => call(() => b.sessionsText(toNativeId(id), text)),
      combo: (id, combo) => call(() => b.sessionsCombo(toNativeId(id), combo)),
      clipboardSend: (id, text) =>
        call(() => b.sessionsClipboardSend(toNativeId(id), text)),
      background: (id, inBackground) =>
        call(() => b.sessionsBackground(toNativeId(id), inBackground)),
    },
    diag: {
      probe: id => callAsync(async () => fromNativeId(await b.diagProbe(id))),
      probeAddress: (addr, port) =>
        callAsync(async () =>
          fromNativeId(await b.diagProbeAddress(addr, port)),
        ),
    },
    logs: {
      export: opts => callAsync(() => b.logsExport(opts.mask)),
      write: (level, tag, msg) => call(() => b.logsWrite(level, tag, msg)),
    },
    ssh: {
      servers: () =>
        readJson<T.SshServer[]>('ssh.servers', () => b.sshServers()),
      open: (id, opts) =>
        callAsync(async () =>
          fromNativeId(
            await b.sshOpen(id, opts ? JSON.stringify(opts) : undefined),
          ),
        ),
      close: id => call(() => b.sshClose(toNativeId(id))),
      keys: () => readJson<T.SshKey[]>('ssh.keys', () => b.sshKeys()),
      createKey: (kind, name) =>
        readJson<T.SshKey>('ssh.createKey', () => b.sshCreateKey(kind, name)),
      installKey: (id, key) => callAsync(() => b.sshInstallKey(id, key)),
      trustHostKey: (id, fp) => callAsync(() => b.sshTrustHostKey(id, fp)),
      trustedHostKeys: () =>
        readJson<T.TrustedHostKey[]>('ssh.trustedHostKeys', () =>
          b.sshTrustedHostKeys(),
        ),
      forgetHostKey: id => callAsync(() => b.sshForgetHostKey(id)),
    },
    files: {
      list: (id, path) =>
        readJson<T.FileListing>('files.list', () =>
          need('files.list', b.filesList)(id, path ?? undefined),
        ),
      download: (id, path) =>
        callAsync(() => need('files.download', b.filesDownload)(id, path)),
      upload: (id, path, dir) =>
        callAsync(() =>
          need('files.upload', b.filesUpload)(id, path, dir ?? undefined),
        ),
    },
    sftp: {
      list: (id, path) =>
        readJson<T.FileListing>('sftp.list', () =>
          need('sftp.list', b.sftpList)(id, path ?? undefined),
        ),
      download: (id, path) =>
        callAsync(() => need('sftp.download', b.sftpDownload)(id, path)),
      upload: (id, path, dir) =>
        callAsync(() =>
          need('sftp.upload', b.sftpUpload)(id, path, dir ?? undefined),
        ),
    },
    transfers: {
      list: () =>
        call(() =>
          parseJson<T.FileTransfer[]>(
            need('transfers.list', b.transfersList)(),
            'transfers.list',
          ),
        ),
      pause: id => call(() => need('transfers.pause', b.transfersPause)(id)),
      resume: id =>
        callAsync(() => need('transfers.resume', b.transfersResume)(id)),
      cancel: id => call(() => need('transfers.cancel', b.transfersCancel)(id)),
      remove: id => call(() => need('transfers.remove', b.transfersRemove)(id)),
    },
    tailnet: {
      available: () => {
        const fn = b.tailnetAvailable;
        return fn === undefined
          ? Promise.resolve(false)
          : callAsync(() => fn());
      },
      status: () =>
        readJson<T.TailnetStatus>('tailnet.status', () =>
          need('tailnet.status', b.tailnetStatus)(),
        ),
      login: () => callAsync(() => need('tailnet.login', b.tailnetLogin)()),
      logout: () => callAsync(() => need('tailnet.logout', b.tailnetLogout)()),
    },
    network: {
      status: () =>
        readJson<T.NetworkStatus>('network.status', () => b.networkStatus()),
    },
    settings: {
      set: (key, value) =>
        call(() =>
          b.settingsSet(
            key,
            JSON.stringify(value === undefined ? null : value),
          ),
        ),
    },
    crash: {
      recordJs: report =>
        call(() =>
          need('crash.recordJs', b.crashRecordJs)(JSON.stringify(report)),
        ),
    },
    recording: {
      start: id =>
        readJson<T.RecordingStatus>('recording.start', () =>
          need('recording.start', b.recordingStart)(toNativeId(id)),
        ),
      stop: id =>
        call(() => need('recording.stop', b.recordingStop)(toNativeId(id))),
      status: id =>
        call(() =>
          parseJson<T.RecordingStatus>(
            need('recording.status', b.recordingStatus)(toNativeId(id)),
            'recording.status',
          ),
        ),
      list: () =>
        readJson<T.RecordingList>('recording.list', () =>
          need('recording.list', b.recordingsList)(),
        ),
      remove: id =>
        callAsync(() => need('recording.remove', b.recordingsDelete)(id)),
      open: id =>
        readJson<T.PlaybackInfo>('recording.open', () =>
          need('recording.open', b.playbackOpen)(id),
        ),
      control: (id, command) =>
        call(() =>
          need('recording.control', b.playbackControl)(
            toNativeId(id),
            JSON.stringify(command),
          ),
        ),
      close: id =>
        call(() => need('recording.close', b.playbackClose)(toNativeId(id))),
      exportMp4: id =>
        readJson<T.RecordingExport>('recording.exportMp4', () =>
          need('recording.exportMp4', b.recordingsExport)(id),
        ),
      discardExport: path =>
        call(() =>
          need('recording.discardExport', b.recordingsDiscardExport)(path),
        ),
    },
    adapters: {
      available: () => b.adaptersOpen !== undefined,
      open: (target, opts) =>
        callAsync(async () =>
          fromNativeId(
            await need('adapters.open', b.adaptersOpen)(
              JSON.stringify(target),
              JSON.stringify(opts),
            ),
          ),
        ),
      close: id =>
        call(() => need('adapters.close', b.adaptersClose)(toNativeId(id))),
      list: () =>
        call(() =>
          parseJson<T.AdapterSummary[]>(
            need('adapters.list', b.adaptersList)(),
            'adapters.list',
          ),
        ),
      submitCredentials: (id, creds) =>
        call(() =>
          need('adapters.submitCredentials', b.adaptersSubmitCredentials)(
            toNativeId(id),
            creds.user,
            creds.password,
            creds.domain,
            creds.save,
          ),
        ),
      cancelCredentials: id =>
        call(() =>
          need(
            'adapters.cancelCredentials',
            b.adaptersCancelCredentials,
          )(toNativeId(id)),
        ),
      saved: target =>
        call(() =>
          parseJson<T.SavedCredentialInfo>(
            need('adapters.saved', b.adaptersSaved)(JSON.stringify(target)),
            'adapters.saved',
          ),
        ),
      forget: target =>
        call(() =>
          need('adapters.forget', b.adaptersForget)(JSON.stringify(target)),
        ),
      canSave: () => call(() => b.adaptersCanSave?.() ?? false),
      typePassword: id =>
        call(() =>
          need('adapters.typePassword', b.adaptersTypePassword)(
            toNativeId(id),
          ),
        ),
      sendCtrlAltDel: id =>
        call(() =>
          need('adapters.sendCtrlAltDel', b.adaptersSendCtrlAltDel)(
            toNativeId(id),
          ),
        ),
      trustCert: (id, accept) =>
        call(() =>
          need('adapters.trustCert', b.adaptersTrustCert)(
            toNativeId(id),
            accept,
          ),
        ),
      resize: (id, viewWidth, viewHeight, scale) =>
        call(() =>
          need('adapters.resize', b.adaptersResize)(
            toNativeId(id),
            viewWidth,
            viewHeight,
            scale,
          ),
        ),
      selectScreen: (id, index) =>
        call(() =>
          need('adapters.selectScreen', b.adaptersSelectScreen)(
            toNativeId(id),
            index,
          ),
        ),
      setMonitors: (id, count) =>
        call(() =>
          need('adapters.setMonitors', b.adaptersSetMonitors)(
            toNativeId(id),
            count,
          ),
        ),
    },
  };
  if (b.discoverProbe !== undefined) {
    api.discover = {
      probe: (addr, port, kind, timeoutMs) =>
        readJson<T.DiscoverProbe>('discover.probe', () =>
          need('discover.probe', b.discoverProbe)(addr, port, kind, timeoutMs),
        ),
      scanLan: ports =>
        callAsync(async () =>
          fromNativeId(await need('discover.scanLan', b.discoverScanLan)(ports)),
        ),
      cancelScan: scanId =>
        call(() =>
          need('discover.cancelScan', b.discoverScanCancel)(toNativeId(scanId)),
        ),
    };
  }
  if (b.tailscaleApiDevices !== undefined) {
    api.tailscaleApi = {
      setToken: (token, controlUrl) =>
        readJson<T.TailscaleDevice[]>('tailscaleApi.setToken', () =>
          need('tailscaleApi.setToken', b.tailscaleApiSetToken)(
            token,
            controlUrl ?? undefined,
          ),
        ),
      clearToken: () =>
        callAsync(() =>
          need('tailscaleApi.clearToken', b.tailscaleApiClearToken)(),
        ),
      status: () =>
        readJson<T.TailscaleApiStatus>('tailscaleApi.status', () =>
          need('tailscaleApi.status', b.tailscaleApiStatus)(),
        ),
      devices: force =>
        readJson<T.TailscaleDevice[]>('tailscaleApi.devices', () =>
          need('tailscaleApi.devices', b.tailscaleApiDevices)(force),
        ),
    };
  }
  return api;
}
