import { FIXTURE_NOW, sshServers } from '../../native/rscore/fixtures';
import type { CoreEventOf, DiscoverOutcome } from '../../native/rscore/types';
import type { NetworkEntry } from '../settings/schema';
import {
  EMPTY_FORM,
  formToServer,
  hostKind,
  parseHostPort,
  requiredNetwork,
  serverToForm,
  shouldSuggestJump,
  testFromDiscover,
  testFromProbe,
  testTarget,
  validateServerForm,
} from './serverForm';
import type { HostKind, ServerForm } from './serverForm';

const networks: NetworkEntry[] = [
  {
    id: 'n1',
    name: '公司网络',
    cidrs: ['10.30.0.0/16'],
    vpnPools: ['10.99.0.0/16'],
  },
];

function form(patch: Partial<ServerForm> = {}): ServerForm {
  return {
    ...EMPTY_FORM,
    name: 'build-srv',
    user: 'alice',
    host: '10.30.20.31',
    ...patch,
  };
}

function probe(
  outcome: CoreEventOf<'probe.result'>['outcome'],
  ms: number | null = 18,
): CoreEventOf<'probe.result'> {
  return {
    type: 'probe.result',
    probeId: 'p1',
    hostId: null,
    path: 'manual',
    addr: '10.30.20.31',
    port: 22,
    transport: 'tcp',
    outcome,
    ms,
    last: true,
  };
}

describe('hostKind', () => {
  it.each<[string, HostKind]>([
    ['100.64.0.31', 'tailnet'],
    ['10.30.20.31', 'private'],
    ['192.168.31.20', 'private'],
    ['8.8.8.8', 'public'],
    ['gpu-train-02', 'magicDns'],
    ['prod-api-01.tail.ts.net', 'magicDns'],
    ['build.corp.example.com', 'name'],
    ['10.30.300.1', 'invalid'],
    ['', 'invalid'],
    ['a b', 'invalid'],
    ['-x', 'invalid'],
    ['fd7a:115c:a1e0::3', 'tailnet'],
    ['fe80::1', 'private'],
    ['2001:db8::1', 'public'],
    ['1::2::3', 'invalid'],
  ])('classifies %p as %p', (host, expected) => {
    expect(hostKind(host)).toBe(expected);
  });

  it('ignores surrounding whitespace', () => {
    expect(hostKind('  10.30.20.31  ')).toBe('private');
  });
});

describe('validateServerForm', () => {
  it('requires host and user for an empty form', () => {
    expect(validateServerForm(EMPTY_FORM, { existingNames: [] })).toEqual({
      host: 'required',
      user: 'required',
    });
  });

  it.each(['0', '70000', '2a'])('rejects invalid port %p', port => {
    expect(validateServerForm(form({ port }), { existingNames: [] })).toEqual({
      port: 'badPort',
    });
  });

  it('rejects a malformed host', () => {
    expect(
      validateServerForm(form({ host: '10.30.300.1' }), {
        existingNames: [],
      }),
    ).toEqual({ host: 'badHost' });
  });

  it('rejects Tailscale SSH outside the tailnet', () => {
    expect(
      validateServerForm(form({ auth: 'tailscale' }), {
        existingNames: [],
      }),
    ).toEqual({ auth: 'tailscaleNeedsTailnet' });
  });

  it('allows Tailscale SSH on a tailnet address', () => {
    const errors = validateServerForm(
      form({ auth: 'tailscale', host: '100.64.0.31' }),
      { existingNames: [] },
    );
    expect(errors.auth).toBeUndefined();
  });

  it('rejects Tailscale SSH through a jump host', () => {
    const errors = validateServerForm(
      form({
        auth: 'tailscale',
        host: '100.64.0.31',
        jumpOn: true,
        jump: 'jump-01',
      }),
      { existingNames: [] },
    );
    expect(errors.auth).toBe('tailscaleNeedsTailnet');
  });

  it('requires a usable jump host when enabled', () => {
    expect(
      validateServerForm(form({ jumpOn: true, jump: '' }), {
        existingNames: [],
      }),
    ).toEqual({ jump: 'required' });
  });

  it('flags an invalid jump address unless it names a saved server', () => {
    const bad = form({ jumpOn: true, jump: '跳板 机' });
    expect(validateServerForm(bad, { existingNames: [] })).toEqual({
      jump: 'badHost',
    });
    expect(
      validateServerForm(bad, { existingNames: [], jumpNames: ['跳板 机'] }),
    ).toEqual({});
  });

  it('asks for a key only when the phone is known to have none', () => {
    expect(
      validateServerForm(form(), { existingNames: [], hasKey: false }),
    ).toEqual({ auth: 'noKey' });
    expect(
      validateServerForm(form(), { existingNames: [], hasKey: true }),
    ).toEqual({});
    expect(validateServerForm(form(), { existingNames: [] })).toEqual({});
    expect(
      validateServerForm(form({ auth: 'password' }), {
        existingNames: [],
        hasKey: false,
      }),
    ).toEqual({});
  });

  it('rejects a duplicate name case-insensitively, except when editing it', () => {
    const current = form({ name: 'BUILD-SRV' });
    expect(
      validateServerForm(current, { existingNames: ['build-srv'] }),
    ).toEqual({ name: 'duplicateName' });
    expect(
      validateServerForm(current, {
        existingNames: ['build-srv'],
        editingName: 'build-srv',
      }),
    ).toEqual({});
  });
});

