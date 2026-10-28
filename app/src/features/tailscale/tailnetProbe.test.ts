import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import type {
  DiscoverKind,
  DiscoverOutcome,
  TailscaleDevice,
} from '../../native/rscore/types';
import { MemoryKv } from '../storage/kv';
import {
  PROBE_TIMEOUT_MS,
  PROBE_TTL_MS,
  PROBE_WAIT_MS,
  TAILNET_PROBE_KEY,
  classifyPorts,
  probeTailnet,
  probeTargets,
  readTailnetProbes,
  tailnetProbesWithin,
} from './tailnetProbe';

const MINUTE_MS = 60_000;

function device(overrides: Partial<TailscaleDevice>): TailscaleDevice {
  return {
    id: '1',
    name: 'node',
    dnsName: '',
    os: 'unknown',
    addresses: [],
    online: true,
    lastSeenAt: null,
    tags: [],
    ...overrides,
  };
}

const mac = device({ id: '1', name: 'mac', addresses: ['100.64.0.1'] });
const pc = device({ id: '2', name: 'pc', addresses: ['100.64.0.2'] });
const box = device({ id: '3', name: 'box', addresses: ['100.64.0.3'] });

/** `open` holds "addr:port" pairs; every other probe times out. */
function withPorts(core: FakeCore, open: string[], delayMs = 0) {
  const probe = jest.fn(
    async (addr: string, port: number, _kind: DiscoverKind, ms: number) => {
      if (delayMs > 0) {
        await new Promise(resolve => setTimeout(resolve, delayMs));
      }
      const outcome: DiscoverOutcome = open.includes(`${addr}:${port}`)
        ? 'open'
        : 'timeout';
      return { addr, port, outcome, ms };
    },
  );
  if (core.discover === undefined) {
    throw new Error('fake discovery missing');
  }
  core.discover.probe = probe;
  return probe;
}

function clock(start = 1_000_000) {
  let value = start;
  return {
    now: () => value,
    advance(ms: number) {
      value += ms;
    },
  };
}

describe('probeTargets', () => {
  it('takes devices without a system at their first IPv4, unless offline', () => {
    expect(
      probeTargets([
        device({ id: '1', addresses: ['fd7a::1', '100.64.0.1', '100.64.0.9'] }),
        device({ id: '2', addresses: ['100.64.0.2'], online: null }),
        device({ id: '3', addresses: ['100.64.0.3'], online: false }),
        device({ id: '4', addresses: ['fd7a::4'] }),
        device({ id: '5', addresses: ['100.64.0.5'], os: 'linux' }),
      ]),
    ).toEqual([
      { id: '1', addr: '100.64.0.1' },
      { id: '2', addr: '100.64.0.2' },
    ]);
  });
});

describe('classifyPorts', () => {
  it('prefers 3389 over 5900 and adds 22 as SSH', () => {
    // A Mac never listens on 3389: both open is a Windows PC with a VNC server.
    expect(classifyPorts({ vnc: true, rdp: true, ssh: false })).toEqual({
      computer: 'windows',
      ssh: false,
    });
    expect(classifyPorts({ vnc: true, rdp: false, ssh: false })).toEqual({
      computer: 'macos',
      ssh: false,
    });
    expect(classifyPorts({ vnc: false, rdp: true, ssh: true })).toEqual({
      computer: 'windows',
      ssh: true,
    });
    expect(classifyPorts({ vnc: false, rdp: false, ssh: true })).toEqual({
      computer: null,
      ssh: true,
    });
  });
});

