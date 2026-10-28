import { FIXTURE_NOW } from '../../native/rscore/fixtures';
import {
  EMPTY_SERVERS,
  manualComputers,
  manualServers,
  newComputerId,
  newServerId,
  parseServersValue,
  removeEntry,
  upsertEntry,
} from './manual';
import type { ManualComputer, ManualServer, ServersValue } from './manual';

const server: ManualServer = {
  id: 'manual-build-srv',
  kind: 'ssh',
  name: 'build-srv',
  host: '10.30.20.31',
  port: 22,
  user: 'alice',
  auth: 'password',
  jump: 'jump-01',
  networkId: 'n1',
  addedAtUs: FIXTURE_NOW - 1000,
  updatedAtUs: FIXTURE_NOW,
};

const computer: ManualComputer = {
  id: 'addr-10-30-12-48-7400',
  kind: 'computer',
  host: '10.30.12.48',
  port: 7400,
  hostId: null,
  name: null,
  networkId: 'n1',
  addedAtUs: FIXTURE_NOW,
};

function value(entries: ServersValue['entries']): ServersValue {
  return { v: 1, entries, tailscaleSsh: {} };
}

describe('parseServersValue', () => {
  it.each([
    ['macos', 'vnc'],
    ['windows', 'rdp'],
  ])('preserves native computer fields %s / %s', (os, protocol) => {
    const entry = { ...computer, os, protocol };
    const parsed = parseServersValue({ v: 1, entries: [entry] });
    expect(parsed.entries).toEqual([entry]);
  });

  it.each([
    { os: 'linux', protocol: 'ssh' },
    { os: null, protocol: 3389 },
    { os: {}, protocol: [] },
  ])('drops invalid native fields without dropping the computer: %p', fields => {
    const parsed = parseServersValue({ v: 1, entries: [{ ...computer, ...fields }] });
    expect(parsed.entries).toEqual([computer]);
    expect(parsed.entries[0]).not.toHaveProperty('os');
    expect(parsed.entries[0]).not.toHaveProperty('protocol');
  });

  it('validates each optional native field independently', () => {
    const parsed = parseServersValue({ v: 1, entries: [
      { ...computer, os: 'macos', protocol: 'ssh' },
      { ...computer, os: 'linux', protocol: 'rdp' },
    ] });
    expect(parsed.entries).toEqual([
      { ...computer, os: 'macos' },
      { ...computer, protocol: 'rdp' },
    ]);
  });

  it('parses legacy computers without adding optional fields', () => {
    expect(parseServersValue(value([computer]))).toEqual(value([computer]));
  });

  it.each([null, 'x', { v: 2, entries: [] }])(
    'returns EMPTY_SERVERS for %p',
    raw => {
      expect(parseServersValue(raw)).toEqual(EMPTY_SERVERS);
    },
  );

  it('keeps valid entries, drops malformed entries, and filters tailscaleSsh', () => {
    const parsed = parseServersValue({
      v: 1,
      entries: [
        server,
        { ...server, id: 'missing-host', host: undefined },
        { ...computer, id: 'printer', kind: 'printer' },
        computer,
      ],
      tailscaleSsh: { 'build-srv': true, 'other-srv': 'yes' },
    });

    expect(parsed.entries).toEqual([server, computer]);
    expect(parsed.tailscaleSsh).toEqual({ 'build-srv': true });
  });

  it('returns an empty tailscale map when that field is not an object', () => {
    expect(
      parseServersValue({ v: 1, entries: [], tailscaleSsh: [] }).tailscaleSsh,
    ).toEqual({});
  });
});

describe('entry mutations', () => {
  it('appends a new entry without changing the input', () => {
    const original = value([computer]);
    Object.freeze(original.entries);
    Object.freeze(original);

    const next = upsertEntry(original, server);

    expect(next.entries).toHaveLength(2);
    expect(next.entries[1]).toEqual(server);
    expect(original.entries).toEqual([computer]);
  });

  it('replaces an entry with the same id in place', () => {
    const original = value([server, computer]);
    const updated = {
      ...server,
      name: 'renamed',
      updatedAtUs: FIXTURE_NOW + 1,
    };

    const next = upsertEntry(original, updated);

    expect(next.entries).toEqual([updated, computer]);
    expect(original.entries).toEqual([server, computer]);
  });

  it('removes an entry and leaves an unknown id unchanged', () => {
    const original = value([server, computer]);

    expect(removeEntry(original, server.id)).toEqual(value([computer]));
    expect(removeEntry(original, 'missing')).toEqual(original);
  });
});

describe('generated ids', () => {
  it('slugs server names and adds a numeric suffix when taken', () => {
    expect(newServerId('build-srv', [])).toBe('manual-build-srv');
    expect(newServerId('build-srv', ['manual-build-srv'])).toBe(
      'manual-build-srv-2',
    );
    expect(
      newServerId('build-srv', ['manual-build-srv', 'manual-build-srv-2']),
    ).toBe('manual-build-srv-3');
    expect(newServerId('  ', [])).toBe('manual-server');
  });

  it('normalizes a computer address into an id', () => {
    expect(newComputerId('10.30.12.48', 7400, [])).toBe(
      'addr-10-30-12-48-7400',
    );
    expect(newComputerId('10.30.12.48', 7400, ['addr-10-30-12-48-7400'])).toBe(
      'addr-10-30-12-48-7400-2',
    );
  });
});

it('filters servers and computers by kind', () => {
  const entries = value([server, computer]);

  expect(manualServers(entries)).toEqual([server]);
  expect(manualComputers(entries)).toEqual([computer]);
});
