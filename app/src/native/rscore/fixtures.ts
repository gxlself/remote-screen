import type {
  Computer,
  Directory,
  FileEntry,
  HostDetail,
  Identity,
  NetworkStatus,
  PairedHost,
  PairFileSummary,
  PairTarget,
  Server,
  SessionSummary,
  SshKey,
  SshServer,
  TailnetRouteStatus,
  TailnetStatus,
  TrustedHostKey,
} from './types';

const MINUTE_US = 60_000_000;
const HOUR_US = 60 * MINUTE_US;
const DAY_US = 24 * HOUR_US;

export const FIXTURE_NOW = 1792465134000000;
export const MIN_HOST_VERSION = '0.3.0';

const STUDIO_FP = 'w-LXse0ld21C5mCG6UpzNHUXoKVl-IiQwO9LWYDVFdQ';
const OFFICE_FP = 'tNTNZkRY8uF2nmpKHGT4leM_1nS8w5M-dd37_l0BbIg';
const MACBOOK_FP = 'oTw-sjBpVKOCNZq9kDfEl9_1aiTxdq69qm2ETaYMGe0';
const HOME_PC_FP = 'GIg6y_HoiKJk3itozo6D3Kafo7B30Y4nfN2BibobWck';
const LAB_MAC_FP = 'fwONQell4qAaPsA73VsIGv1PEBruwguor8M2GM5Nswg';

export const SELF_DEVICE_ID = 'p8r2t6y4u1i9oa3s';

const STUDIO_HOST_ID = 'h7v2m9q3k8x4tb1a';
const OFFICE_HOST_ID = 'w3k8p2x7m4q9tb6c';
const MACBOOK_HOST_ID = 'm5q1z8r2k7v3xa9d';
const HOME_PC_HOST_ID = 'd2n6c4v8b1x5za7e';
const LAB_MAC_HOST_ID = 'l4b8n2m6q1w5ec3r';

export const computers: Computer[] = [
  {
    hostId: STUDIO_HOST_ID,
    name: 'Studio Mac mini',
    node: 'studio-mac',
    os: 'macos',
    osVersion: 'macOS 26.1',
    online: true,
    hostRunning: true,
    lastSeenAt: FIXTURE_NOW,
    lastHeartbeatAt: FIXTURE_NOW,
    hostVersion: '0.3.1',
    features: 32767,
    fp: STUDIO_FP,
    addresses: [
      { kind: 'tailnet', addr: '100.64.0.7', port: 7400 },
      {
        kind: 'lan',
        addr: '192.168.31.20',
        port: 7400,
        cidr: '192.168.31.0/24',
      },
    ],
    displays: [
      {
        id: 1,
        name: 'Studio Display',
        widthPx: 2560,
        heightPx: 1600,
        primary: true,
      },
      {
        id: 2,
        name: '竖屏显示器',
        widthPx: 2160,
        heightPx: 3840,
        primary: false,
      },
    ],
    permsOk: true,
    sessions: [],
    paired: true,
    update: { channel: 'stable', available: null },
    thumbnailUrl: `/v1/hosts/${STUDIO_HOST_ID}/thumbnail`,
    filevault: true,
    console: { loggedIn: true, locked: false, loginWindow: false },
  },
  {
    hostId: OFFICE_HOST_ID,
    name: '工位 Windows',
    node: 'office-win',
    os: 'windows',
    osVersion: 'Windows 11',
    online: true,
    hostRunning: true,
    lastSeenAt: FIXTURE_NOW,
    lastHeartbeatAt: FIXTURE_NOW,
    hostVersion: '0.3.1',
    features: 32767,
    fp: OFFICE_FP,
    addresses: [
      { kind: 'tailnet', addr: '100.64.0.12', port: 7400 },
      {
        kind: 'intranet',
        addr: '10.30.12.48',
        port: 7400,
        cidr: '10.30.0.0/16',
      },
    ],
    displays: [
      {
        id: 1,
        name: 'Dell U2723QE',
        widthPx: 2560,
        heightPx: 1440,
        primary: true,
      },
    ],
    permsOk: true,
    sessions: [],
    paired: true,
    update: { channel: 'stable', available: null },
    thumbnailUrl: '',
    filevault: false,
    console: { loggedIn: true, locked: false, loginWindow: false },
  },
  {
    hostId: MACBOOK_HOST_ID,
    name: '小明的 MacBook Pro',
    node: 'xl-mbp',
    os: 'macos',
    osVersion: 'macOS 26.1',
    online: false,
    hostRunning: false,
    lastSeenAt: FIXTURE_NOW - 20 * MINUTE_US,
    lastHeartbeatAt: FIXTURE_NOW - 20 * MINUTE_US,
    hostVersion: '0.3.1',
    features: 32767,
    fp: MACBOOK_FP,
    addresses: [{ kind: 'tailnet', addr: '100.64.0.9', port: 7400 }],
    displays: [
      {
        id: 1,
        name: '内建显示器',
        widthPx: 3024,
        heightPx: 1964,
        primary: true,
      },
    ],
    permsOk: true,
    sessions: [],
    paired: true,
    update: { channel: 'stable', available: null },
    thumbnailUrl: '',
    filevault: true,
    console: null,
  },
  {
    hostId: HOME_PC_HOST_ID,
    name: '家里台式机',
    node: 'home-pc',
    os: 'windows',
    osVersion: 'Windows 11',
    online: false,
    hostRunning: false,
    lastSeenAt: FIXTURE_NOW - 2 * HOUR_US,
    lastHeartbeatAt: FIXTURE_NOW - 2 * HOUR_US,
    hostVersion: '0.2.4',
    features: 32767,
    fp: HOME_PC_FP,
    addresses: [{ kind: 'tailnet', addr: '100.64.0.15', port: 7400 }],
    displays: [
      {
        id: 1,
        name: 'LG 24MP60G',
        widthPx: 1920,
        heightPx: 1080,
        primary: true,
      },
    ],
    permsOk: true,
    sessions: [],
    paired: true,
    update: { channel: 'stable', available: '0.3.1' },
    thumbnailUrl: '',
    filevault: false,
    console: null,
  },
  {
    hostId: LAB_MAC_HOST_ID,
    name: '实验室 Mac',
    node: 'lab-mac',
    os: 'macos',
    osVersion: 'macOS 26.1',
    online: true,
    hostRunning: false,
    lastSeenAt: FIXTURE_NOW,
    lastHeartbeatAt: FIXTURE_NOW - 5 * MINUTE_US,
    hostVersion: '0.3.1',
    features: 32767,
    fp: LAB_MAC_FP,
    addresses: [{ kind: 'tailnet', addr: '100.64.0.18', port: 7400 }],
    displays: [
      {
        id: 1,
        name: '内建显示器',
        widthPx: 2560,
        heightPx: 1440,
        primary: true,
      },
    ],
    permsOk: true,
    sessions: [],
    paired: false,
    update: { channel: 'stable', available: null },
    thumbnailUrl: '',
    filevault: true,
    console: null,
  },
];