describe('network hints', () => {
  it('finds the network containing an IPv4 host', () => {
    expect(requiredNetwork('10.30.20.31', networks)).toBe(networks[0]);
    expect(requiredNetwork('192.168.31.20', networks)).toBeNull();
    expect(requiredNetwork('build.corp.example.com', networks)).toBeNull();
  });

  it('suggests a jump host only before one is enabled', () => {
    expect(shouldSuggestJump(form(), networks)).toBe(true);
    expect(shouldSuggestJump(form({ jumpOn: true }), networks)).toBe(false);
  });
});

describe('parseHostPort', () => {
  it('parses explicit and default ports', () => {
    expect(parseHostPort('10.30.0.5:22', 22)).toEqual({
      host: '10.30.0.5',
      port: 22,
    });
    expect(parseHostPort('jump-01', 22)).toEqual({
      host: 'jump-01',
      port: 22,
    });
  });

  it('rejects invalid ports and hosts', () => {
    expect(parseHostPort('jump-01:99999', 22)).toBeNull();
    expect(parseHostPort('a b:22', 22)).toBeNull();
    expect(parseHostPort('jump-01', 0)).toBeNull();
    expect(parseHostPort('', 22)).toBeNull();
  });

  it('accepts IPv6 bare or in brackets with a port', () => {
    expect(parseHostPort('fd7a:115c:a1e0::3', 22)).toEqual({
      host: 'fd7a:115c:a1e0::3',
      port: 22,
    });
    expect(parseHostPort('[fd7a:115c:a1e0::3]:2222', 22)).toEqual({
      host: 'fd7a:115c:a1e0::3',
      port: 2222,
    });
    expect(parseHostPort('[jump-01]:22', 22)).toBeNull();
    expect(parseHostPort('fd7a::1::2', 22)).toBeNull();
  });
});

describe('form conversion', () => {
  it('builds a server from the form', () => {
    const built = formToServer(form({ name: '', jump: 'jump-01' }), {
      id: 'manual-build-srv',
      nowUs: FIXTURE_NOW,
      networks,
    });

    expect(built).toMatchObject({
      id: 'manual-build-srv',
      kind: 'ssh',
      name: '10.30.20.31',
      host: '10.30.20.31',
      port: 22,
      user: 'alice',
      jump: null,
      networkId: 'n1',
      addedAtUs: FIXTURE_NOW,
      updatedAtUs: FIXTURE_NOW,
    });
  });

  it('keeps the original addition time while updating', () => {
    const built = formToServer(form(), {
      id: 'manual-build-srv',
      nowUs: FIXTURE_NOW,
      addedAtUs: FIXTURE_NOW - 1000,
      networks,
    });
    expect(built.addedAtUs).toBe(FIXTURE_NOW - 1000);
  });

  it('maps a fixture server back into an editable form', () => {
    const manual = sshServers.find(
      candidate => candidate.serverId === 'manual-build-srv',
    );
    expect(manual).toBeDefined();
    expect(serverToForm(manual!)).toMatchObject({
      name: 'build-srv',
      user: 'alice',
      host: '10.30.20.31',
      port: '22',
      auth: 'password',
      jumpOn: true,
      jump: 'jump-01',
    });
  });
});

describe('connection testing', () => {
  it('tests the target directly without a jump host', () => {
    expect(testTarget(form())).toEqual({
      host: '10.30.20.31',
      port: 22,
      via: null,
    });
  });

  it('tests the jump host when one is enabled', () => {
    expect(testTarget(form({ jumpOn: true, jump: 'jump-01' }))).toEqual({
      host: 'jump-01',
      port: 22,
      via: 'jump-01',
    });
  });

  it('returns null for an invalid target or jump host', () => {
    expect(testTarget(form({ host: '10.30.300.1' }))).toBeNull();
    expect(testTarget(form({ port: '0' }))).toBeNull();
    expect(testTarget(form({ user: '' }))).toBeNull();
    expect(testTarget(form({ jumpOn: true, jump: '' }))).toBeNull();
  });

  it('maps successful and failed probes', () => {
    expect(testFromProbe(probe('ok', 12), 'jump-01')).toEqual({
      kind: 'ok',
      ms: 12,
      via: 'jump-01',
    });
    expect(testFromProbe(probe('timeout', null), null)).toEqual({
      kind: 'fail',
      reason: 'timeout',
      via: null,
    });
  });

  it('maps TCP probe results onto existing failure reasons', () => {
    const tcp = (outcome: DiscoverOutcome) => ({
      addr: '10.30.20.31',
      port: 22,
      outcome,
      ms: 7,
    });
    expect(testFromDiscover(tcp('open'), 'jump-01')).toEqual({
      kind: 'ok',
      ms: 7,
      via: 'jump-01',
    });
    expect(testFromDiscover(tcp('refused'), null)).toEqual({
      kind: 'fail',
      reason: 'refused',
      via: null,
    });
    expect(testFromDiscover(tcp('timeout'), null)).toEqual({
      kind: 'fail',
      reason: 'timeout',
      via: null,
    });
    expect(testFromDiscover(tcp('mismatch'), null)).toEqual({
      kind: 'fail',
      reason: 'refused',
      via: null,
    });
  });
});
