import { FakePlatform, createFakePlatform } from './fakePlatform';

describe('FakePlatform', () => {
  it('records copied text', async () => {
    const platform = new FakePlatform();

    await platform.copyText('a');
    expect(platform.copied).toEqual(['a']);
    expect(platform.calls).toContainEqual({ method: 'copyText', args: ['a'] });
  });

  it('honours the configured authentication result', async () => {
    const platform = createFakePlatform();
    platform.authResult = 'cancelled';

    await expect(platform.authenticate('unlock')).resolves.toBe('cancelled');
    expect(platform.calls).toContainEqual({
      method: 'authenticate',
      args: ['unlock'],
    });
  });

  it('reports Face ID by default and records the capability call', () => {
    const platform = createFakePlatform();

    expect(platform.authCapability()).toEqual({
      biometry: 'faceId',
      passcode: true,
    });
    expect(platform.calls).toContainEqual({
      method: 'authCapability',
      args: [],
    });
  });

  it('honours the configured authentication capability', () => {
    const platform = createFakePlatform();
    platform.authCapabilityResult = { biometry: 'touchId', passcode: false };

    expect(platform.authCapability()).toEqual({
      biometry: 'touchId',
      passcode: false,
    });
    expect(platform.calls).toContainEqual({
      method: 'authCapability',
      args: [],
    });
  });

  it('records opened URLs and returns the configured result', async () => {
    const platform = new FakePlatform();
    platform.openUrlResult = false;

    await expect(platform.openUrl('vnc://100.64.0.7')).resolves.toBe(false);
    expect(platform.opened).toEqual(['vnc://100.64.0.7']);
    await expect(platform.canOpenUrl('vnc://100.64.0.7')).resolves.toBe(false);
  });

  it('returns clipboard text and makes info configurable', async () => {
    const platform = new FakePlatform();
    expect(await platform.readText()).toBeNull();
    platform.clipboardText = 'from clipboard';
    await expect(platform.readText()).resolves.toBe('from clipboard');

    platform.info = {
      platform: 'ipados',
      isPad: true,
      model: 'iPad',
      osVersion: '26.1',
      appVersion: '0.3.1',
    };
    expect(platform.deviceInfo()).toEqual(platform.info);
    expect(platform.calls).toContainEqual({ method: 'readText', args: [] });
  });

  it('keeps the remaining platform calls observable', async () => {
    const platform = createFakePlatform();

    await platform.share({ text: 'key', title: 'SSH' });
    platform.haptic('success');
    await expect(platform.requestNotifications()).resolves.toBe('granted');
    await expect(platform.requestLocalNetwork()).resolves.toBe('granted');
    expect(platform.calls.map(call => call.method)).toEqual([
      'share',
      'haptic',
      'requestNotifications',
      'requestLocalNetwork',
    ]);
  });
});
