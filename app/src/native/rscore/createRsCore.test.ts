import { GAP_METHODS, RsCoreError } from './RsCoreApi';
import type { RsCoreApi, Unsubscribe } from './RsCoreApi';
import type { EventSinkLike, RsMobileBindings } from './bindings';
import { createRsCore } from './createRsCore';
import { createFakeBindings } from './fakeBindings';
import * as fixtures from './fixtures';
import type { CoreEvent, DiscoverProbe, FileTransfer, TailscaleApiStatus, TailscaleDevice } from './types';

type MethodPaths<T> = {
  [K in keyof T & string]: T[K] extends (...args: never[]) => unknown
    ? K
    : `${K}.${MethodPaths<NonNullable<T[K]>>}`;
}[keyof T & string];

const cfg = {
  dataDir: 'core-data',
  appVersion: '0.3.1',
  platform: 'ios',
} as const;

// Keep this explicit: adding an API or binding requires updating the coverage list.
const methods: Array<
  [MethodPaths<RsCoreApi>, keyof RsMobileBindings, unknown[]]
> = [
  ['init', 'coreInit', [cfg]],
  ['events.subscribe', 'eventsSubscribe', [() => undefined]],
  ['identity.ensure', 'identityEnsure', ['phone']],
  ['pairing.parseQr', 'pairingParseQr', ['bad']],
  ['pairing.start', 'pairingStart', [{ addr: '100.64.0.7', code: '123456' }]],
  ['pairing.loadFile', 'pairingLoadFile', [{ text: 'pair file' }]],
  ['pairing.cancel', 'pairingCancel', []],
  ['hosts.paired', 'hostsPaired', []],
  ['hosts.forget', 'hostsForget', ['h1']],
  ['sessions.prewarm', 'sessionsPrewarm', [['h1', 'h2']]],
  ['sessions.open', 'sessionsOpen', ['h1', { intent: 'view' }]],
  ['sessions.close', 'sessionsClose', ['1']],
  ['sessions.list', 'sessionsList', []],
  ['sessions.setQuality', 'sessionsSetQuality', ['1', 'clear']],
  ['sessions.setAudio', 'sessionsSetAudio', ['1', true]],
  ['sessions.selectDisplay', 'sessionsSelectDisplay', ['1', 2]],
  [
    'sessions.createVirtualDisplay',
    'sessionsCreateVirtualDisplay',
    ['1', { widthPx: 2732, heightPx: 2048 }],
  ],
  ['sessions.releaseVirtualDisplay', 'sessionsReleaseVirtualDisplay', ['1', 3]],
  ['sessions.requestControl', 'sessionsRequestControl', ['1']],
  ['sessions.respondControl', 'sessionsRespondControl', ['1', 'd1', true]],
  ['sessions.action', 'sessionsAction', ['1', 'lock']],
  ['sessions.key', 'sessionsKey', ['1', 4, true, 2]],
  ['sessions.text', 'sessionsText', ['1', 'hello']],
  ['sessions.combo', 'sessionsCombo', ['1', 'cmdTab']],
  ['sessions.clipboardSend', 'sessionsClipboardSend', ['1', 'hello']],
  ['sessions.background', 'sessionsBackground', ['1', true]],
  ['diag.probe', 'diagProbe', ['h1']],
  ['diag.probeAddress', 'diagProbeAddress', ['100.64.0.7', 7400]],
  ['logs.export', 'logsExport', [{ mask: true }]],
  ['logs.write', 'logsWrite', ['info', 'test', 'message']],
  ['ssh.servers', 'sshServers', []],
  ['ssh.open', 'sshOpen', ['s1']],
  ['ssh.close', 'sshClose', ['1']],
  ['ssh.keys', 'sshKeys', []],
  ['ssh.createKey', 'sshCreateKey', ['ed25519', 'device']],
  ['ssh.installKey', 'sshInstallKey', ['s1', 'device']],
  ['ssh.trustHostKey', 'sshTrustHostKey', ['s1', 'SHA256:example']],
  ['ssh.trustedHostKeys', 'sshTrustedHostKeys', []],
  ['ssh.forgetHostKey', 'sshForgetHostKey', ['s1']],
  ['files.list', 'filesList', ['h', null]],
  ['files.download', 'filesDownload', ['h', '/file']],
  ['files.upload', 'filesUpload', ['h', '/file', null]],
  ['sftp.list', 'sftpList', ['h', null]],
  ['sftp.download', 'sftpDownload', ['h', '/file']],
  ['sftp.upload', 'sftpUpload', ['h', '/file', null]],
  ['transfers.list', 'transfersList', []],
  ['transfers.pause', 'transfersPause', ['transfer-1']],
  ['transfers.resume', 'transfersResume', ['transfer-1']],
  ['transfers.cancel', 'transfersCancel', ['transfer-1']],
  ['transfers.remove', 'transfersRemove', ['transfer-1']],
  ['tailnet.available', 'tailnetAvailable', []],
  ['tailnet.status', 'tailnetStatus', []],
  ['tailnet.login', 'tailnetLogin', []],
  ['tailnet.logout', 'tailnetLogout', []],
  ['network.status', 'networkStatus', []],
  ['settings.set', 'settingsSet', ['quality.default', 'clear']],
  [
    'crash.recordJs',
    'crashRecordJs',
    [
      {
        name: 'Error',
        message: 'x',
        stack: null,
        isFatal: true,
        osVersion: '17.4',
      },
    ],
  ],
  ['recording.start', 'recordingStart', ['1']],
  ['recording.stop', 'recordingStop', ['1']],
  ['recording.status', 'recordingStatus', ['1']],
  ['recording.list', 'recordingsList', []],
  ['recording.remove', 'recordingsDelete', ['r1']],
  ['recording.open', 'playbackOpen', ['r1']],
  ['recording.control', 'playbackControl', ['1', { op: 'play' }]],
  ['recording.close', 'playbackClose', ['1']],
  ['recording.exportMp4', 'recordingsExport', ['r1']],
  ['recording.discardExport', 'recordingsDiscardExport', ['/tmp/export.mp4']],
];

