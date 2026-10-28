import { FakeCore, createFakeCore } from './fakeCore';
import { GAP_METHODS, RsCoreError } from './RsCoreApi';
import type { RsCoreApi } from './RsCoreApi';
import {
  FIXTURE_NOW,
  MIN_HOST_VERSION,
  SELF_DEVICE_ID,
  computers,
  directory,
  liveSession,
  netCompanyVpn,
  netNoTailnet,
  netTailnetOk,
  pairFileSummary,
  pairFileText,
  pairQrStudio,
  pairTargetStudio,
  pairedHosts,
  routeEmbeddedNeedsLogin,
  routeEmbeddedRunning,
  servers,
  sshKeys,
  sshServers,
  tailnetNeedsLogin,
  tailnetRunning,
  tailnetUnavailable,
  trustedHostKeys,
} from './fixtures';
import type { CoreEvent } from './types';

describe('fixtures', () => {
  it('matches the canvas directory', () => {
    expect(computers).toHaveLength(5);
    expect(servers).toHaveLength(3);
    expect(directory.generatedAt).toBe(1792465134000000);
    expect(directory.others).toEqual([]);
    expect(MIN_HOST_VERSION).toBe('0.3.0');
    expect(
      computers.filter(computer => computer.update.available !== null),
    ).toEqual([
      expect.objectContaining({
        name: '家里台式机',
        update: { channel: 'stable', available: '0.3.1' },
      }),
    ]);
    expect(computers.find(computer => computer.name === '实验室 Mac')).toEqual(
      expect.objectContaining({
        online: true,
        hostRunning: false,
        lastHeartbeatAt: 1792464834000000,
      }),
    );
  });

  it('provides all shape presets used by pages', () => {
    expect(sshServers.map(server => server.serverId)).toEqual([
      'prod-api-01',
      'gpu-train-02',
      'nas-home',
      'manual-build-srv',
    ]);
    expect(sshKeys[0]?.installedOn).toEqual(['gpu-train-02', 'nas-home']);
    expect(trustedHostKeys).toHaveLength(2);
    expect(netTailnetOk.ifaces[0]).toEqual({
      name: 'utun4',
      addrs: ['100.64.0.21'],
      isVpn: true,
    });
    expect(netCompanyVpn.ifaces[0]?.addrs).toEqual(['10.99.4.27']);
    expect(netNoTailnet.ifaces).toEqual([
      { name: 'en0', addrs: ['192.168.31.45'], isVpn: false },
    ]);
    expect(liveSession).toEqual(
      expect.objectContaining({
        id: 's-live-1',
        hostId: 'h7v2m9q3k8x4tb1a',
        phase: 'streaming',
        role: 'controller',
        path: 'tailnetDirect',
        transport: 'quic',
        displayId: 1,
      }),
    );
    expect(pairTargetStudio.hostId).toBe('h7v2m9q3k8x4tb1a');
    expect(pairQrStudio).toContain('c=482913');
    expect(pairFileSummary.addrs).toEqual(['100.64.0.7:7843']);
    expect(pairFileSummary.expired).toBe(false);
  });
});

describe('fake discovery', () => {
  it('defaults to a timeout and supports injected probe responses', async () => {
    const core = createFakeCore();
    expect(await core.discover?.probe('192.168.31.20', 5900, 'vnc', 800)).toEqual({
      addr: '192.168.31.20', port: 5900, outcome: 'timeout', ms: 800,
    });
    if (!core.discover) {
      throw new Error('fake discovery missing');
    }
    core.discover.probe = jest.fn(async (addr, port) => ({
      addr, port, outcome: 'open' as const, ms: 5,
    }));
    expect(await core.discover.probe('192.168.31.20', 5900, 'vnc', 800)).toMatchObject({ outcome: 'open' });
    core.discover = undefined;
    expect(core.discover).toBeUndefined();
  });

  it('returns distinct decimal scan ids and records cancellation', async () => {
    const core = createFakeCore();
    expect(await core.discover?.scanLan([5900])).toBe('1');
    expect(await core.discover?.scanLan([3389])).toBe('2');
    core.discover?.cancelScan('2');
    expect(core.calls).toContainEqual({ method: 'discover.cancelScan', args: ['2'] });
  });
});

