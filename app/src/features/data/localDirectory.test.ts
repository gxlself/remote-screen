import {
  FIXTURE_NOW,
  MIN_HOST_VERSION,
  tailnetRunning,
} from '../../native/rscore/fixtures';
import type {
  PairedHost,
  TailnetPeer,
  TailnetStatus,
  TailscaleDevice,
} from '../../native/rscore/types';
import { cardModel, compareVersion } from '../directory/cardModel';
import type { NetVerdict } from '../network/judge';
import type { TailnetProbes } from '../tailscale/apiDevices';
import { buildLocalDirectory } from './localDirectory';
import type { LocalReach } from './localDirectory';

const DAY_US = 86_400_000_000;

function host(overrides: Partial<PairedHost> = {}): PairedHost {
  return {
    hostId: 'h7v2m9q3k8x4tb1a',
    fp: 'fp-studio',
    name: 'Studio Mac mini',
    addresses: ['100.64.0.7:7400', '192.168.31.20:7400'],
    pairedAt: FIXTURE_NOW - 30 * DAY_US,
    os: null,
    osVersion: null,
    hostVersion: null,
    features: null,
    displays: [],
    lastSeenAt: null,
    ...overrides,
  };
}

function peer(overrides: Partial<TailnetPeer> = {}): TailnetPeer {
  return {
    hostname: 'node',
    dnsName: 'node.ts.example.com.',
    ips: ['100.64.0.50'],
    os: 'macOS',
    online: true,
    tags: [],
    direct: true,
    curAddr: '',
    relay: '',
    peerRelay: '',
    lastHandshakeUs: 0,
    rxBytes: 0,
    txBytes: 0,
    ...overrides,
  };
}

function tailnet(peers: TailnetPeer[]): TailnetStatus {
  return { ...tailnetRunning, peers };
}

function build(input: {
  paired?: PairedHost[];
  reach?: LocalReach;
  tailnet?: TailnetStatus | null;
}) {
  return buildLocalDirectory({
    paired: input.paired ?? [host()],
    reach: input.reach ?? {},
    tailnet: input.tailnet === undefined ? null : input.tailnet,
    now: FIXTURE_NOW,
  });
}

function only<T>(items: T[]): T {
  expect(items).toHaveLength(1);
  return items[0] as T;
}

const verdict: NetVerdict = {
  tailnet: 'up',
  tailnetAddr: '100.64.0.21',
  companyVpn: null,
  atCompany: null,
  banner: 'none',
};