function invoke(
  core: RsCoreApi,
  path: string,
  args: readonly unknown[],
): unknown {
  let value: unknown = core;
  for (const part of path.split('.')) {
    value = (value as Record<string, unknown>)[part];
  }
  return (value as (...params: unknown[]) => unknown)(...args);
}

function paths(object: object): string[] {
  return Object.entries(object).flatMap(([key, value]) =>
    key === 'adapters'
      ? []
      : typeof value === 'function'
        ? [key]
        : paths(value as object).map(path => `${key}.${path}`),
  );
}

const fileListings = {
  files: {
    path: '/Users/demo',
    parent: null,
    entries: [],
    truncated: false,
    netPath: 'tailnetDirect',
  },
  sftp: {
    path: '/home/demo',
    parent: null,
    entries: [],
    truncated: false,
    netPath: null,
  },
} as const;

const transfer: FileTransfer = {
  transferId: 't1',
  direction: 'download',
  source: { kind: 'computer', hostId: 'h1' },
  name: 'a.bin',
  remotePath: '/Users/demo/a.bin',
  localPath: null,
  size: 10,
  done: 0,
  bytesPerSec: 0,
  state: 'queued',
  fail: null,
  netPath: 'tailnetDirect',
  savedName: null,
  startedAt: fixtures.FIXTURE_NOW,
  updatedAt: fixtures.FIXTURE_NOW,
};

