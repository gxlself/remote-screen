import { computers } from '../../native/rscore/fixtures';
import { MIN_HOST_VERSION } from '../../native/rscore/fixtures';
import { versionInfo } from './versionInfo';

const home = computers.find(computer => computer.hostId === 'd2n6c4v8b1x5za7e');

if (home === undefined) {
  throw new Error('home fixture missing');
}

const onlineHome = {
  ...home,
  online: true,
  hostRunning: true,
  update: { ...home.update, available: '0.3.1' },
};

describe('versionInfo', () => {
  it('allows an online host with a newer available release to upgrade', () => {
    expect(
      versionInfo({
        code: 170,
        host: onlineHome,
        appVersion: '0.3.1',
        minHost: MIN_HOST_VERSION,
      }),
    ).toMatchObject({
      hostVersion: '0.2.4',
      minHost: '0.3.0',
      target: '0.3.1',
      minApp: null,
      canUpgrade: true,
    });
  });

  it('does not allow an offline host to upgrade', () => {
    expect(
      versionInfo({
        code: 170,
        host: { ...onlineHome, online: false },
        appVersion: '0.3.1',
        minHost: MIN_HOST_VERSION,
      }).canUpgrade,
    ).toBe(false);
  });

  it('does not allow an upgrade when no release is available', () => {
    expect(
      versionInfo({
        code: 170,
        host: { ...onlineHome, update: { channel: 'stable', available: null } },
        appVersion: '0.3.1',
        minHost: MIN_HOST_VERSION,
      }).canUpgrade,
    ).toBe(false);
  });

  it('prefers detail values and rejects a target below the minimum host', () => {
    const info = versionInfo({
      code: 170,
      host: {
        ...onlineHome,
        update: { channel: 'stable', available: '0.2.9' },
      },
      detail: { host_version: '0.2.0', min_version: '0.3.0' },
      appVersion: '0.3.1',
      minHost: MIN_HOST_VERSION,
    });
    expect(info.hostVersion).toBe('0.2.0');
    expect(info.minHost).toBe('0.3.0');
    expect(info.canUpgrade).toBe(false);
  });

  it('never upgrades for the App-too-old variant', () => {
    expect(
      versionInfo({
        code: 171,
        host: onlineHome,
        detail: { min_app_version: '0.3.5' },
        appVersion: '0.3.1',
        minHost: MIN_HOST_VERSION,
      }),
    ).toMatchObject({
      minApp: '0.3.5',
      canUpgrade: false,
    });
  });
});