export const servers: Server[] = [
  {
    node: 'prod-api-01',
    addr: '100.64.0.31',
    online: true,
    os: 'Ubuntu 24.04',
    tailscaleSsh: true,
  },
  {
    node: 'gpu-train-02',
    addr: '100.64.0.32',
    online: true,
    os: 'Ubuntu 22.04',
    tailscaleSsh: false,
  },
  {
    node: 'nas-home',
    addr: '100.64.0.33',
    online: true,
    os: 'Debian 12',
    tailscaleSsh: false,
  },
];

export const directory: Directory = {
  others: [],
  generatedAt: FIXTURE_NOW,
  computers,
  servers,
};

export const hostDetail = (
  hostId: string,
  source: readonly Computer[] = computers,
): HostDetail => {
  const computer = source.find(candidate => candidate.hostId === hostId);
  if (!computer) {
    throw new Error(`Unknown host fixture: ${hostId}`);
  }

  return {
    hostId: computer.hostId,
    name: computer.name,
    node: computer.node,
    os: computer.os,
    osVersion:
      computer.osVersion ??
      (computer.os === 'macos' ? 'macOS 26.1' : 'Windows 11'),
    online: computer.online,
    hostRunning: computer.hostRunning,
    lastSeenAt: computer.lastSeenAt,
    lastHeartbeatAt: computer.lastHeartbeatAt,
    hostVersion: computer.hostVersion,
    features: computer.features,
    fp: computer.fp,
    addresses: computer.addresses,
    displays: computer.displays,
    sessions: computer.sessions,
    arch: computer.os === 'macos' ? 'universal' : 'x64',
    perms: { screen: true, accessibility: true },
    paired: computer.paired ? [SELF_DEVICE_ID] : [],
    update: {
      channel: computer.update.channel,
      pending: computer.update.available,
      state: 'idle',
    },
    thumbnailUrl: computer.thumbnailUrl === '' ? null : computer.thumbnailUrl,
    filevault: computer.filevault,
    console: computer.console,
  };
};