describe('computers', () => {
  it('lists every paired Host and copies its identity', () => {
    const directory = build({
      paired: [host(), host({ hostId: 'k3jq7x2m9p4tbw8a', name: 'Office' })],
    });
    expect(directory.computers.map(c => c.hostId)).toEqual([
      'h7v2m9q3k8x4tb1a',
      'k3jq7x2m9p4tbw8a',
    ]);
    expect(directory.computers[0]).toMatchObject({
      hostId: 'h7v2m9q3k8x4tb1a',
      fp: 'fp-studio',
      name: 'Studio Mac mini',
    });
    expect(directory.generatedAt).toBe(FIXTURE_NOW);
  });

  it('takes node from the tailnet peer whose IP matches, else empty', () => {
    const matched = build({
      tailnet: tailnet([
        peer({ hostname: 'other', ips: ['100.64.0.99'] }),
        peer({ hostname: 'studio-mac', ips: ['100.64.0.7', 'fd7a::7'] }),
      ]),
    });
    expect(only(matched.computers).node).toBe('studio-mac');

    const unmatched = build({
      tailnet: tailnet([peer({ hostname: 'other', ips: ['100.64.0.99'] })]),
    });
    expect(only(unmatched.computers).node).toBe('');
    expect(only(build({ tailnet: null }).computers).node).toBe('');
  });

  it('splits ip:port and marks tailnet or lan without a cidr', () => {
    const computer = only(
      build({
        paired: [
          host({
            addresses: [
              '100.64.0.7:7400',
              '192.168.31.20:7401',
              '[fd7a:115c:a1e0::7]:7402',
            ],
          }),
        ],
      }).computers,
    );
    expect(computer.addresses).toEqual([
      { kind: 'tailnet', addr: '100.64.0.7', port: 7400 },
      { kind: 'lan', addr: '192.168.31.20', port: 7401 },
      { kind: 'lan', addr: 'fd7a:115c:a1e0::7', port: 7402 },
    ]);
    for (const address of computer.addresses) {
      expect(address).not.toHaveProperty('cidr');
    }
  });

  it('maps os from the Host first, linux as windows', () => {
    const studioPeer = peer({ ips: ['100.64.0.7'], os: 'windows' });
    const os = (value: PairedHost['os']) =>
      only(
        build({
          paired: [host({ os: value })],
          tailnet: tailnet([studioPeer]),
        }).computers,
      ).os;
    expect(os('macos')).toBe('macos');
    expect(os('windows')).toBe('windows');
    expect(os('linux')).toBe('windows');
  });

  it('falls back to the matched peer os, then macos', () => {
    const os = (peers: TailnetPeer[] | null) =>
      only(build({ tailnet: peers === null ? null : tailnet(peers) }).computers)
        .os;
    expect(os([peer({ ips: ['100.64.0.7'], os: 'windows' })])).toBe('windows');
    expect(os([peer({ ips: ['100.64.0.7'], os: 'macOS' })])).toBe('macos');
    expect(os([peer({ ips: ['100.64.0.7'], os: 'linux' })])).toBe('windows');
    expect(os([peer({ ips: ['100.64.0.7'], os: 'iOS' })])).toBe('macos');
    expect(os([peer({ ips: ['100.64.0.99'], os: 'windows' })])).toBe('macos');
    expect(os(null)).toBe('macos');
  });

  it('reads online and hostRunning from reach, offline when never probed', () => {
    const reach: LocalReach = {
      h7v2m9q3k8x4tb1a: { online: true, at: FIXTURE_NOW - 1000 },
      k3jq7x2m9p4tbw8a: { online: false, at: FIXTURE_NOW - 2000 },
    };
    const directory = build({
      paired: [
        host(),
        host({ hostId: 'k3jq7x2m9p4tbw8a' }),
        host({ hostId: 'm4xk8q2v7p3nz6a1' }),
      ],
      reach,
    });
    expect(directory.computers.map(c => [c.online, c.hostRunning])).toEqual([
      [true, true],
      [false, false],
      [false, false],
    ]);
    expect(directory.computers.map(c => c.lastHeartbeatAt)).toEqual([
      null,
      null,
      null,
    ]);
  });

  it('uses the latest of reach.at, lastSeenAt and pairedAt for lastSeenAt', () => {
    const pairedAt = FIXTURE_NOW - 10 * DAY_US;
    const lastSeen = (reachAt: number | null, seenAt: number | null) =>
      only(
        build({
          paired: [host({ pairedAt, lastSeenAt: seenAt })],
          reach:
            reachAt === null
              ? {}
              : { h7v2m9q3k8x4tb1a: { online: false, at: reachAt } },
        }).computers,
      ).lastSeenAt;
    expect(lastSeen(null, null)).toBe(pairedAt);
    expect(lastSeen(null, pairedAt + 5)).toBe(pairedAt + 5);
    expect(lastSeen(pairedAt + 9, pairedAt + 5)).toBe(pairedAt + 9);
    expect(lastSeen(pairedAt + 1, pairedAt + 5)).toBe(pairedAt + 5);
    expect(lastSeen(pairedAt - 1, null)).toBe(pairedAt);
  });

  it('copies hostVersion, features, displays and osVersion from the last hello', () => {
    const display = {
      id: 1,
      name: 'Built-in',
      widthPx: 3024,
      heightPx: 1964,
      primary: true,
    };
    const computer = only(
      build({
        paired: [
          host({
            hostVersion: '0.3.2',
            features: 2 ** 22,
            displays: [display],
            osVersion: '15.1',
          }),
        ],
      }).computers,
    );
    expect(computer).toMatchObject({
      hostVersion: '0.3.2',
      features: 2 ** 22,
      displays: [display],
      osVersion: '15.1',
    });
  });

  it('defaults hostVersion to an empty string that does not trigger 170', () => {
    const computer = only(
      build({
        reach: { h7v2m9q3k8x4tb1a: { online: true, at: FIXTURE_NOW } },
      }).computers,
    );
    expect(computer.hostVersion).toBe('');
    expect(computer.features).toBe(0);
    expect(computer.displays).toEqual([]);
    expect(computer).not.toHaveProperty('osVersion');
    expect(compareVersion(computer.hostVersion, MIN_HOST_VERSION)).toBe(0);
    const card = cardModel({
      computer,
      nowUs: FIXTURE_NOW,
      verdict,
      minHostVersion: MIN_HOST_VERSION,
    });
    expect(card.tap).toEqual({ kind: 'open' });
  });

  it('fills unavailable directory fields with neutral defaults', () => {
    expect(only(build({}).computers)).toMatchObject({
      permsOk: true,
      sessions: [],
      paired: true,
      update: { channel: 'stable', available: null },
      thumbnailUrl: '',
      filevault: false,
      console: null,
      wake: null,
    });
  });
});