describe('FakeCore identity', () => {
  it('fails the next call only, then resumes normally', async () => {
    const core = createFakeCore();
    core.failNext('hosts.paired', new Error('x'));

    await expect(core.hosts.paired()).rejects.toThrow('x');
    await expect(core.hosts.paired()).resolves.toEqual(pairedHosts);
  });

  it('creates an identity once', async () => {
    const core = createFakeCore();

    await expect(core.identity.ensure('iPad')).resolves.toEqual({
      deviceId: SELF_DEVICE_ID,
      fp: 'fake-fp',
    });
    await expect(core.identity.ensure('iPad')).resolves.toEqual({
      deviceId: SELF_DEVICE_ID,
      fp: 'fake-fp',
    });
    expect(core.calls).toEqual([
      { method: 'identity.ensure', args: ['iPad'] },
      { method: 'identity.ensure', args: ['iPad'] },
    ]);
  });

  it('keeps current fixtures when a patch field is undefined', async () => {
    const core = createFakeCore();
    await core.identity.ensure('iPad');
    core.setFixtures({ pairedHosts: undefined });

    await expect(core.identity.ensure('iPad')).resolves.toEqual({
      deviceId: SELF_DEVICE_ID,
      fp: 'fake-fp',
    });
    await expect(core.hosts.paired()).resolves.toHaveLength(
      pairedHosts.length,
    );
  });
});

describe('FakeCore pairing', () => {
  it('parses the studio QR and rejects malformed URLs', () => {
    const core = createFakeCore();
    expect(core.pairing.parseQr(pairQrStudio)).toEqual({
      ...pairTargetStudio,
      addrs: ['100.64.0.7', '192.168.31.20'],
    });
    expect(core.pairing.parseQr('rsp://pair?v=2&h=x')).toBeNull();
    expect(
      core.pairing.parseQr('rsp://pair?v=1&h=x&fp=f&c=12345&n=x&a=a'),
    ).toBeNull();
    expect(core.pairing.parseQr('https://example.com')).toBeNull();
  });

  it('resolves a pending pairing with resolvePairing', async () => {
    const core = createFakeCore();
    const pending = core.pairing.start(pairTargetStudio);
    core.resolvePairing({
      status: 'ok',
      hostId: 'h7v2m9q3k8x4tb1a',
      hostName: 'Studio Mac mini',
    });
    await expect(pending).resolves.toEqual({
      status: 'ok',
      hostId: 'h7v2m9q3k8x4tb1a',
      hostName: 'Studio Mac mini',
    });
  });

  it('cancels a pending pairing', async () => {
    const core = createFakeCore();
    const pending = core.pairing.start({
      addr: '100.64.0.7',
      code: '482913',
    });
    core.pairing.cancel();
    await expect(pending).resolves.toEqual({ status: 'cancelled' });
  });

  it('rejects a failed start instead of throwing synchronously', async () => {
    const core = createFakeCore();
    core.failNext('pairing.start', new Error('pairing unavailable'));

    await expect(core.pairing.start(pairTargetStudio)).rejects.toThrow(
      'pairing unavailable',
    );
  });

  it('loads a pairing file by trimmed text or file URI', async () => {
    const core = createFakeCore();

    await expect(
      core.pairing.loadFile({ text: `  ${pairFileText}\n` }),
    ).resolves.toEqual(pairFileSummary);
    await expect(
      core.pairing.loadFile({ uri: `file://${pairFileText}` }),
    ).rejects.toEqual(new RsCoreError('internal', 'not a pairing file'));
    const uriCore = createFakeCore({
      pairFiles: { [`file://${pairFileText}`]: pairFileSummary },
    });
    await expect(
      uriCore.pairing.loadFile({ uri: `file://${pairFileText}` }),
    ).resolves.toEqual(pairFileSummary);
    await expect(
      core.pairing.loadFile({ text: 'not a pairing file' }),
    ).rejects.toEqual(new RsCoreError('internal', 'not a pairing file'));
    await expect(
      core.pairing.loadFile({ uri: `file://missing` }),
    ).rejects.toEqual(new RsCoreError('internal', 'not a pairing file'));
    await expect(
      core.pairing.loadFile({ uri: 'content://pairing/file' }),
    ).rejects.toMatchObject({ kind: 'unsupported' });
  });
});