export const JUMP_HOST = { name: 'jump-01', host: '10.30.0.5', port: 22 };

export const sshServers: SshServer[] = [
  {
    serverId: 'prod-api-01',
    name: 'prod-api-01',
    host: '100.64.0.31',
    port: 22,
    user: 'alice',
    jump: null,
    auth: 'tailscale',
    tailscaleSsh: true,
    os: 'Ubuntu 24.04',
    source: 'directory',
  },
  {
    serverId: 'gpu-train-02',
    name: 'gpu-train-02',
    host: '100.64.0.32',
    port: 22,
    user: 'alice',
    jump: null,
    auth: 'deviceKey',
    tailscaleSsh: false,
    os: 'Ubuntu 22.04',
    source: 'directory',
  },
  {
    serverId: 'nas-home',
    name: 'nas-home',
    host: '100.64.0.33',
    port: 22,
    user: 'alice',
    jump: null,
    auth: 'deviceKey',
    tailscaleSsh: false,
    os: 'Debian 12',
    source: 'directory',
  },
  {
    serverId: 'manual-build-srv',
    name: 'build-srv',
    host: '10.30.20.31',
    port: 22,
    user: 'alice',
    jump: 'jump-01',
    auth: 'password',
    tailscaleSsh: false,
    os: 'Ubuntu 24.04',
    source: 'manual',
  },
];

export const pairedHosts: PairedHost[] = [
  {
    hostId: STUDIO_HOST_ID,
    fp: STUDIO_FP,
    name: 'Studio Mac mini',
    addresses: ['100.64.0.7:7400', '192.168.31.20:7400'],
    pairedAt: FIXTURE_NOW - 120 * DAY_US,
    os: null,
    osVersion: null,
    hostVersion: null,
    features: null,
    displays: [],
    lastSeenAt: null,
  },
  {
    hostId: OFFICE_HOST_ID,
    fp: OFFICE_FP,
    name: '工位 Windows',
    addresses: ['100.64.0.12:7400', '10.30.12.48:7400'],
    pairedAt: FIXTURE_NOW - 60 * DAY_US,
    os: null,
    osVersion: null,
    hostVersion: null,
    features: null,
    displays: [],
    lastSeenAt: null,
  },
  {
    hostId: MACBOOK_HOST_ID,
    fp: MACBOOK_FP,
    name: '小明的 MacBook Pro',
    addresses: ['100.64.0.9:7400'],
    pairedAt: FIXTURE_NOW - 30 * DAY_US,
    os: null,
    osVersion: null,
    hostVersion: null,
    features: null,
    displays: [],
    lastSeenAt: null,
  },
  {
    hostId: HOME_PC_HOST_ID,
    fp: HOME_PC_FP,
    name: '家里台式机',
    addresses: ['100.64.0.15:7400'],
    pairedAt: FIXTURE_NOW - 10 * DAY_US,
    os: null,
    osVersion: null,
    hostVersion: null,
    features: null,
    displays: [],
    lastSeenAt: null,
  },
];

export const sshKeys: SshKey[] = [
  {
    keyId: 'rs-iphone-se',
    name: 'rs-iphone-se',
    kind: 'se-p256',
    fp: 'SHA256:q7Lm2cRvXo4WqwgtkV53b-uDk7MNiWtyokBmrDW9fXk',
    publicKey:
      'ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBCUxv14Dlvc+wct8Nhnah8/4rRROO/3pSYo4DMzgw1SG1nqrKavi3CLxutmPoYoqcUNmtue74K9V0dJ9jq8ASos= rs-iphone-se',
    createdAt: 1788314400000000,
    installedOn: ['gpu-train-02', 'nas-home'],
  },
];

export const trustedHostKeys: TrustedHostKey[] = [
  {
    serverId: 'gpu-train-02',
    keyType: 'ssh-ed25519',
    fp: 'SHA256:CD--4oUk0xO-kvYaRRSXY0WhM-sPJgjYaV5tNagXYdA',
    addedAt: FIXTURE_NOW - 20 * DAY_US,
  },
  {
    serverId: 'manual-build-srv',
    keyType: 'ssh-ed25519',
    fp: 'SHA256:xXMe9C0LZJN_XEY2jqX29YAC7gx1bgG6Eg-FaOmALik',
    addedAt: FIXTURE_NOW - 12 * DAY_US,
  },
];

export const netTailnetOk: NetworkStatus = {
  ifaces: [
    { name: 'utun4', addrs: ['100.64.0.21'], isVpn: true },
    { name: 'en0', addrs: ['192.168.31.45'], isVpn: false },
  ],
};