describe('createRsCore', () => {
  it('forwards discover.probe and parses its JSON result', async () => {
    const b = createFakeBindings();
    const probe: DiscoverProbe = {
      addr: '192.168.31.20',
      port: 5900,
      outcome: 'open',
      ms: 12,
      banner: 'RFB 003.889',
    };
    b.discoverProbe = jest.fn(async () => JSON.stringify(probe));
    const core = createRsCore(b);
    expect(await core.discover?.probe(probe.addr, 5900, 'vnc', 1500)).toEqual(probe);
    expect(b.discoverProbe).toHaveBeenCalledWith(probe.addr, 5900, 'vnc', 1500);
  });

  it('forwards discover.scanLan and preserves a u64 scan id', async () => {
    const b = createFakeBindings();
    b.discoverProbe = jest.fn();
    b.discoverScanLan = jest.fn(async () => 18446744073709551615n);
    const core = createRsCore(b);
    expect(await core.discover?.scanLan([5900, 3389])).toBe('18446744073709551615');
    expect(b.discoverScanLan).toHaveBeenCalledWith([5900, 3389]);
  });

  it('forwards discover.cancelScan as bigint without precision loss', () => {
    const b = createFakeBindings();
    b.discoverProbe = jest.fn();
    b.discoverScanCancel = jest.fn();
    const core = createRsCore(b);
    core.discover?.cancelScan('18446744073709551615');
    expect(b.discoverScanCancel).toHaveBeenCalledWith(18446744073709551615n);
    expect(() => core.discover?.cancelScan('invalid')).toThrow(RsCoreError);
    expect(b.discoverScanCancel).toHaveBeenCalledTimes(1);
  });

  it('omits discover when the native probe binding is absent', () => {
    const b = createFakeBindings();
    b.discoverScanLan = jest.fn();
    expect(createRsCore(b).discover).toBeUndefined();
  });

  it('forwards tailscaleApi calls and parses their JSON results', async () => {
    const b = createFakeBindings();
    const device: TailscaleDevice = {
      id: 'n1',
      name: 'aliyun-bj',
      dnsName: 'aliyun-bj.example.ts.net',
      os: 'linux',
      addresses: ['100.64.0.2', 'fd7a:115c:a1e0::2'],
      online: true,
      lastSeenAt: null,
      tags: [],
    };
    const status: TailscaleApiStatus = {
      configured: true,
      controlUrl: null,
      lastError: null,
      lastOkAt: 1,
      deviceCount: 1,
    };
    b.tailscaleApiSetToken = jest.fn(async () => JSON.stringify([device]));
    b.tailscaleApiClearToken = jest.fn(async () => undefined);
    b.tailscaleApiStatus = jest.fn(async () => JSON.stringify(status));
    b.tailscaleApiDevices = jest.fn(async () => JSON.stringify([device]));
    const core = createRsCore(b);
    expect(await core.tailscaleApi?.setToken('tskey-api-x', null)).toEqual([
      device,
    ]);
    expect(b.tailscaleApiSetToken).toHaveBeenCalledWith(
      'tskey-api-x',
      undefined,
    );
    expect(await core.tailscaleApi?.status()).toEqual(status);
    expect(await core.tailscaleApi?.devices(true)).toEqual([device]);
    expect(b.tailscaleApiDevices).toHaveBeenCalledWith(true);
    await core.tailscaleApi?.clearToken();
    expect(b.tailscaleApiClearToken).toHaveBeenCalledTimes(1);
  });

  it('passes the Headscale control URL through to the binding', async () => {
    const b = createFakeBindings();
    const status: TailscaleApiStatus = {
      configured: true,
      controlUrl: 'https://hs.example.com',
      lastError: null,
      lastOkAt: 1,
      deviceCount: 0,
    };
    b.tailscaleApiSetToken = jest.fn(async () => '[]');
    b.tailscaleApiStatus = jest.fn(async () => JSON.stringify(status));
    b.tailscaleApiDevices = jest.fn(async () => '[]');
    const core = createRsCore(b);

    expect(
      await core.tailscaleApi?.setToken('hskey-x', 'https://hs.example.com'),
    ).toEqual([]);
    expect(b.tailscaleApiSetToken).toHaveBeenLastCalledWith(
      'hskey-x',
      'https://hs.example.com',
    );
    await core.tailscaleApi?.setToken('hskey-x', '');
    expect(b.tailscaleApiSetToken).toHaveBeenLastCalledWith('hskey-x', '');
    expect(await core.tailscaleApi?.status()).toEqual(status);
  });

  it('maps a bad control URL to an internal error', async () => {
    const b = createFakeBindings();
    b.tailscaleApiSetToken = jest.fn(async () => {
      throw b.coreError('Internal', { message: 'bad control url' });
    });
    b.tailscaleApiDevices = jest.fn(async () => '[]');
    const core = createRsCore(b);
    await expect(
      core.tailscaleApi?.setToken('hskey-x', 'http://hs.example.com'),
    ).rejects.toMatchObject({ kind: 'internal', message: 'bad control url' });
  });

  it('omits tailscaleApi when the native devices binding is absent', () => {
    const b = createFakeBindings();
    b.tailscaleApiStatus = jest.fn();
    expect(createRsCore(b).tailscaleApi).toBeUndefined();
  });

  it('normalizes discovery errors and rejects malformed probe JSON', async () => {
    const b = createFakeBindings();
    b.discoverProbe = jest.fn(async () => 'invalid json');
    const core = createRsCore(b);
    await expect(core.discover?.probe('192.168.31.20', 3389, 'rdp', 800)).rejects.toMatchObject({
      kind: 'internal',
      message: 'bad json from core: discover.probe',
    });
    b.discoverProbe = jest.fn(async () => {
      throw b.coreError('Unsupported', { message: 'discover: not built yet' });
    });
    await expect(core.discover?.probe('192.168.31.20', 3389, 'rdp', 800)).rejects.toMatchObject({
      kind: 'unsupported',
    });
    await expect(core.discover?.scanLan([3389])).rejects.toMatchObject({ kind: 'unsupported' });
    expect(() => core.discover?.cancelScan('1')).toThrow(RsCoreError);
  });

  it('encodes parameters and converts native IDs without precision loss', async () => {
    const b = createFakeBindings();
    const core = createRsCore(b);
    await core.init(cfg);
    expect(await core.sessions.open('h1', { intent: 'view' })).toBe('1');
    core.sessions.selectDisplay('1', 2);
    core.sessions.action('1', 'lock');
    core.settings.set('quality.default', 'clear');
    core.settings.set('optional', undefined);
    await core.ssh.open('s1');
    await core.ssh.open('s1', { tmux: true });
    await core.logs.export({ mask: true });
    await core.ssh.installKey('s1', 'device');
    await core.pairing.start({ addr: '100.64.0.7', code: '123456' });
    core.sessions.close('18446744073709551615');
    await core.diag.probeAddress('100.64.0.7');
    expect(b.calls).toEqual([
      { fn: 'coreInit', args: [JSON.stringify(cfg)] },
      { fn: 'sessionsOpen', args: ['h1', '{"intent":"view"}'] },
      { fn: 'sessionsSelectDisplay', args: [1n, 2] },
      { fn: 'sessionsAction', args: [1n, 'lock', undefined] },
      { fn: 'settingsSet', args: ['quality.default', '"clear"'] },
      { fn: 'settingsSet', args: ['optional', 'null'] },
      { fn: 'sshOpen', args: ['s1', undefined] },
      { fn: 'sshOpen', args: ['s1', '{"tmux":true}'] },
      { fn: 'logsExport', args: [true] },
      { fn: 'sshInstallKey', args: ['s1', 'device'] },
      { fn: 'pairingStart', args: ['{"addr":"100.64.0.7","code":"123456"}'] },
      { fn: 'sessionsClose', args: [18446744073709551615n] },
      { fn: 'diagProbeAddress', args: ['100.64.0.7', undefined] },
    ]);
    b.respond('diagProbe', 18446744073709551615n);
    expect(await core.diag.probe('h1')).toBe('18446744073709551615');
  });

  it('exposes optional adapter bindings and preserves u64 IDs', async () => {
    const legacy = createFakeBindings();
    const legacyCore = createRsCore(legacy);
    expect(legacyCore.adapters?.available()).toBe(false);
    await expect(
      legacyCore.adapters?.open(
        { protocol: 'vnc', addr: '100.64.0.7' },
        { viewWidth: 100, viewHeight: 100, scale: 1 },
      ),
    ).rejects.toMatchObject({ kind: 'unsupported' });

    const b = createFakeBindings();
    b.adaptersOpen = async () => 4611686018427387905n;
    const core = createRsCore(b);
    expect(core.adapters?.available()).toBe(true);
    await expect(
      core.adapters?.open(
        { protocol: 'vnc', addr: '100.64.0.7' },
        { viewWidth: 100, viewHeight: 100, scale: 1 },
      ),
    ).resolves.toBe('4611686018427387905');

    let missing: unknown;
    try {
      legacyCore.adapters?.setMonitors('4611686018427387905', 2);
    } catch (error) {
      missing = error;
    }
    expect(missing).toMatchObject({ kind: 'unsupported' });
    const setMonitors = jest.fn();
    b.adaptersSetMonitors = setMonitors;
    core.adapters?.setMonitors('4611686018427387905', 2);
    expect(setMonitors).toHaveBeenCalledWith(4611686018427387905n, 2);
  });

  it('decodes Rust JSON using the contract fixtures', async () => {
    const b = createFakeBindings();
    const core = createRsCore(b);
    const cases: Array<[string, keyof RsMobileBindings, unknown[], unknown]> = [
      [
        'identity.ensure',
        'identityEnsure',
        ['phone'],
        fixtures.fixtureIdentity,
      ],
      ['hosts.paired', 'hostsPaired', [], fixtures.pairedHosts],
      ['pairing.parseQr', 'pairingParseQr', ['qr'], fixtures.pairTargetStudio],
      [
        'pairing.start',
        'pairingStart',
        [fixtures.pairTargetStudio],
        { status: 'ok', hostId: 'h1', hostName: 'host' },
      ],
      ['sessions.list', 'sessionsList', [], [fixtures.liveSession]],
      ['ssh.servers', 'sshServers', [], fixtures.sshServers],
      ['ssh.keys', 'sshKeys', [], fixtures.sshKeys],
      [
        'ssh.createKey',
        'sshCreateKey',
        ['se-p256', 'device'],
        fixtures.sshKeys[0],
      ],
      [
        'ssh.trustedHostKeys',
        'sshTrustedHostKeys',
        [],
        fixtures.trustedHostKeys,
      ],
      ['network.status', 'networkStatus', [], fixtures.netTailnetOk],
    ];
    for (const [path, fn, args, result] of cases) {
      b.respond(fn, JSON.stringify(result));
      expect(await invoke(core, path, args)).toEqual(result);
      b.respond(fn, 'not json');
      await expect(
        Promise.resolve().then(() => invoke(core, path, args)),
      ).rejects.toMatchObject({
        kind: 'internal',
        message: `bad json from core: ${path}`,
      });
    }
    expect(core.pairing.parseQr('bad')).toBeNull();
    expect(core.sessions.list()).toEqual([]);
    b.respond('logsExport', 'export.zip');
    await expect(core.logs.export({ mask: true })).resolves.toBe('export.zip');
  });

  it('normalizes async conflicts, synchronous errors, and invalid IDs', async () => {
    const b = createFakeBindings();
    const core = createRsCore(b);
    b.respond(
      'sshTrustHostKey',
      b.coreError('Conflict', {
        message: 'c',
        currentJson: '{"version":3,"value":[1]}',
      }),
    );
    await expect(core.ssh.trustHostKey('s1', 'SHA256:x')).rejects.toMatchObject({
      kind: 'conflict',
      current: { version: 3, value: [1] },
    });
    b.respond(
      'sessionsClose',
      b.coreError('Internal', { message: 'close failed' }),
    );
    expect(() => core.sessions.close('1')).toThrow(RsCoreError);
    const before = b.calls.length;
    expect(() => core.sessions.close('abc')).toThrow(RsCoreError);
    expect(() => core.ssh.close('-1')).toThrow(RsCoreError);
    expect(b.calls).toHaveLength(before);
    const cyclic: { self?: unknown } = {};
    cyclic.self = cyclic;
    expect(() => core.settings.set('cycle', cyclic)).toThrow(RsCoreError);
    await expect(
      core.pairing.start(cyclic as unknown as { fileId: string }),
    ).rejects.toBeInstanceOf(RsCoreError);
    expect(b.calls).toHaveLength(before);
  });

  it.each(methods)(
    'normalizes binding errors for %s',
    async (path, fn, args) => {
      const b = createFakeBindings();
      const core = createRsCore(b);
      b.respond(fn, b.coreError('Unsupported', { message: 'unavailable' }));
      await expect(
        Promise.resolve().then(() => invoke(core, path, args)),
      ).rejects.toMatchObject({
        kind: 'unsupported',
        message: 'unavailable',
      });
      expect(b.calls).toEqual([{ fn, args: expect.any(Array) }]);
    },
  );

  it.each([
    [
      'files.list',
      'filesList',
      ['h1', null],
      ['h1', undefined],
      JSON.stringify(fileListings.files),
      fileListings.files,
    ],
    [
      'files.list explicit path',
      'filesList',
      ['h1', '/Users/demo/Desktop'],
      ['h1', '/Users/demo/Desktop'],
      JSON.stringify(fileListings.files),
      fileListings.files,
    ],
    [
      'files.download',
      'filesDownload',
      ['h1', '/Users/demo/a.bin'],
      ['h1', '/Users/demo/a.bin'],
      't-download',
      't-download',
    ],
    [
      'files.upload',
      'filesUpload',
      ['h1', '/tmp/a.bin', null],
      ['h1', '/tmp/a.bin', undefined],
      't-upload',
      't-upload',
    ],
    [
      'sftp.list',
      'sftpList',
      ['server-1', null],
      ['server-1', undefined],
      JSON.stringify(fileListings.sftp),
      fileListings.sftp,
    ],
    [
      'sftp.download',
      'sftpDownload',
      ['server-1', '/home/demo/a.bin'],
      ['server-1', '/home/demo/a.bin'],
      't-sftp-download',
      't-sftp-download',
    ],
    [
      'sftp.upload',
      'sftpUpload',
      ['server-1', '/tmp/a.bin', '/home/demo'],
      ['server-1', '/tmp/a.bin', '/home/demo'],
      't-sftp-upload',
      't-sftp-upload',
    ],
    [
      'transfers.list',
      'transfersList',
      [],
      [],
      JSON.stringify([transfer]),
      [transfer],
    ],
    ['transfers.pause', 'transfersPause', ['t1'], ['t1'], undefined, undefined],
    [
      'transfers.resume',
      'transfersResume',
      ['t1'],
      ['t1'],
      undefined,
      undefined,
    ],
    [
      'transfers.cancel',
      'transfersCancel',
      ['t1'],
      ['t1'],
      undefined,
      undefined,
    ],
    [
      'transfers.remove',
      'transfersRemove',
      ['t1'],
      ['t1'],
      undefined,
      undefined,
    ],
  ] as const)(
    'forwards %s to its binding with parsed results',
    async (label, fn, args, bindingArgs, response, expected) => {
      const b = createFakeBindings();
      const core = createRsCore(b);
      b.respond(fn, response);
      const before = b.calls.length;
      const path = label.replace(' explicit path', '');
      const result = await invoke(core, path, args);
      expect(result).toEqual(expected);
      expect(b.calls.slice(before)).toEqual([{ fn, args: bindingArgs }]);
    },
  );

  it('throws synchronously for transfer operations when bindings are missing', () => {
    const b = createFakeBindings();
    const core = createRsCore(b);
    const bindings = b as Partial<RsMobileBindings>;
    delete bindings.filesList;
    delete bindings.transfersList;
    delete bindings.transfersPause;
    delete bindings.transfersCancel;
    delete bindings.transfersRemove;

    expect(() => core.transfers.pause('t1')).toThrow(
      new RsCoreError('unsupported', 'transfers.pause: binding missing'),
    );
    expect(() => core.transfers.list()).toThrow(
      new RsCoreError('unsupported', 'transfers.list: binding missing'),
    );
    expect(() => core.transfers.cancel('t1')).toThrow(
      new RsCoreError('unsupported', 'transfers.cancel: binding missing'),
    );
    expect(() => core.transfers.remove('t1')).toThrow(
      new RsCoreError('unsupported', 'transfers.remove: binding missing'),
    );
    expect(b.calls).toHaveLength(0);
  });

  it('rejects files.list asynchronously when the binding is missing', async () => {
    const b = createFakeBindings();
    delete (b as Partial<RsMobileBindings>).filesList;
    const core = createRsCore(b);

    await expect(core.files.list('h1', null)).rejects.toMatchObject({
      kind: 'unsupported',
      message: 'files.list: binding missing',
    });
    expect(b.calls).toHaveLength(0);
  });

  it('returns rejected promises for missing async file bindings', async () => {
    const b = createFakeBindings();
    const bindings = b as Partial<RsMobileBindings>;
    delete bindings.filesDownload;
    delete bindings.filesUpload;
    delete bindings.sftpList;
    delete bindings.sftpDownload;
    delete bindings.sftpUpload;
    delete bindings.transfersResume;
    const core = createRsCore(b);

    await expect(core.files.download('h1', '/a.bin')).rejects.toMatchObject({
      kind: 'unsupported',
      message: 'files.download: binding missing',
    });
    await expect(
      core.files.upload('h1', '/tmp/a.bin', null),
    ).rejects.toMatchObject({
      kind: 'unsupported',
      message: 'files.upload: binding missing',
    });
    await expect(core.sftp.list('s1', null)).rejects.toMatchObject({
      kind: 'unsupported',
      message: 'sftp.list: binding missing',
    });
    await expect(core.sftp.download('s1', '/a.bin')).rejects.toMatchObject({
      kind: 'unsupported',
      message: 'sftp.download: binding missing',
    });
    await expect(
      core.sftp.upload('s1', '/tmp/a.bin', null),
    ).rejects.toMatchObject({
      kind: 'unsupported',
      message: 'sftp.upload: binding missing',
    });
    await expect(core.transfers.resume('t1')).rejects.toMatchObject({
      kind: 'unsupported',
      message: 'transfers.resume: binding missing',
    });
    expect(b.calls).toHaveLength(0);
  });

  it('maps a CoreError Http 404 to an RsCoreError with status', async () => {
    const b = createFakeBindings();
    const core = createRsCore(b);
    b.respond(
      'filesList',
      b.coreError('Http', {
        status: 404,
        message: 'files.notFound: x',
      }),
    );

    await expect(core.files.list('h1', null)).rejects.toMatchObject({
      kind: 'http',
      message: 'files.notFound: x',
      httpStatus: 404,
    });
  });

  it('reports tailnet unavailable when the binding is missing', async () => {
    const b = createFakeBindings();
    const bindings = b as Partial<RsMobileBindings>;
    delete bindings.tailnetAvailable;
    const core = createRsCore(b);

    await expect(core.tailnet.available()).resolves.toBe(false);
    expect(b.calls).toEqual([]);
  });

  it('rejects tailnet status as unsupported when the binding is missing', async () => {
    const b = createFakeBindings();
    const bindings = b as Partial<RsMobileBindings>;
    delete bindings.tailnetStatus;
    const core = createRsCore(b);

    await expect(core.tailnet.status()).rejects.toEqual(
      new RsCoreError('unsupported', 'tailnet.status: binding missing'),
    );
  });

  it('rejects tailnet login as unsupported when the binding is missing', async () => {
    const b = createFakeBindings();
    const bindings = b as Partial<RsMobileBindings>;
    delete bindings.tailnetLogin;
    const core = createRsCore(b);

    await expect(core.tailnet.login()).rejects.toEqual(
      new RsCoreError('unsupported', 'tailnet.login: binding missing'),
    );
  });

  it('parses tailnet status JSON when the binding is present', async () => {
    const b = createFakeBindings();
    b.respond('tailnetStatus', JSON.stringify(fixtures.tailnetRunning));
    const core = createRsCore(b);

    await expect(core.tailnet.status()).resolves.toEqual(
      fixtures.tailnetRunning,
    );
    expect(b.calls).toEqual([{ fn: 'tailnetStatus', args: [] }]);
  });

  it('forwards all 67 API methods and covers every binding except coreShutdown', async () => {
    const b = createFakeBindings();
    const core = createRsCore(b);
    expect(methods).toHaveLength(67);
    expect(methods.map(([path]) => path).sort()).toEqual(paths(core).sort());
    expect(methods.map(([path]) => path)).toEqual(
      expect.arrayContaining([...GAP_METHODS]),
    );
    let unsubscribe: Unsubscribe | undefined;
    for (const [path, fn, args] of methods) {
      const before = b.calls.length;
      const result = await invoke(core, path, args);
      expect(b.calls.slice(before)).toEqual([{ fn, args: expect.any(Array) }]);
      if (path === 'events.subscribe') {
        unsubscribe = result as Unsubscribe;
      }
    }
    expect(unsubscribe).toEqual(expect.any(Function));
    unsubscribe?.();
    const bindingNames = Object.keys(b).filter(
      key =>
        ![
          'calls',
          'respond',
          'emit',
          'subscriberCount',
          'coreError',
          'coreShutdown',
        ].includes(key),
    );
    expect([...new Set(b.calls.map(entry => entry.fn))].sort()).toEqual(
      bindingNames.sort(),
    );
    expect(b.subscriberCount()).toBe(0);
  });

  it('crash.recordJs sends JSON and returns the native result', () => {
    const b = createFakeBindings();
    const core = createRsCore(b);
    const report = {
      name: 'Error',
      message: 'x',
      stack: null,
      isFatal: true,
      osVersion: '17.4',
    };

    expect(core.crash?.recordJs(report)).toBe(true);
    expect(b.calls.at(-1)).toEqual({
      fn: 'crashRecordJs',
      args: [JSON.stringify(report)],
    });

    b.respond('crashRecordJs', false);
    expect(core.crash?.recordJs(report)).toBe(false);
  });

  it('crash bindings missing are unsupported', async () => {
    const b = createFakeBindings();
    delete b.crashRecordJs;
    const core = createRsCore(b);

    expect(() =>
      core.crash?.recordJs({
        name: null,
        message: 'x',
        stack: null,
        isFatal: true,
        osVersion: null,
      }),
    ).toThrow(
      new RsCoreError('unsupported', 'crash.recordJs: binding missing'),
    );
  });

  it('reports setAudio as unsupported without the generated binding', () => {
    const bindings = createFakeBindings();
    delete bindings.sessionsSetAudio;
    const core = createRsCore(bindings);
    expect(() => core.sessions.setAudio('1', true)).toThrow(
      new RsCoreError('unsupported', 'sessions.setAudio'),
    );
    expect(bindings.calls).toHaveLength(0);
  });

  it('passes setAudio a native session id and the enabled flag', () => {
    const bindings = createFakeBindings();
    const core = createRsCore(bindings);
    core.sessions.setAudio('18446744073709551615', false);
    expect(bindings.calls).toEqual([
      { fn: 'sessionsSetAudio', args: [18446744073709551615n, false] },
    ]);
  });

  it('serializes pairing file arguments', async () => {
    const b = createFakeBindings();
    const core = createRsCore(b);
    const outcome = { status: 'locked', retryAfterS: 873 } as const;
    b.respond('pairingLoadFile', JSON.stringify(fixtures.pairFileSummary));
    b.respond('pairingStart', JSON.stringify(outcome));

    await expect(core.pairing.loadFile({ text: 'pair file' })).resolves.toEqual(
      fixtures.pairFileSummary,
    );
    await expect(core.pairing.start({ fileId: 'file-1' })).resolves.toEqual(
      outcome,
    );

    expect(b.calls).toEqual([
      { fn: 'pairingLoadFile', args: ['{"text":"pair file"}'] },
      { fn: 'pairingStart', args: ['{"fileId":"file-1"}'] },
    ]);
  });

});

