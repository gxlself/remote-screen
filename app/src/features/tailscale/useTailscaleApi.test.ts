import { tailscaleApi as copy } from '../../i18n/zh/tailscaleApi';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import type { TailscaleApiStatus } from '../../native/rscore/types';
import { controlHost, errorText, statusText } from './useTailscaleApi';

function status(overrides: Partial<TailscaleApiStatus>): TailscaleApiStatus {
  return {
    configured: true,
    controlUrl: null,
    lastError: null,
    lastOkAt: 1,
    deviceCount: 3,
    ...overrides,
  };
}

describe('statusText', () => {
  it('names every token state', () => {
    expect(statusText(undefined)).toBe('未设置');
    expect(statusText(status({ configured: false, deviceCount: null }))).toBe(
      '未设置',
    );
    expect(statusText(status({}))).toBe('已连接 · 3 台设备');
    expect(statusText(status({ lastError: 'auth' }))).toBe('令牌无效或已过期');
    expect(statusText(status({ lastError: 'network' }))).toBe(
      '连不上 Tailscale',
    );
    expect(statusText(status({ lastError: 'http' }))).toBe('请求失败');
    expect(statusText(status({ lastOkAt: null, deviceCount: null }))).toBe(
      copy.row.configured,
    );
  });

  it('names the Headscale server in front of the state', () => {
    const headscale = (overrides: Partial<TailscaleApiStatus>) =>
      statusText(
        status({ controlUrl: 'https://hs.example.com:8443', ...overrides }),
      );
    expect(headscale({})).toBe('hs.example.com · 已连接 · 3 台设备');
    expect(headscale({ lastError: 'network' })).toBe(
      'hs.example.com · 连不上控制服务器',
    );
    expect(headscale({ lastError: 'auth' })).toBe(
      'hs.example.com · 令牌无效或已过期',
    );
    expect(headscale({ configured: false })).toBe('未设置');
  });

  it('treats a status without controlUrl from an older package as Tailscale', () => {
    const legacy = { ...status({}), controlUrl: undefined };
    expect(statusText(legacy as unknown as TailscaleApiStatus)).toBe(
      '已连接 · 3 台设备',
    );
  });
});

describe('controlHost', () => {
  it('keeps only the host name', () => {
    expect(controlHost('https://hs.example.com')).toBe('hs.example.com');
    expect(controlHost('https://hs.example.com:8443')).toBe('hs.example.com');
    expect(controlHost('https://[fd7a::1]:8443')).toBe('[fd7a::1]');
  });
});

describe('errorText', () => {
  it('maps 401 and 403 to the token message', () => {
    for (const code of [401, 403]) {
      expect(errorText(new RsCoreError('http', 'unauthorized', code))).toBe(
        copy.sheet.errors.auth,
      );
    }
  });

  it('maps network failures to the api.tailscale.com message', () => {
    expect(errorText(new RsCoreError('network', 'timed out'))).toBe(
      '连不上 api.tailscale.com，检查网络',
    );
  });

  it('uses the Headscale wording for a Headscale server', () => {
    expect(
      errorText(new RsCoreError('http', 'unauthorized', 401), true),
    ).toBe('API key 无效或已过期');
    expect(errorText(new RsCoreError('network', 'timed out'), true)).toBe(
      '连不上控制服务器，检查网络和地址',
    );
  });

  it('maps the bad control url error from Rust', () => {
    for (const headscale of [true, false]) {
      expect(
        errorText(new RsCoreError('internal', 'bad control url'), headscale),
      ).toBe('控制服务器地址不对');
    }
  });

  it('falls back to the generic message', () => {
    expect(errorText(new RsCoreError('http', 'bad gateway', 502))).toBe(
      copy.sheet.errors.generic,
    );
    expect(errorText(new RsCoreError('internal', 'bad token'))).toBe(
      copy.sheet.errors.generic,
    );
    expect(errorText(new Error('boom'))).toBe(copy.sheet.errors.generic);
  });
});
