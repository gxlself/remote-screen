import { computers } from '../../native/rscore/fixtures';
import type { SshServer } from '../../native/rscore/types';
import type { ComputerEntry } from '../native/types';
import type { ManualServer } from '../servers/manual';
import { isTailnetHost, tailnetInUse } from './tailnetUse';
import type { TailnetUseInputs } from './tailnetUse';

const lanComputer = {
  ...computers[0],
  addresses: [
    {
      kind: 'lan' as const,
      addr: '192.168.31.20',
      port: 7400,
      cidr: '192.168.31.0/24',
    },
  ],
};

function manualEntry(host: string): ComputerEntry {
  return {
    kind: 'manual',
    key: `manual:${host}`,
    manual: {
      id: host,
      kind: 'computer',
      host,
      port: 5900,
      hostId: null,
      name: null,
      networkId: null,
      addedAtUs: 1,
      os: 'macos',
    },
  };
}

function sshServer(host: string, source: SshServer['source']): SshServer {
  return {
    serverId: host,
    name: host,
    host,
    port: 22,
    user: 'me',
    jump: null,
    auth: 'password',
    tailscaleSsh: null,
    source,
  };
}

function manualServer(host: string): ManualServer {
  return {
    id: host,
    kind: 'ssh',
    name: host,
    host,
    port: 22,
    user: 'me',
    auth: 'password',
    jump: null,
    networkId: null,
    addedAtUs: 1,
    updatedAtUs: 1,
  };
}

const lanOnly: TailnetUseInputs = {
  enabled: true,
  entries: [
    { kind: 'host', key: 'host:lan', computer: lanComputer },
    manualEntry('192.168.31.30'),
    manualEntry('mac-mini.local'),
  ],
  sshServers: [sshServer('10.30.12.48', 'manual')],
  manualServers: [manualServer('10.30.12.48')],
  apiConfigured: false,
  embeddedConfigured: false,
};

describe('isTailnetHost', () => {
  it('recognizes Tailscale addresses and MagicDNS names', () => {
    expect(isTailnetHost('100.64.0.7')).toBe(true);
    expect(isTailnetHost('100.127.255.254')).toBe(true);
    expect(isTailnetHost('[fd7a:115c:a1e0::1]')).toBe(true);
    expect(isTailnetHost('Studio.tail1234.TS.NET.')).toBe(true);
  });

  it('leaves local addresses and names alone', () => {
    expect(isTailnetHost('100.128.0.1')).toBe(false);
    expect(isTailnetHost('192.168.31.20')).toBe(false);
    expect(isTailnetHost('mac-mini.local')).toBe(false);
    expect(isTailnetHost('studio')).toBe(false);
  });
});

describe('tailnetInUse', () => {
  it('is false for someone who only uses the local network', () => {
    expect(tailnetInUse(lanOnly)).toBe(false);
    expect(
      tailnetInUse({
        enabled: true,
        entries: [],
        sshServers: [],
        manualServers: [],
        apiConfigured: false,
        embeddedConfigured: false,
      }),
    ).toBe(false);
  });

  it('is true once a computer was added by a tailnet address', () => {
    expect(
      tailnetInUse({
        ...lanOnly,
        entries: [...lanOnly.entries, manualEntry('100.64.0.9')],
      }),
    ).toBe(true);
    expect(
      tailnetInUse({
        ...lanOnly,
        entries: [manualEntry('studio.tail1234.ts.net')],
      }),
    ).toBe(true);
  });

  it('is true for a paired Host with a tailnet address', () => {
    expect(
      tailnetInUse({
        ...lanOnly,
        entries: [{ kind: 'host', key: 'host:ts', computer: computers[0] }],
      }),
    ).toBe(true);
  });

  it('is true for tailnet devices and servers', () => {
    expect(
      tailnetInUse({
        ...lanOnly,
        entries: [
          {
            kind: 'tailnet',
            key: 'tailnet:pc',
            device: {
              node: 'pc',
              name: 'PC',
              os: 'windows',
              addrs: ['100.64.0.30'],
              online: true,
              lastSeenAt: null,
            },
          },
        ],
      }),
    ).toBe(true);
    expect(
      tailnetInUse({
        ...lanOnly,
        sshServers: [sshServer('build', 'directory')],
      }),
    ).toBe(true);
    expect(
      tailnetInUse({
        ...lanOnly,
        manualServers: [manualServer('100.64.0.40')],
      }),
    ).toBe(true);
  });

  it('is true once a Tailscale API token or Headscale address is set', () => {
    expect(tailnetInUse({ ...lanOnly, apiConfigured: true })).toBe(true);
    expect(tailnetInUse({ ...lanOnly, embeddedConfigured: true })).toBe(true);
  });

  it('is false whatever the list holds while Tailscale is switched off', () => {
    expect(
      tailnetInUse({
        ...lanOnly,
        enabled: false,
        entries: [manualEntry('100.64.0.9')],
        sshServers: [sshServer('build', 'directory')],
        apiConfigured: true,
        embeddedConfigured: true,
      }),
    ).toBe(false);
  });
});