describe('shared native event subscription', () => {
  const event = {
    type: 'session.state',
    sessionId: '1',
    hostId: 'h',
    phase: 'streaming',
  } as const;

  afterEach(() => jest.restoreAllMocks());

  it('dispatches in order, isolates callback errors, and unsubscribes idempotently', () => {
    const b = createFakeBindings();
    const core = createRsCore(b);
    const warn = jest
      .spyOn(console, 'warn')
      .mockImplementation(() => undefined);
    const received: CoreEvent[] = [];
    const order: number[] = [];
    const first = jest.fn(() => {
      order.push(1);
      throw new Error('callback');
    });
    const second = jest.fn((value: CoreEvent) => {
      order.push(2);
      received.push(value);
    });
    const off1 = core.events.subscribe(first);
    const off2 = core.events.subscribe(second);
    expect(
      b.calls.filter(entry => entry.fn === 'eventsSubscribe'),
    ).toHaveLength(1);
    expect(b.subscriberCount()).toBe(1);
    b.emit(event);
    expect(first).toHaveBeenCalledWith(event);
    expect(received).toEqual([event]);
    expect(order).toEqual([1, 2]);
    expect(warn).toHaveBeenCalledTimes(1);
    b.emit({ foo: 1 });
    expect(second).toHaveBeenCalledTimes(1);
    off1();
    off1();
    expect(b.subscriberCount()).toBe(1);
    off2();
    off2();
    expect(b.subscriberCount()).toBe(0);
    expect(
      b.calls.filter(entry => entry.fn === 'eventsUnsubscribe'),
    ).toHaveLength(1);
    b.emit(event);
    expect(second).toHaveBeenCalledTimes(1);
    const off3 = core.events.subscribe(second);
    expect(
      b.calls.filter(entry => entry.fn === 'eventsSubscribe'),
    ).toHaveLength(2);
    b.emit(event);
    expect(second).toHaveBeenCalledTimes(2);
    off3();
  });

  it('treats repeated registrations of the same callback independently', () => {
    const b = createFakeBindings();
    const core = createRsCore(b);
    const cb = jest.fn();
    const off1 = core.events.subscribe(cb);
    const off2 = core.events.subscribe(cb);
    b.emit(event);
    expect(cb).toHaveBeenCalledTimes(2);
    off1();
    b.emit(event);
    expect(cb).toHaveBeenCalledTimes(3);
    off2();
    expect(b.subscriberCount()).toBe(0);
  });

  it('drops malformed events and warns on sequence gaps without resubscribing', () => {
    const b = createFakeBindings();
    const core = createRsCore(b);
    const cb = jest.fn();
    const warn = jest
      .spyOn(console, 'warn')
      .mockImplementation(() => undefined);
    const off = core.events.subscribe(cb);
    const sink = b.calls[0]?.args[0] as EventSinkLike;
    ['not json', 'null', '[]', '1', '{"type":1}', '{"foo":1}'].forEach(
      (json, index) => {
        sink.onEvent(BigInt(index + 1), json);
      },
    );
    expect(cb).not.toHaveBeenCalled();
    sink.onEvent(8n, JSON.stringify(event));
    expect(cb).toHaveBeenCalledWith(event);
    expect(warn).toHaveBeenCalledTimes(1);
    expect(b.calls).toHaveLength(1);
    off();
    const off2 = core.events.subscribe(cb);
    const nextSink = b.calls[2]?.args[0] as EventSinkLike;
    nextSink.onEvent(100n, JSON.stringify(event));
    sink.onEvent(9n, JSON.stringify(event));
    expect(cb).toHaveBeenCalledTimes(2);
    expect(warn).toHaveBeenCalledTimes(1);
    off2();
  });

  it('recovers after a failed native subscribe and normalizes unsubscribe errors', () => {
    const b = createFakeBindings();
    const core = createRsCore(b);
    const cb = jest.fn();
    b.respond(
      'eventsSubscribe',
      b.coreError('Internal', { message: 'subscribe' }),
    );
    expect(() => core.events.subscribe(cb)).toThrow(RsCoreError);
    const off = core.events.subscribe(cb);
    b.emit(event);
    expect(cb).toHaveBeenCalledTimes(1);
    b.respond(
      'eventsUnsubscribe',
      b.coreError('Internal', { message: 'unsubscribe' }),
    );
    expect(off).toThrow(RsCoreError);
    expect(off).not.toThrow();
  });
});