export const netCompanyVpn: NetworkStatus = {
  ifaces: [
    { name: 'utun5', addrs: ['10.99.4.27'], isVpn: true },
    { name: 'en0', addrs: ['192.168.31.45'], isVpn: false },
  ],
};

export const netNoTailnet: NetworkStatus = {
  ifaces: [{ name: 'en0', addrs: ['192.168.31.45'], isVpn: false }],
};

export const tailnetUnavailable: TailnetStatus = {
  state: 'unavailable',
  authUrl: null,
  self: null,
  magicDnsSuffix: '',
  peers: [],
};

export const tailnetNeedsLogin: TailnetStatus = {
  state: 'needsLogin',
  authUrl: 'https://hs.example.com/register/nodekey-test',
  self: null,
  magicDnsSuffix: '',
  peers: [],
};

export const tailnetRunning: TailnetStatus = {
  state: 'running',
  authUrl: null,
  self: {
    hostname: 'rs-ios-k7m2q4xa',
    dnsName: 'rs-ios-k7m2q4xa.ts.example.com.',
    ips: ['100.64.0.21', 'fd7a:115c:a1e0::15'],
    online: true,
    derpRegion: 'sha',
    derpRegionName: '上海',
  },
  magicDnsSuffix: 'ts.example.com',
  peers: [
    {
      hostname: 'office-win',
      dnsName: 'office-win.ts.example.com.',
      ips: ['100.64.0.12'],
      os: 'windows',
      online: true,
      tags: [],
      direct: false,
      curAddr: '',
      relay: 'sha',
      peerRelay: '',
      lastHandshakeUs: 0,
      rxBytes: 64,
      txBytes: 32,
    },
    {
      hostname: 'studio-mac',
      dnsName: 'studio-mac.ts.example.com.',
      ips: ['100.64.0.7'],
      os: 'macos',
      online: true,
      tags: [],
      direct: true,
      curAddr: '192.168.31.20:41641',
      relay: 'sha',
      peerRelay: '',
      lastHandshakeUs: 0,
      rxBytes: 128,
      txBytes: 64,
    },
  ],
};

export const routeEmbeddedRunning: TailnetRouteStatus = {
  mode: 'embedded',
  active: 'embedded',
  available: true,
  controlUrlSet: true,
  embedded: {
    state: 'running',
    hostname: 'rs-ios-k7m2q4xa',
    ips: ['100.64.0.21', 'fd7a:115c:a1e0::15'],
    derpRegion: 'sha',
    derpRegionName: '上海',
  },
};

export const routeEmbeddedNeedsLogin: TailnetRouteStatus = {
  mode: 'embedded',
  active: 'embedded',
  available: true,
  controlUrlSet: true,
  embedded: {
    state: 'needsLogin',
    hostname: 'rs-ios-k7m2q4xa',
    ips: [],
    derpRegion: 'sha',
    derpRegionName: '上海',
  },
};

export const netCompanyVpnEmbedded: NetworkStatus = {
  ifaces: netCompanyVpn.ifaces,
  tailnet: routeEmbeddedRunning,
};

export const liveSession: SessionSummary = {
  id: 's-live-1',
  hostId: STUDIO_HOST_ID,
  hostName: 'Studio Mac mini',
  phase: 'streaming',
  role: 'controller',
  path: 'tailnetDirect',
  transport: 'quic',
  startedAt: FIXTURE_NOW - 2538e6,
  displayId: 1,
};

export const pairTargetStudio: PairTarget = {
  v: 1,
  hostId: STUDIO_HOST_ID,
  fp: STUDIO_FP,
  code: '482913',
  name: 'Studio Mac mini',
  addrs: ['100.64.0.7', '192.168.31.20'],
};

export const pairQrStudio = `rsp://pair?v=1&h=${pairTargetStudio.hostId}&fp=${
  pairTargetStudio.fp
}&c=${
  pairTargetStudio.code
}&n=Studio%20Mac%20mini&a=${pairTargetStudio.addrs.join(',')}`;

export const fixtureIdentity: Identity = {
  deviceId: SELF_DEVICE_ID,
  fp: 'uoyD8XaPUkbhaFwvnIGuVy3Yy_zHbTXLTjAJvtcw9qo',
};

