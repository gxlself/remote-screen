import {
  defaultKeyName,
  installRows,
  keyCreatedOn,
  keyKindLabel,
  shortFp,
  trustedRows,
} from './keysView';
import {
  sshKeys,
  sshServers,
  trustedHostKeys,
} from '../../native/rscore/fixtures';

describe('keyKindLabel', () => {
  it.each([
    ['se-p256', 'ECDSA P-256'],
    ['ed25519', 'Ed25519'],
  ] as const)('labels %s', (kind, expected) => {
    expect(keyKindLabel(kind)).toBe(expected);
  });
});

describe('shortFp', () => {
  it('keeps the prefix and both ends of a long fingerprint', () => {
    expect(shortFp('SHA256:nT4kP0aZaaaaaaaaaaaaWq8c')).toBe(
      'SHA256:nT4kP0aZ…Wq8c',
    );
  });

  it('returns a short fingerprint unchanged', () => {
    expect(shortFp('SHA256:short')).toBe('SHA256:short');
  });
});

describe('defaultKeyName', () => {
  it('slugifies the device model', () => {
    expect(defaultKeyName('iPhone SE')).toBe('rs-iphone-se');
  });
});

describe('installRows', () => {
  it('excludes Tailscale SSH servers and marks installed keys', () => {
    const result = installRows(sshServers, sshKeys[0]);

    expect(result.rows.map(row => row.name)).toEqual([
      'gpu-train-02',
      'nas-home',
      'build-srv',
    ]);
    expect(result.rows.map(row => row.installed)).toEqual([true, true, false]);
    expect(result.tailscaleNames).toEqual(['prod-api-01']);
    expect(result.rows[0]?.subtitle).toBe('Ubuntu 22.04 · 密钥登录');
    expect(result.rows[2]?.subtitle).toBe('10.30.20.31 · 经 jump-01');
  });

  it('treats a missing key as not installed', () => {
    expect(
      installRows(sshServers, null).rows.every(row => !row.installed),
    ).toBe(true);
  });
});

describe('trustedRows', () => {
  it('uses server names and sorts by name', () => {
    const rows = trustedRows(
      [...trustedHostKeys].reverse(),
      [...sshServers].reverse(),
    );

    expect(rows.map(row => row.name)).toEqual(['build-srv', 'gpu-train-02']);
    expect(rows[1]).toMatchObject({
      serverId: 'gpu-train-02',
      keyType: 'ED25519',
      short: 'SHA256:CD--4oUk…XYdA',
    });
  });

  it('falls back to the server id when it is not in the directory', () => {
    expect(
      trustedRows([{ ...trustedHostKeys[0]!, serverId: 'unknown-host' }], []),
    ).toEqual([
      expect.objectContaining({
        serverId: 'unknown-host',
        name: 'unknown-host',
      }),
    ]);
  });
});

describe('keyCreatedOn', () => {
  it('formats unix microseconds in local calendar time', () => {
    expect(keyCreatedOn(sshKeys[0]!.createdAt, 480)).toBe('9月2日');
  });
});