describe('FakeCore events and sessions', () => {
  it('subscribes, emits and unsubscribes synchronously', () => {
    const core = createFakeCore();
    const received: CoreEvent[] = [];
    const unsubscribe = core.events.subscribe(event => received.push(event));
    const event: CoreEvent = {
      type: 'session.state',
      sessionId: 's1',
      hostId: 'h7v2m9q3k8x4tb1a',
      phase: 'streaming',
    };

    core.emit(event);
    expect(received).toEqual([event]);
    expect(core.subscriberCount()).toBe(1);
    unsubscribe();
    core.emit(event);
    expect(received).toHaveLength(1);
    expect(core.subscriberCount()).toBe(0);
  });

  it('opens, lists and closes a session', async () => {
    const core = createFakeCore();
    const events: CoreEvent[] = [];
    core.events.subscribe(event => events.push(event));
    const before = core.sessions.list().length;

    await expect(
      core.sessions.open('h7v2m9q3k8x4tb1a', { intent: 'control' }),
    ).resolves.toBe('s1');
    expect(core.sessions.list()).toHaveLength(before + 1);
    expect(core.sessions.list()[before]).toEqual(
      expect.objectContaining({
        id: 's1',
        hostId: 'h7v2m9q3k8x4tb1a',
        phase: 'connecting',
      }),
    );
    core.sessions.close('s1');
    expect(core.sessions.list()).toHaveLength(before);
    expect(events).toContainEqual({
      type: 'session.error',
      sessionId: 's1',
      hostId: 'h7v2m9q3k8x4tb1a',
      reason: 'userClosed',
    });
  });

  it('returns copies for list and fixtures', async () => {
    const core = createFakeCore();
    const first = core.sessions.list();
    first.push({ ...liveSession, id: 'mutated' });
    expect(core.sessions.list().some(item => item.id === 'mutated')).toBe(
      false,
    );

    const hostsResult = await core.hosts.paired();
    hostsResult[0].name = 'changed';
    expect((await core.hosts.paired())[0]?.name).toBe('Studio Mac mini');
  });

  it('records session audio changes', () => {
    const core = createFakeCore();
    core.sessions.setAudio('s1', true);
    core.sessions.setAudio('s1', false);
    expect(core.calls).toEqual([
      { method: 'sessions.setAudio', args: ['s1', true] },
      { method: 'sessions.setAudio', args: ['s1', false] },
    ]);
  });
});

describe('FakeCore SSH, diagnostics and settings', () => {
  it('creates keys and removes trusted host keys', async () => {
    const core = createFakeCore();
    await expect(core.ssh.keys()).resolves.toHaveLength(1);
    const key = await core.ssh.createKey('ed25519', 'new key');
    expect(key).toEqual(
      expect.objectContaining({ name: 'new key', kind: 'ed25519' }),
    );
    expect(await core.ssh.keys()).toHaveLength(2);
    await core.ssh.forgetHostKey('gpu-train-02');
    expect(await core.ssh.trustedHostKeys()).toEqual([trustedHostKeys[1]]);
  });

  it('returns fixture servers, network status and numbered probes', async () => {
    const core = createFakeCore();
    await expect(core.ssh.servers()).resolves.toEqual(sshServers);
    await expect(core.network.status()).resolves.toEqual(netTailnetOk);
    await expect(core.diag.probe('h7v2m9q3k8x4tb1a')).resolves.toBe('p1');
    await expect(core.diag.probe('h7v2m9q3k8x4tb1a')).resolves.toBe('p2');
    await expect(core.diag.probeAddress('100.64.0.7', 7400)).resolves.toBe(
      'p1',
    );
  });

  it('exports logs with and without masking and records settings', async () => {
    const core = createFakeCore();
    await expect(core.logs.export({ mask: true })).resolves.toBe(
      '/tmp/rs-logs-masked.zip',
    );
    await expect(core.logs.export({ mask: false })).resolves.toBe(
      '/tmp/rs-logs.zip',
    );
    core.settings.set('theme', 'dark');
    expect(core.calls).toContainEqual({
      method: 'settings.set',
      args: ['theme', 'dark'],
    });
  });

  it('records method names and arguments', async () => {
    const core = new FakeCore();
    await core.hosts.forget('h7v2m9q3k8x4tb1a');
    expect(core.calls).toContainEqual({
      method: 'hosts.forget',
      args: ['h7v2m9q3k8x4tb1a'],
    });
  });
});