export const fileTrees: Record<string, Record<string, FileEntry[]>> = {
  [`computer:${pairedHosts[0]?.hostId ?? ''}`]: {
    '/Users/demo': [
      {
        name: 'Desktop',
        kind: 'dir',
        size: null,
        mtimeUs: FIXTURE_NOW - 12 * MINUTE_US,
        childCount: 3,
      },
      {
        name: 'Documents',
        kind: 'dir',
        size: null,
        mtimeUs: FIXTURE_NOW - DAY_US,
        childCount: 2,
      },
      {
        name: 'Downloads',
        kind: 'dir',
        size: null,
        mtimeUs: FIXTURE_NOW - 2 * HOUR_US,
        childCount: 2,
      },
      {
        name: 'notes-1020.md',
        kind: 'file',
        size: 6144,
        mtimeUs: FIXTURE_NOW - 88 * MINUTE_US,
        childCount: null,
      },
      {
        name: 'remote-screen-0.3.1.dmg',
        kind: 'file',
        size: 196712858,
        mtimeUs: FIXTURE_NOW - 66 * MINUTE_US,
        childCount: null,
      },
      {
        name: 'Q3-remote-screen-accessibility-review-notes-final-approved-2026-10-20.pdf',
        kind: 'file',
        size: 4823449,
        mtimeUs: FIXTURE_NOW - 3 * HOUR_US,
        childCount: null,
      },
      {
        name: 'latest-build',
        kind: 'symlink',
        size: 17,
        mtimeUs: FIXTURE_NOW - 45 * MINUTE_US,
        childCount: null,
      },
      {
        name: 'screen-capture.sock',
        kind: 'other',
        size: null,
        mtimeUs: FIXTURE_NOW - 8 * MINUTE_US,
        childCount: null,
      },
    ],
    '/Users/demo/Desktop': [
      {
        name: '设计稿',
        kind: 'dir',
        size: null,
        mtimeUs: FIXTURE_NOW - 30 * MINUTE_US,
        childCount: 12,
      },
      {
        name: '截屏 10.20.png',
        kind: 'file',
        size: 2516582,
        mtimeUs: FIXTURE_NOW - 37 * MINUTE_US,
        childCount: null,
      },
      {
        name: 'latest-build',
        kind: 'symlink',
        size: 17,
        mtimeUs: FIXTURE_NOW - 45 * MINUTE_US,
        childCount: null,
      },
    ],
    '/Users/demo/Documents': [
      {
        name: '项目说明.md',
        kind: 'file',
        size: 12288,
        mtimeUs: FIXTURE_NOW - 2 * DAY_US,
        childCount: null,
      },
      {
        name: '交接清单.txt',
        kind: 'file',
        size: 2048,
        mtimeUs: FIXTURE_NOW - 5 * DAY_US,
        childCount: null,
      },
    ],
    '/Users/demo/Downloads': [
      {
        name: 'remote-screen-0.3.1.dmg',
        kind: 'file',
        size: 196712858,
        mtimeUs: FIXTURE_NOW - 66 * MINUTE_US,
        childCount: null,
      },
      {
        name: 'Q3-remote-screen-accessibility-review-notes-final-approved-2026-10-20.pdf',
        kind: 'file',
        size: 4823449,
        mtimeUs: FIXTURE_NOW - 3 * HOUR_US,
        childCount: null,
      },
    ],
  },
  [`server:${sshServers[0]?.serverId ?? ''}`]: {
    '/home/demo': [
      {
        name: 'logs',
        kind: 'dir',
        size: null,
        mtimeUs: FIXTURE_NOW - 20 * MINUTE_US,
        childCount: 2,
      },
      {
        name: 'app.tar.gz',
        kind: 'file',
        size: 15728640,
        mtimeUs: FIXTURE_NOW - 6 * HOUR_US,
        childCount: null,
      },
    ],
    '/home/demo/logs': [
      {
        name: 'app.log',
        kind: 'file',
        size: 1048576,
        mtimeUs: FIXTURE_NOW - 10 * MINUTE_US,
        childCount: null,
      },
      {
        name: 'error.log',
        kind: 'file',
        size: 32768,
        mtimeUs: FIXTURE_NOW - 2 * HOUR_US,
        childCount: null,
      },
    ],
  },
};

export const pairFileSummary: PairFileSummary = {
  fileId: 'pair-file-demo-1',
  hostId: STUDIO_HOST_ID,
  hostName: 'Studio Mac mini',
  fp: STUDIO_FP,
  addrs: ['100.64.0.7:7843'],
  expiresAt: FIXTURE_NOW + 10 * MINUTE_US,
  expired: false,
};

export const pairFileText =
  'Remote Screen pairing file\nhost=Studio Mac mini\naddr=100.64.0.7:7843\nfingerprint=w-LXse0ld21C5mCG6UpzNHUXoKVl-IiQwO9LWYDVFdQ\nsecret=fixture-only';