describe('servers', () => {
  it('lists tag:server peers with their first IPv4', () => {
    const directory = build({
      tailnet: tailnet([
        peer({
          hostname: 'build-01',
          ips: ['fd7a::5', '100.64.0.5'],
          os: 'linux',
          online: false,
          tags: ['tag:server'],
        }),
        peer({ hostname: 'plain', ips: ['100.64.0.6'], os: 'linux' }),
      ]),
    });
    expect(directory.servers).toEqual([
      {
        node: 'build-01',
        addr: '100.64.0.5',
        online: false,
        os: 'linux',
        tailscaleSsh: null,
      },
    ]);
  });
});

describe('others', () => {
  it('lists Mac and Windows peers without a Host', () => {
    const directory = build({
      tailnet: tailnet([
        peer({ hostname: 'alice-mac', ips: ['100.64.0.30', 'fd7a::30'] }),
        peer({
          hostname: 'bob-pc',
          ips: ['100.64.0.31'],
          os: 'windows',
          online: false,
        }),
        peer({ hostname: 'phone', ips: ['100.64.0.32'], os: 'iOS' }),
        peer({ hostname: 'pixel', ips: ['100.64.0.33'], os: 'android' }),
        peer({ hostname: 'nas', ips: ['100.64.0.34'], os: 'linux' }),
      ]),
    });
    expect(directory.others).toEqual([
      {
        node: 'alice-mac',
        name: 'alice-mac',
        os: 'macos',
        addrs: ['100.64.0.30', 'fd7a::30'],
        online: true,
        lastSeenAt: null,
      },
      {
        node: 'bob-pc',
        name: 'bob-pc',
        os: 'windows',
        addrs: ['100.64.0.31'],
        online: false,
        lastSeenAt: null,
      },
    ]);
  });

  it('skips tag:rs-host and tag:server peers', () => {
    const directory = build({
      tailnet: tailnet([
        peer({ hostname: 'tagged-host', tags: ['tag:rs-host'] }),
        peer({ hostname: 'tagged-server', tags: ['tag:server'] }),
      ]),
    });
    expect(directory.others).toEqual([]);
  });

  it('skips peers whose IP belongs to a paired Host', () => {
    const directory = build({
      tailnet: tailnet([
        peer({ hostname: 'studio-mac', ips: ['100.64.0.7'] }),
        peer({ hostname: 'lan-match', ips: ['192.168.31.20'] }),
      ]),
    });
    expect(directory.others).toEqual([]);
  });

  it('is empty with system Tailscale (tailnet null)', () => {
    const directory = build({ tailnet: null });
    expect(directory.others).toEqual([]);
    expect(directory.servers).toEqual([]);
  });
});

