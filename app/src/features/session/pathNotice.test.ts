import type { NetworkEntry } from '../settings/schema';
import { hudCaption, noticeFor } from './pathNotice';

const networks: NetworkEntry[] = [
  { id: 'company', name: '公司 VPN', cidrs: ['10.30.0.0/16'], vpnPools: [] },
];

describe('hudCaption', () => {
  it('always asks the relay path to keep probing for a direct route', () => {
    expect(
      hudCaption({
        relay: true,
        tcp: false,
        path: 'tailnetDerp',
        addr: '100.64.0.7',
        networks,
      }),
    ).toBe('后台继续尝试直连');
  });

  it('names the configured network that owns the address', () => {
    expect(
      hudCaption({
        relay: false,
        tcp: true,
        path: 'intranet',
        addr: '10.30.12.48',
        networks,
      }),
    ).toBe('经公司 VPN · 10.30.12.48');
  });

  it('falls back to the generic intranet label when the address is unknown', () => {
    expect(
      hudCaption({
        relay: false,
        tcp: true,
        path: 'intranet',
        addr: '10.30.12.48',
        networks: [],
      }),
    ).toBe('经内网 · 10.30.12.48');
  });

  it('returns null without relay or TCP', () => {
    expect(
      hudCaption({
        relay: false,
        tcp: false,
        path: 'tailnetDirect',
        addr: '100.64.0.7',
        networks,
      }),
    ).toBeNull();
  });

  it('does not label a TCP address outside an intranet path as intranet', () => {
    expect(
      hudCaption({
        relay: false,
        tcp: true,
        path: 'manual',
        addr: '203.0.113.5',
        networks,
      }),
    ).toBeNull();
  });
});

describe('noticeFor', () => {
  it('describes a relay downgrade with the region and current quality', () => {
    expect(
      noticeFor('relay', {
        region: '上海',
        height: 1080,
        fps: 30,
        intranet: false,
      }),
    ).toEqual({
      title: '经上海中继转发，画质已自动降低',
      body: '能直接连上电脑时会自动换回来，不用手动切',
    });
  });

  it('uses the automatic downgrade copy without statistics', () => {
    expect(
      noticeFor('relay', {
        region: '上海',
        height: null,
        fps: null,
        intranet: false,
      }).title,
    ).toBe('经上海中继转发，画质会自动降低');
  });

  it('describes TCP on the company network', () => {
    expect(
      noticeFor('tcp', {
        region: null,
        height: null,
        fps: null,
        intranet: true,
      }),
    ).toEqual({
      title: '公司网络限制了快速通道，已换成备用连接',
      body: '画面会比平时糊一点，换个网络后会自动换回来',
    });
  });

  it('describes TCP outside the company network', () => {
    expect(
      noticeFor('tcp', {
        region: null,
        height: null,
        fps: null,
        intranet: false,
      }).title,
    ).toBe('当前网络限制了快速通道，已换成备用连接');
  });
});