describe('FakeCore diagnostics defaults and settings validation', () => {
  it('records the effective probe port', async () => {
    const core = createFakeCore();

    await expect(core.diag.probeAddress('100.64.0.7')).resolves.toBe('p1');
    expect(core.calls).toContainEqual({
      method: 'diag.probeAddress',
      args: ['100.64.0.7', 7400],
    });

    await expect(core.diag.probeAddress('100.64.0.7', 22)).resolves.toBe('p2');
    expect(core.calls).toContainEqual({
      method: 'diag.probeAddress',
      args: ['100.64.0.7', 22],
    });
  });

  it.each([0, 65536, 1.5])('rejects invalid probe port %s', async port => {
    const core = createFakeCore();
    const failure = await core.diag
      .probeAddress('100.64.0.7', port)
      .catch(error => error);

    expect(failure).toBeInstanceOf(RsCoreError);
    expect(failure.kind).toBe('internal');
    expect(core.calls).toContainEqual({
      method: 'diag.probeAddress',
      args: ['100.64.0.7', port],
    });
  });

  it.each([
    ['quality.default', 'ultra'],
    ['prewarm.enabled', 'yes'],
    ['networks', [{ id: 'x' }]],
  ])('rejects a bad value for %s', (key, value) => {
    const core = createFakeCore();

    expect(() => core.settings.set(key, value)).toThrow(
      new RsCoreError('internal', `bad value for ${key}`),
    );
    expect(core.calls).toContainEqual({
      method: 'settings.set',
      args: [key, value],
    });
  });

  it('accepts supported settings and ignores unknown keys', () => {
    const core = createFakeCore();

    expect(() => core.settings.set('networks', null)).not.toThrow();
    expect(() => core.settings.set('tailnet.mode', 'embedded')).not.toThrow();
    expect(() => core.settings.set('session.hud', true)).not.toThrow();
    expect(core.calls).toEqual(
      expect.arrayContaining([
        { method: 'settings.set', args: ['networks', null] },
        { method: 'settings.set', args: ['tailnet.mode', 'embedded'] },
        { method: 'settings.set', args: ['session.hud', true] },
      ]),
    );
  });
});

describe('FakeCore tailnet', () => {
  it('defaults to unavailable tailnet', async () => {
    const core = createFakeCore();

    await expect(core.tailnet.available()).resolves.toBe(false);
    await expect(core.tailnet.status()).resolves.toEqual(tailnetUnavailable);
  });

  it('emits tailnet.state and network.changed when setTailnet updates fixtures', async () => {
    const core = createFakeCore({ network: netCompanyVpn });
    const events: CoreEvent[] = [];
    core.events.subscribe(event => events.push(event));

    core.setTailnet({
      available: true,
      status: tailnetRunning,
      route: routeEmbeddedRunning,
    });
    expect(events).toEqual([
      {
        type: 'tailnet.state',
        state: 'running',
        authUrl: null,
        ips: ['100.64.0.21', 'fd7a:115c:a1e0::15'],
        derpRegion: 'sha',
        message: null,
      },
      {
        type: 'network.changed',
        status: { ...netCompanyVpn, tailnet: routeEmbeddedRunning },
      },
    ]);
    await expect(core.tailnet.available()).resolves.toBe(true);
    await expect(core.tailnet.status()).resolves.toEqual(tailnetRunning);

    core.setTailnet({ status: tailnetNeedsLogin, route: routeEmbeddedNeedsLogin });
    expect(events.slice(2)).toEqual([
      {
        type: 'tailnet.state',
        state: 'needsLogin',
        authUrl: tailnetNeedsLogin.authUrl,
        ips: [],
        derpRegion: '',
        message: null,
      },
      {
        type: 'network.changed',
        status: { ...netCompanyVpn, tailnet: routeEmbeddedNeedsLogin },
      },
    ]);
    core.setTailnet({ route: null });
    expect(events[4]).toEqual({ type: 'network.changed', status: netCompanyVpn });
    await expect(core.network.status()).resolves.toEqual(netCompanyVpn);
  });

  it('returns tailnet status copies and records available login and logout calls', async () => {
    const core = createFakeCore({ tailnetAvailable: true, tailnet: tailnetRunning });

    const status = await core.tailnet.status();
    status.peers.splice(0);
    await expect(core.tailnet.status()).resolves.toEqual(tailnetRunning);
    await expect(core.tailnet.available()).resolves.toBe(true);
    await expect(core.tailnet.login()).resolves.toBeUndefined();
    await expect(core.tailnet.logout()).resolves.toBeUndefined();
    expect(core.calls).toEqual([
      { method: 'tailnet.status', args: [] },
      { method: 'tailnet.status', args: [] },
      { method: 'tailnet.available', args: [] },
      { method: 'tailnet.login', args: [] },
      { method: 'tailnet.logout', args: [] },
    ]);
  });

  it('rejects tailnet login and logout as unsupported when unavailable', async () => {
    const core = createFakeCore();

    await expect(core.tailnet.login()).rejects.toEqual(
      new RsCoreError('unsupported', 'tailnet.login'),
    );
    await expect(core.tailnet.logout()).rejects.toEqual(
      new RsCoreError('unsupported', 'tailnet.logout'),
    );
    expect(core.calls).toEqual([
      { method: 'tailnet.login', args: [] },
      { method: 'tailnet.logout', args: [] },
    ]);
  });

  it('validates the control URL and app active runtime hint', () => {
    const core = createFakeCore();

    expect(() => core.settings.set('tailnet.controlUrl', null)).not.toThrow();
    expect(() =>
      core.settings.set('tailnet.controlUrl', 'https://hs.example.com'),
    ).not.toThrow();
    expect(() => core.settings.set('tailnet.controlUrl', 'hs.example.com')).toThrow(
      new RsCoreError('internal', 'bad value for tailnet.controlUrl'),
    );
    expect(() => core.settings.set('device.appActive', false)).not.toThrow();
    expect(() => core.settings.set('device.appActive', 'yes')).toThrow(
      new RsCoreError('internal', 'bad value for device.appActive'),
    );
  });
});