describe('Tailscale API devices', () => {
  const MINUTE_US = 60_000_000;

  function device(overrides: Partial<TailscaleDevice> = {}): TailscaleDevice {
    return {
      id: 'n1',
      name: 'node',
      dnsName: 'node.tail1234.ts.net',
      os: 'linux',
      addresses: ['100.64.0.60'],
      online: true,
      lastSeenAt: null,
      tags: [],
      ...overrides,
    };
  }

  function withApi(
    apiDevices: TailscaleDevice[] | null,
    input: { paired?: PairedHost[]; tailnet?: TailnetStatus | null } = {},
  ) {
    return buildLocalDirectory({
      paired: input.paired ?? [host()],
      reach: {},
      tailnet: input.tailnet ?? null,
      apiDevices,
      now: FIXTURE_NOW,
    });
  }

  it('lists Linux as a server, Mac and Windows as others, and skips phones', () => {
    const directory = withApi([
      device({
        id: 'n1',
        name: 'aliyun-bj',
        addresses: ['100.64.0.60', 'fd7a::60'],
      }),
      device({
        id: 'n2',
        name: 'alice-mac',
        os: 'macos',
        addresses: ['100.64.0.61', 'fd7a::61'],
      }),
      device({
        id: 'n3',
        name: 'bob-pc',
        os: 'windows',
        addresses: ['100.64.0.62'],
        online: false,
        lastSeenAt: FIXTURE_NOW - 60 * MINUTE_US,
      }),
      device({
        id: 'n4',
        name: 'iphone',
        os: 'ios',
        addresses: ['100.64.0.63'],
      }),
    ]);
    expect(directory.servers).toEqual([
      {
        node: 'aliyun-bj',
        addr: '100.64.0.60',
        online: true,
        os: 'linux',
        tailscaleSsh: null,
      },
    ]);
    expect(directory.others).toEqual([
      {
        node: 'alice-mac',
        name: 'alice-mac',
        os: 'macos',
        addrs: ['100.64.0.61', 'fd7a::61'],
        online: true,
        lastSeenAt: null,
      },
      {
        node: 'bob-pc',
        name: 'bob-pc',
        os: 'windows',
        addrs: ['100.64.0.62'],
        online: false,
        lastSeenAt: FIXTURE_NOW - 60 * MINUTE_US,
      },
    ]);
    expect(JSON.stringify(directory)).not.toContain('iphone');
    expect(only(directory.computers).hostId).toBe('h7v2m9q3k8x4tb1a');
  });

  it('skips Android and unknown systems too', () => {
    const directory = withApi([
      device({ name: 'pixel', os: 'android' }),
      device({ name: 'printer', os: 'other' }),
    ]);
    expect(directory.servers).toEqual([]);
    expect(directory.others).toEqual([]);
  });

  it('uses the first address for a Linux device without IPv4', () => {
    const directory = withApi([
      device({ name: 'v6-only', addresses: ['fd7a::70', 'fd7a::71'] }),
    ]);
    expect(only(directory.servers).addr).toBe('fd7a::70');
  });

  it('lists a device once when an in-app tailnet peer has the same address', () => {
    const directory = withApi(
      [
        device({
          name: 'alice-mac-api',
          os: 'macos',
          addresses: ['100.64.0.30'],
        }),
        device({ name: 'gpu-01-api', addresses: ['100.64.0.41', 'fd7a::41'] }),
      ],
      {
        tailnet: tailnet([
          peer({ hostname: 'alice-mac', ips: ['100.64.0.30', 'fd7a::30'] }),
          peer({
            hostname: 'gpu-01',
            ips: ['fd7a::41', '100.64.0.41'],
            os: 'linux',
            tags: ['tag:server'],
          }),
        ]),
      },
    );
    expect(directory.others.map(other => other.node)).toEqual(['alice-mac']);
    expect(directory.servers.map(server => server.node)).toEqual(['gpu-01']);
  });

  it('skips devices at a paired Host address', () => {
    const directory = withApi([
      device({ name: 'studio-mac', os: 'macos', addresses: ['100.64.0.7'] }),
      device({ name: 'studio-linux', addresses: ['192.168.31.20'] }),
    ]);
    expect(directory.others).toEqual([]);
    expect(directory.servers).toEqual([]);
    expect(only(directory.computers).name).toBe('Studio Mac mini');
  });

  it('applies the peer tag rules: no Host or Hub nodes, tag:server is a server', () => {
    const directory = withApi([
      device({
        name: 'host-mac',
        os: 'macos',
        addresses: ['100.64.0.64'],
        tags: ['tag:rs-host'],
      }),
      device({
        name: 'rs-hub',
        addresses: ['100.64.0.65'],
        tags: ['tag:rs-hub'],
      }),
      device({
        name: 'build-mac',
        os: 'macos',
        addresses: ['100.64.0.66'],
        tags: ['tag:server'],
      }),
    ]);
    expect(directory.others).toEqual([]);
    expect(directory.servers).toEqual([
      {
        node: 'build-mac',
        addr: '100.64.0.66',
        online: true,
        os: 'macos',
        tailscaleSsh: null,
      },
    ]);
  });

  it('judges online by lastSeenAt within 5 minutes when online is null', () => {
    const online = (
      value: boolean | null,
      lastSeenAt: number | null,
    ): boolean | undefined =>
      withApi([device({ name: 'mac', os: 'macos', online: value, lastSeenAt })])
        .others[0]?.online;
    expect(online(null, FIXTURE_NOW - 4 * MINUTE_US)).toBe(true);
    expect(online(null, FIXTURE_NOW - 5 * MINUTE_US)).toBe(true);
    expect(online(null, FIXTURE_NOW - 6 * MINUTE_US)).toBe(false);
    expect(online(null, null)).toBe(false);
    expect(online(false, FIXTURE_NOW)).toBe(false);
    expect(online(true, null)).toBe(true);
  });

  it('builds the same directory as before when apiDevices is null or empty', () => {
    const input = {
      paired: [host()],
      reach: {},
      tailnet: tailnet([
        peer({ hostname: 'alice-mac', ips: ['100.64.0.30'] }),
        peer({
          hostname: 'gpu-01',
          ips: ['100.64.0.41'],
          os: 'linux',
          tags: ['tag:server'],
        }),
      ]),
      now: FIXTURE_NOW,
    };
    const before = buildLocalDirectory(input);
    expect(buildLocalDirectory({ ...input, apiDevices: null })).toEqual(before);
    expect(buildLocalDirectory({ ...input, apiDevices: [] })).toEqual(before);
  });
});