describe('probeTailnet', () => {
  it('probes 5900, 3389 and 22 at the first IPv4 and classifies each device', async () => {
    const core = createFakeCore();
    const probe = withPorts(core, [
      '100.64.0.1:5900',
      '100.64.0.2:3389',
      '100.64.0.3:22',
    ]);
    const kv = new MemoryKv();

    const probes = await probeTailnet(core, kv, probeTargets([mac, pc, box]));

    expect(probes).toEqual({
      '1': { computer: 'macos', ssh: false },
      '2': { computer: 'windows', ssh: false },
      '3': { computer: null, ssh: true },
    });
    expect(probe).toHaveBeenCalledTimes(9);
    expect(probe.mock.calls.filter(call => call[0] === '100.64.0.1')).toEqual([
      ['100.64.0.1', 5900, 'vnc', PROBE_TIMEOUT_MS],
      ['100.64.0.1', 3389, 'rdp', PROBE_TIMEOUT_MS],
      ['100.64.0.1', 22, 'tcp', PROBE_TIMEOUT_MS],
    ]);
    expect(readTailnetProbes(kv, probeTargets([mac, pc, box]))).toEqual(probes);
  });

  it('keeps a Mac with SSH as both', async () => {
    const core = createFakeCore();
    withPorts(core, ['100.64.0.1:5900', '100.64.0.1:22']);
    const probes = await probeTailnet(core, new MemoryKv(), probeTargets([mac]));
    expect(probes).toEqual({ '1': { computer: 'macos', ssh: true } });
  });

  it('counts mismatches and failed calls as closed', async () => {
    const core = createFakeCore();
    if (core.discover === undefined) {
      throw new Error('fake discovery missing');
    }
    core.discover.probe = jest.fn(async (addr, port, _kind, ms) => {
      if (port === 22) {
        throw new Error('boom');
      }
      return { addr, port, outcome: 'mismatch' as const, ms };
    });
    const probes = await probeTailnet(core, new MemoryKv(), probeTargets([mac]));
    expect(probes).toEqual({ '1': { computer: null, ssh: false } });
  });

  it('never probes offline devices', async () => {
    const core = createFakeCore();
    const probe = withPorts(core, ['100.64.0.1:5900']);
    const probes = await probeTailnet(
      core,
      new MemoryKv(),
      probeTargets([{ ...mac, online: false }]),
    );
    expect(probes).toEqual({});
    expect(probe).not.toHaveBeenCalled();
  });

  it('reuses results for 10 minutes unless forced', async () => {
    const core = createFakeCore();
    const probe = withPorts(core, ['100.64.0.1:5900']);
    const kv = new MemoryKv();
    const time = clock();
    const targets = probeTargets([mac]);

    await probeTailnet(core, kv, targets, { now: time.now });
    expect(probe).toHaveBeenCalledTimes(3);

    time.advance(PROBE_TTL_MS - 1);
    await probeTailnet(core, kv, targets, { now: time.now });
    expect(probe).toHaveBeenCalledTimes(3);

    await probeTailnet(core, kv, targets, { now: time.now, force: true });
    expect(probe).toHaveBeenCalledTimes(6);

    // The forced round restarted the 10 minutes.
    time.advance(PROBE_TTL_MS - 1);
    await probeTailnet(core, kv, targets, { now: time.now });
    expect(probe).toHaveBeenCalledTimes(6);
    time.advance(1);
    await probeTailnet(core, kv, targets, { now: time.now });
    expect(probe).toHaveBeenCalledTimes(9);
  });

  it('probes again when the address changes and drops removed devices', async () => {
    const core = createFakeCore();
    const probe = withPorts(core, ['100.64.0.1:5900', '100.64.0.7:3389']);
    const kv = new MemoryKv();
    await probeTailnet(core, kv, probeTargets([mac, pc]));
    expect(probe).toHaveBeenCalledTimes(6);

    const moved = { ...mac, addresses: ['100.64.0.7'] };
    expect(readTailnetProbes(kv, probeTargets([moved]))).toEqual({});
    const probes = await probeTailnet(core, kv, probeTargets([moved]));
    expect(probe).toHaveBeenCalledTimes(9);
    expect(probes).toEqual({ '1': { computer: 'windows', ssh: false } });
    const saved = kv.getJSON<Record<string, unknown>>(TAILNET_PROBE_KEY);
    expect(Object.keys(saved ?? {})).toEqual(['1']);
  });

  it('returns the saved classes without discovery', async () => {
    const core = createFakeCore();
    withPorts(core, ['100.64.0.1:5900']);
    const kv = new MemoryKv();
    await probeTailnet(core, kv, probeTargets([mac]));
    core.discover = undefined;
    await expect(
      probeTailnet(core, kv, probeTargets([mac]), { force: true }),
    ).resolves.toEqual({ '1': { computer: 'macos', ssh: false } });
  });

  it('ignores malformed saved entries', () => {
    const kv = new MemoryKv();
    kv.setJSON(TAILNET_PROBE_KEY, {
      '1': { addr: '100.64.0.1', at: 1, vnc: 'yes', rdp: false, ssh: false },
      '2': { addr: '100.64.0.2', at: 1, vnc: false, rdp: true, ssh: false },
    });
    expect(readTailnetProbes(kv, probeTargets([mac, pc]))).toEqual({
      '2': { computer: 'windows', ssh: false },
    });
    kv.set(TAILNET_PROBE_KEY, '[]');
    expect(readTailnetProbes(kv, probeTargets([mac]))).toEqual({});
  });
});

describe('tailnetProbesWithin', () => {
  beforeEach(() => {
    jest.useFakeTimers();
  });

  afterEach(() => {
    jest.useRealTimers();
  });

  it('answers with the saved classes after 2 s and saves the late round', async () => {
    const core = createFakeCore();
    const kv = new MemoryKv();
    kv.setJSON(TAILNET_PROBE_KEY, {
      '1': { addr: '100.64.0.1', at: 0, vnc: true, rdp: false, ssh: false },
    });
    withPorts(core, ['100.64.0.1:3389'], 5_000);

    let answer: unknown;
    tailnetProbesWithin(core, kv, probeTargets([mac])).then(value => {
      answer = value;
    });
    await jest.advanceTimersByTimeAsync(PROBE_WAIT_MS - 1);
    expect(answer).toBeUndefined();
    await jest.advanceTimersByTimeAsync(1);
    expect(answer).toEqual({ '1': { computer: 'macos', ssh: false } });

    await jest.advanceTimersByTimeAsync(5_000);
    expect(readTailnetProbes(kv, probeTargets([mac]))).toEqual({
      '1': { computer: 'windows', ssh: false },
    });
  });

  it('answers as soon as a quick round ends', async () => {
    const core = createFakeCore();
    withPorts(core, ['100.64.0.2:3389'], 100);
    let answer: unknown;
    tailnetProbesWithin(core, new MemoryKv(), probeTargets([pc])).then(
      value => {
        answer = value;
      },
    );
    await jest.advanceTimersByTimeAsync(100);
    expect(answer).toEqual({ '2': { computer: 'windows', ssh: false } });
  });
});

it('documents the cache lifetime', () => {
  expect(PROBE_TTL_MS).toBe(10 * MINUTE_MS);
  expect(PROBE_WAIT_MS).toBe(2_000);
});