describe('FakeCore tailscaleApi', () => {
  const device = {
    id: 'n1',
    name: 'mac-a',
    dnsName: 'mac-a.example.ts.net',
    os: 'macos' as const,
    addresses: ['100.64.0.3'],
    online: true,
    lastSeenAt: null,
    tags: [],
  };

  it('needs a token before listing devices', async () => {
    const core = new FakeCore();
    core.setTailscale({ devices: [device] });
    await expect(core.tailscaleApi?.devices(false)).rejects.toMatchObject({
      kind: 'unsupported',
    });
    expect(await core.tailscaleApi?.status()).toMatchObject({ configured: false });
  });

  it('saves a token, lists devices and clears it', async () => {
    const core = new FakeCore();
    core.setTailscale({ devices: [device] });
    expect(await core.tailscaleApi?.setToken('tskey-api-x', null)).toEqual([
      device,
    ]);
    expect(await core.tailscaleApi?.devices(true)).toEqual([device]);
    expect(await core.tailscaleApi?.status()).toEqual({
      configured: true,
      controlUrl: null,
      lastError: null,
      lastOkAt: FIXTURE_NOW,
      deviceCount: 1,
    });
    await core.tailscaleApi?.clearToken();
    expect(await core.tailscaleApi?.status()).toMatchObject({ configured: false });
    await expect(core.tailscaleApi?.devices(false)).rejects.toBeInstanceOf(RsCoreError);
  });

  it('does not save the token when validation fails', async () => {
    const core = new FakeCore();
    core.failNext('tailscaleApi.setToken', new RsCoreError('http', 'unauthorized', 401));
    await expect(core.tailscaleApi?.setToken('bad', null)).rejects.toMatchObject({
      kind: 'http',
    });
    expect(await core.tailscaleApi?.status()).toMatchObject({ configured: false });
  });

  it('saves a Headscale control URL with the token and clears it', async () => {
    const core = new FakeCore();
    core.setTailscale({ devices: [{ ...device, os: 'unknown', dnsName: '' }] });
    await core.tailscaleApi?.setToken('hskey-x', ' https://HS.example.com:8443/ ');
    expect(core.calls.at(-1)).toEqual({
      method: 'tailscaleApi.setToken',
      args: ['hskey-x', ' https://HS.example.com:8443/ '],
    });
    expect(await core.tailscaleApi?.status()).toMatchObject({
      configured: true,
      controlUrl: 'https://hs.example.com:8443',
    });
    await core.tailscaleApi?.devices(true);
    expect(await core.tailscaleApi?.status()).toMatchObject({
      controlUrl: 'https://hs.example.com:8443',
    });

    await core.tailscaleApi?.setToken('tskey-api-x', '  ');
    expect(await core.tailscaleApi?.status()).toMatchObject({ controlUrl: null });
    await core.tailscaleApi?.setToken('hskey-x', 'https://hs.example.com');
    await core.tailscaleApi?.clearToken();
    expect(await core.tailscaleApi?.status()).toMatchObject({
      configured: false,
      controlUrl: null,
    });
  });

  it('rejects a bad control URL like Rust and keeps the old token', async () => {
    const core = new FakeCore();
    await core.tailscaleApi?.setToken('hskey-x', 'https://hs.example.com');
    for (const url of [
      'http://hs.example.com',
      'https://hs.example.com/api',
      'https://admin@hs.example.com',
      'https://hs.example.com?x=1',
      'https://',
    ]) {
      await expect(core.tailscaleApi?.setToken('hskey-y', url)).rejects.toMatchObject({
        kind: 'internal',
        message: 'bad control url',
      });
    }
    expect(await core.tailscaleApi?.status()).toMatchObject({
      configured: true,
      controlUrl: 'https://hs.example.com',
    });
  });

  it('seeds a saved control URL', async () => {
    const core = new FakeCore();
    core.setTailscale({ configured: true, controlUrl: 'https://hs.example.com' });
    expect(await core.tailscaleApi?.status()).toMatchObject({
      configured: true,
      controlUrl: 'https://hs.example.com',
    });
  });
});