it('reports createVirtualDisplay as unsupported without the generated binding', () => {
  const bindings = createFakeBindings();
  delete bindings.sessionsCreateVirtualDisplay;
  const core = createRsCore(bindings);
  expect(() =>
    core.sessions.createVirtualDisplay('1', { widthPx: 2732, heightPx: 2048 }),
  ).toThrow(new RsCoreError('unsupported', 'sessions.createVirtualDisplay'));
  expect(bindings.calls).toHaveLength(0);
});

it('reports releaseVirtualDisplay as unsupported without the generated binding', () => {
  const bindings = createFakeBindings();
  delete bindings.sessionsReleaseVirtualDisplay;
  const core = createRsCore(bindings);
  expect(() => core.sessions.releaseVirtualDisplay('1', 3)).toThrow(
    new RsCoreError('unsupported', 'sessions.releaseVirtualDisplay'),
  );
  expect(bindings.calls).toHaveLength(0);
});

it('preserves the native session id, virtual display options and request id', () => {
  const bindings = createFakeBindings();
  bindings.respond('sessionsCreateVirtualDisplay', 7);
  const core = createRsCore(bindings);
  const opts = {
    widthPx: 2732,
    heightPx: 2048,
    scale: 2,
    refreshHz: 60,
    select: true,
  };
  expect(core.sessions.createVirtualDisplay('18446744073709551615', opts)).toBe(
    7,
  );
  core.sessions.releaseVirtualDisplay('18446744073709551615', 3);
  expect(bindings.calls).toEqual([
    {
      fn: 'sessionsCreateVirtualDisplay',
      args: [18446744073709551615n, JSON.stringify(opts)],
    },
    { fn: 'sessionsReleaseVirtualDisplay', args: [18446744073709551615n, 3] },
  ]);
});