describe('Headscale devices by probed ports', () => {
  function node(overrides: Partial<TailscaleDevice>): TailscaleDevice {
    return {
      id: '1',
      name: 'node',
      dnsName: '',
      os: 'unknown',
      addresses: ['100.64.0.80'],
      online: true,
      lastSeenAt: null,
      tags: [],
      ...overrides,
    };
  }

  function withProbes(
    apiDevices: TailscaleDevice[],
    apiProbes: TailnetProbes,
    paired: PairedHost[] = [],
  ) {
    return buildLocalDirectory({
      paired,
      reach: {},
      tailnet: null,
      apiDevices,
      apiProbes,
      now: FIXTURE_NOW,
    });
  }

  it('lists 5900 as a Mac, 3389 as Windows and 22 as an SSH server', () => {
    const directory = withProbes(
      [
        node({ id: '1', name: 'studio', addresses: ['fd7a::81', '100.64.0.81'] }),
        node({ id: '2', name: 'office-pc', addresses: ['100.64.0.82'] }),
        node({ id: '3', name: 'gpu-box', addresses: ['100.64.0.83'] }),
      ],
      {
        '1': { computer: 'macos', ssh: false },
        '2': { computer: 'windows', ssh: false },
        '3': { computer: null, ssh: true },
      },
    );
    expect(directory.others).toEqual([
      {
        node: 'studio',
        name: 'studio',
        os: 'macos',
        addrs: ['fd7a::81', '100.64.0.81'],
        online: true,
        lastSeenAt: null,
      },
      {
        node: 'office-pc',
        name: 'office-pc',
        os: 'windows',
        addrs: ['100.64.0.82'],
        online: true,
        lastSeenAt: null,
      },
    ]);
    expect(directory.servers).toEqual([
      {
        node: 'gpu-box',
        addr: '100.64.0.83',
        online: true,
        os: 'linux',
        tailscaleSsh: null,
      },
    ]);
  });

  it('lists a Mac with SSH on both sides', () => {
    const directory = withProbes(
      [node({ id: '1', name: 'studio', addresses: ['fd7a::81', '100.64.0.81'] })],
      { '1': { computer: 'macos', ssh: true } },
    );
    expect(directory.others.map(other => other.node)).toEqual(['studio']);
    expect(directory.servers).toEqual([
      {
        node: 'studio',
        addr: '100.64.0.81',
        online: true,
        os: 'macos',
        tailscaleSsh: null,
      },
    ]);
  });

  it('leaves out closed, unprobed, offline, Host and Hub devices', () => {
    const directory = withProbes(
      [
        node({ id: '1', name: 'closed' }),
        node({ id: '2', name: 'unprobed' }),
        node({ id: '3', name: 'offline', online: false }),
        node({ id: '4', name: 'host-mac', tags: ['tag:rs-host'] }),
        node({ id: '5', name: 'rs-hub', tags: ['tag:rs-hub'] }),
        node({ id: '6', name: 'paired', addresses: ['100.64.0.7'] }),
      ],
      {
        '1': { computer: null, ssh: false },
        '3': { computer: 'macos', ssh: true },
        '4': { computer: 'macos', ssh: false },
        '5': { computer: null, ssh: true },
        '6': { computer: 'macos', ssh: false },
      },
      [host()],
    );
    expect(directory.others).toEqual([]);
    expect(directory.servers).toEqual([]);
  });

  it('lists nothing for unknown devices without probes', () => {
    const directory = withProbes([node({})], {});
    expect(directory.others).toEqual([]);
    expect(directory.servers).toEqual([]);
  });
});