describe('FakeCore file gaps', () => {
  afterEach(() => {
    jest.clearAllTimers();
    jest.useRealTimers();
  });

  it.each(
    GAP_METHODS.filter(
      method => method.startsWith('files.') || method.startsWith('sftp.'),
    ),
  )('implements %s without returning unsupported', async method => {
    jest.useFakeTimers();
    const core = createFakeCore();
    const hostId = computers[0]?.hostId ?? '';
    const serverId = sshServers[0]?.serverId ?? '';
    const args: Record<string, unknown[]> = {
      'files.list': [hostId, null],
      'files.download': [hostId, '/Users/demo/notes-1020.md'],
      'files.upload': [hostId, '/tmp/report.pdf', null],
      'sftp.list': [serverId, null],
      'sftp.download': [serverId, '/home/demo/app.tar.gz'],
      'sftp.upload': [serverId, '/tmp/report.pdf', null],
    };
    const parts = method.split('.');
    const group = (core as unknown as Record<string, unknown>)[
      parts[0] ?? ''
    ] as Record<string, (...params: unknown[]) => unknown>;
    const fn = group[parts[1] ?? ''];
    const result = fn?.(...(args[method] ?? []));
    if (result instanceof Promise) {
      await expect(result).resolves.not.toThrow();
    }
  });

  it('implements every file API group on the fake', async () => {
    const core = createFakeCore();
    const api: Partial<RsCoreApi> = core;
    expect(api.files).toBeDefined();
    expect(api.sftp).toBeDefined();
    expect(api.transfers).toBeDefined();
    await expect(
      core.files.list(computers[0]?.hostId ?? '', null),
    ).resolves.toHaveProperty('entries');
    await expect(
      core.sftp.list(sshServers[0]?.serverId ?? '', null),
    ).resolves.toHaveProperty('entries');
    expect(core.transfers.list()).toEqual([]);
  });

  it('supports every transfer gap operation without unsupported errors', async () => {
    jest.useFakeTimers();
    const core = createFakeCore();
    const id = await core.files.download(
      computers[0]?.hostId ?? '',
      '/Users/demo/notes-1020.md',
    );
    jest.advanceTimersByTime(100);
    await Promise.resolve();
    expect(() => core.transfers.pause(id)).not.toThrow();
    await expect(core.transfers.resume(id)).resolves.toBeUndefined();
    expect(core.transfers.list()).toHaveLength(1);
    expect(() => core.transfers.cancel(id)).not.toThrow();
    expect(() => core.transfers.remove(id)).not.toThrow();
  });
});
