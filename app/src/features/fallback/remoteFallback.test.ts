import { Platform } from 'react-native';
import {
  FALLBACK_APPS,
  fallbackKind,
  fallbackUrl,
  pickAddress,
  rdpUrl,
  storeUrl,
  storeUrls,
  urlHost,
  vncUrl,
} from './remoteFallback';

afterEach(() => {
  jest.restoreAllMocks();
});

describe('remoteFallback URLs', () => {
  it('builds an RDP URL with the default port', () => {
    expect(rdpUrl('100.64.0.7')).toBe('rdp://full%20address=s:100.64.0.7:3389');
  });

  it('brackets IPv6 VNC hosts and keeps a non-default port', () => {
    expect(vncUrl('fd7a:115c::7')).toBe('vnc://[fd7a:115c::7]');
    expect(vncUrl('[fd7a:115c::7]', 5901)).toBe('vnc://[fd7a:115c::7]:5901');
    expect(vncUrl('100.64.0.7', 5900)).toBe('vnc://100.64.0.7');
  });

  it('escapes zone IDs in RDP IPv6 addresses', () => {
    expect(rdpUrl('fe80::1%en0')).toBe(
      'rdp://full%20address=s:[fe80::1%25en0]:3389',
    );
  });

  it('removes existing brackets before normalizing a host', () => {
    expect(urlHost('[100.64.0.7]')).toBe('100.64.0.7');
    expect(urlHost('[fe80::1%en0]')).toBe('[fe80::1%25en0]');
  });

  it('selects the matching protocol for an OS and dispatches its URL', () => {
    expect(fallbackKind('windows')).toBe('rdp');
    expect(fallbackKind('macos')).toBe('vnc');
    expect(fallbackUrl('rdp', '100.64.0.7')).toBe(
      'rdp://full%20address=s:100.64.0.7:3389',
    );
    expect(fallbackUrl('vnc', '100.64.0.7')).toBe('vnc://100.64.0.7');
  });
});

describe('pickAddress', () => {
  it('falls back through tailnet, intranet and lan', () => {
    expect(
      pickAddress([
        { kind: 'lan', addr: '192.168.31.20', port: 7400 },
        { kind: 'tailnet', addr: '100.64.0.7', port: 7400 },
      ]),
    ).toBe('100.64.0.7');
    expect(
      pickAddress([
        { kind: 'lan', addr: '192.168.31.20', port: 7400 },
        { kind: 'intranet', addr: '10.30.12.48', port: 7400 },
      ]),
    ).toBe('10.30.12.48');
    expect(
      pickAddress([{ kind: 'lan', addr: '192.168.31.20', port: 7400 }]),
    ).toBe('192.168.31.20');
    expect(pickAddress([])).toBeNull();
  });
});

describe('storeUrl', () => {
  it('targets the verified App Store listings', () => {
    expect(storeUrl('rdp')).toBe('itms-apps://apps.apple.com/app/id714464092');
    expect(storeUrl('vnc')).toBe('itms-apps://apps.apple.com/app/id352019548');
    expect(FALLBACK_APPS.vnc.name).toBe('VNC Viewer');
  });
});

describe('storeUrls', () => {
  it('returns the iOS listing for each fallback app', () => {
    expect(storeUrls('rdp', 'ios')).toEqual([
      'itms-apps://apps.apple.com/app/id714464092',
    ]);
    expect(storeUrls('vnc', 'ios')).toEqual([
      'itms-apps://apps.apple.com/app/id352019548',
    ]);
  });

  it('returns the matching Play Store listings on Android', () => {
    expect(storeUrls('rdp', 'android')).toEqual([
      'market://details?id=com.microsoft.rdc.androidx',
      'https://play.google.com/store/apps/details?id=com.microsoft.rdc.androidx',
    ]);
    expect(storeUrls('vnc', 'android')).toEqual([
      'market://details?id=com.realvnc.viewer.android',
      'https://play.google.com/store/apps/details?id=com.realvnc.viewer.android',
    ]);
  });

  it('uses the current platform when no OS is supplied', () => {
    jest.replaceProperty(Platform, 'OS', 'android');
    expect(FALLBACK_APPS.rdp.androidPackage).toBe('com.microsoft.rdc.androidx');
    expect(storeUrls('vnc')[0]).toBe(
      'market://details?id=com.realvnc.viewer.android',
    );
  });
});
