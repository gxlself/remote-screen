import type { Directory, PairedHost } from '../../native/rscore/types';
import { parseAutoConnect, resolveHostId } from './target';

it.each([
  ['k3jq7x2m9p4tbw8a', { kind: 'hostId', hostId: 'k3jq7x2m9p4tbw8a' }],
  ['100.64.0.7', { kind: 'addr', host: '100.64.0.7' }],
  ['100.64.0.7:7400', { kind: 'addr', host: '100.64.0.7', port: 7400 }],
  ['[fd7a:115c::7]:7400', { kind: 'addr', host: 'fd7a:115c::7', port: 7400 }],
  ['fd7a:115c::7', { kind: 'addr', host: 'fd7a:115c::7' }],
  ['studio.example.com', { kind: 'addr', host: 'studio.example.com' }],
  [
    'studio.example.com:7400',
    {
      kind: 'addr',
      host: 'studio.example.com',
      port: 7400,
    },
  ],
])('parses %s', (raw, expected) => {
  expect(parseAutoConnect(raw)).toEqual(expected);
});

it.each(['100.64.0.7:0', '100.64.0.7:65536', '', 'a b', '   ', ':7400'])(
  'rejects %s',
  raw => {
    expect(parseAutoConnect(raw)).toBeNull();
  },
);

const paired: PairedHost[] = [
  {
    hostId: 'paired-host-0001',
    fp: 'fp',
    name: 'Studio',
    addresses: ['100.64.0.7:7400', '[fd7a:115c::7]:7400'],
    pairedAt: 0,
    os: null,
    osVersion: null,
    hostVersion: null,
    features: null,
    displays: [],
    lastSeenAt: null,
  },
];

const hub: Pick<Directory, 'computers'> = {
  computers: [
    {
      hostId: 'directory-host-0001',
      name: 'Office',
      node: 'office',
      os: 'windows',
      online: true,
      hostRunning: true,
      lastSeenAt: 0,
      lastHeartbeatAt: null,
      hostVersion: '0.3.0',
      features: 0,
      fp: 'fp',
      addresses: [{ kind: 'tailnet', addr: '100.64.0.12', port: 7400 }],
      displays: [],
      permsOk: true,
      sessions: [],
      paired: false,
      update: { channel: 'stable', available: null },
      thumbnailUrl: '',
      filevault: false,
      console: null,
    },
  ],
};

it('resolves a host ID directly', () => {
  expect(
    resolveHostId({ kind: 'hostId', hostId: 'k3jq7x2m9p4tbw8a' }, paired, hub),
  ).toEqual({ hostId: 'k3jq7x2m9p4tbw8a' });
});

it('matches a paired address without its port or brackets', () => {
  expect(
    resolveHostId({ kind: 'addr', host: '100.64.0.7' }, paired, hub),
  ).toEqual({ hostId: 'paired-host-0001' });
  expect(
    resolveHostId({ kind: 'addr', host: 'fd7a:115c::7' }, paired, hub),
  ).toEqual({ hostId: 'paired-host-0001' });
});

it('falls back to the Hub directory', () => {
  expect(
    resolveHostId({ kind: 'addr', host: '100.64.0.12:9000' }, paired, hub),
  ).toEqual({ hostId: 'directory-host-0001' });
});

it('marks the first match ambiguous when two hosts share an address', () => {
  const duplicate: PairedHost[] = [
    ...paired,
    { ...paired[0]!, hostId: 'paired-host-0002' },
  ];
  expect(
    resolveHostId({ kind: 'addr', host: '100.64.0.7' }, duplicate, hub),
  ).toEqual({ hostId: 'paired-host-0001', ambiguous: true });
});

it('reports an unknown host', () => {
  expect(
    resolveHostId({ kind: 'addr', host: '100.64.0.99' }, paired, hub),
  ).toEqual({ error: 'unknownHost' });
});
