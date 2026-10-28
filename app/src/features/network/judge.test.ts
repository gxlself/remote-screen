import type { NetworkStatus } from '../../native/rscore/types';
import {
  netCompanyVpn,
  netCompanyVpnEmbedded,
  netNoTailnet,
  netTailnetOk,
  routeEmbeddedNeedsLogin,
  routeEmbeddedRunning,
} from '../../native/rscore/fixtures';
import { resetTailnetSwitchStore } from '../tailnet/switchStore';
import type { NetworkEntry } from '../settings/schema';
import { judgeNetwork, localHostAddrs } from './judge';

const companyNetwork: NetworkEntry = {
  id: 'company',
  name: '公司网络',
  cidrs: ['10.30.0.0/16'],
  vpnPools: ['10.99.0.0/16'],
};

const noNetworks: NetworkEntry[] = [];

describe('localHostAddrs', () => {
  it('removes interface prefixes and handles unavailable status', () => {
    const status: NetworkStatus = {
      ifaces: [
        {
          name: 'en0',
          addrs: ['192.168.31.45/24', '192.168.31.46'],
          isVpn: false,
        },
        { name: 'utun4', addrs: ['100.64.0.21/32'], isVpn: true },
      ],
    };

    expect(localHostAddrs(status)).toEqual([
      '192.168.31.45',
      '192.168.31.46',
      '100.64.0.21',
    ]);
    expect(localHostAddrs(null)).toEqual([]);
  });
});

describe('judgeNetwork decision table', () => {
  it('reports unknown when native status is unavailable', () => {
    expect(judgeNetwork(null, [companyNetwork])).toEqual({
      tailnet: 'unknown',
      tailnetAddr: null,
      companyVpn: null,
      atCompany: null,
      banner: 'none',
    });
  });

  it('reports tailnet up when a 100.64/10 address exists', () => {
    const verdict = judgeNetwork(netTailnetOk, noNetworks);
    expect(verdict.tailnet).toBe('up');
    expect(verdict.tailnetAddr).toBe('100.64.0.21');
    expect(verdict.banner).toBe('none');
  });

  it('reports a company VPN conflict when the VPN pool matches', () => {
    const verdict = judgeNetwork(netCompanyVpn, [companyNetwork]);
    expect(verdict.tailnet).toBe('down');
    expect(verdict.companyVpn).toEqual({
      network: companyNetwork,
      addr: '10.99.4.27',
    });
    expect(verdict.banner).toBe('vpnConflict');
  });

  it('reports tailnet down when neither tailnet nor company VPN is present', () => {
    const verdict = judgeNetwork(netNoTailnet, [companyNetwork]);
    expect(verdict.tailnet).toBe('down');
    expect(verdict.companyVpn).toBeNull();
    expect(verdict.banner).toBe('tailnetDown');
  });

  it('reports atCompany without changing tailnet or the banner', () => {
    const status: NetworkStatus = {
      ifaces: [
        { name: 'utun4', addrs: ['100.64.0.21'], isVpn: true },
        { name: 'en0', addrs: ['10.30.5.6'], isVpn: false },
      ],
    };
    const verdict = judgeNetwork(status, [companyNetwork]);
    expect(verdict.tailnet).toBe('up');
    expect(verdict.banner).toBe('none');
    expect(verdict.atCompany).toBe(companyNetwork);
  });

  it('reports atCompany when only en0 is at the office', () => {
    const status: NetworkStatus = {
      ifaces: [{ name: 'en0', addrs: ['10.30.5.6'], isVpn: false }],
    };
    const verdict = judgeNetwork(status, [companyNetwork]);
    expect(verdict.atCompany?.name).toBe('公司网络');
  });

  it('ignores a work-profile VPN for the company VPN conflict', () => {
    const status: NetworkStatus = {
      ifaces: [
        {
          name: 'utun5',
          addrs: ['10.99.4.27'],
          isVpn: true,
          workProfile: true,
        },
        { name: 'en0', addrs: ['192.168.31.45'], isVpn: false },
      ],
    };
    const verdict = judgeNetwork(status, [companyNetwork]);
    expect(verdict.companyVpn).toBeNull();
    expect(verdict.banner).toBe('tailnetDown');
  });

  it('accepts the ip/prefix addresses reported by the real core', () => {
    const status: NetworkStatus = {
      ifaces: [
        { name: 'en0', addrs: ['10.30.5.6/16'], isVpn: false },
        { name: 'utun12', addrs: ['100.64.0.14/32'], isVpn: false },
        { name: 'utun13', addrs: ['10.99.4.27/32'], isVpn: true },
      ],
    };
    const verdict = judgeNetwork(status, [companyNetwork]);
    expect(verdict.tailnet).toBe('up');
    expect(verdict.tailnetAddr).toBe('100.64.0.14');
    expect(verdict.companyVpn).toEqual({
      network: companyNetwork,
      addr: '10.99.4.27',
    });
    expect(verdict.atCompany).toBe(companyNetwork);
    expect(verdict.banner).toBe('none');
  });
});

describe('judgeNetwork fixtures', () => {
  it.each([
    ['netTailnetOk', netTailnetOk, 'up', 'none'],
    ['netCompanyVpn', netCompanyVpn, 'down', 'vpnConflict'],
    ['netNoTailnet', netNoTailnet, 'down', 'tailnetDown'],
  ] as const)('%s maps to %s/%s', (_name, status, tailnet, banner) => {
    const verdict = judgeNetwork(status, [companyNetwork]);
    expect(verdict.tailnet).toBe(tailnet);
    expect(verdict.banner).toBe(banner);
  });
});

describe('judgeNetwork embedded tailnet', () => {
  beforeEach(() => resetTailnetSwitchStore());

  it('keeps the company VPN while using the running embedded node', () => {
    const verdict = judgeNetwork(netCompanyVpnEmbedded, [companyNetwork]);
    expect(verdict).toMatchObject({
      tailnet: 'up',
      tailnetAddr: '100.64.0.21',
      banner: 'none',
      via: 'embedded',
    });
    expect(verdict.companyVpn?.addr).toBe('10.99.4.27');
  });

  it.each(['needsLogin', 'needsMachineAuth'] as const)(
    'asks for embedded login for %s',
    state => {
      const verdict = judgeNetwork(
        {
          ...netCompanyVpn,
          tailnet: {
            ...routeEmbeddedNeedsLogin,
            embedded: { ...routeEmbeddedNeedsLogin.embedded!, state },
          },
        },
        [companyNetwork],
      );
      expect(verdict).toMatchObject({
        tailnet: 'down',
        tailnetAddr: null,
        banner: 'embeddedLogin',
        via: 'embedded',
      });
    },
  );

  it.each([
    ['noState', 'unknown'],
    ['starting', 'unknown'],
    ['running', 'unknown'],
    ['stopped', 'down'],
    ['unknown', 'down'],
    ['unavailable', 'down'],
  ] as const)('reports %s as %s without a banner', (state, tailnet) => {
    expect(
      judgeNetwork(
        {
          ...netCompanyVpn,
          tailnet: {
            ...routeEmbeddedRunning,
            embedded: { ...routeEmbeddedRunning.embedded!, state, ips: [] },
          },
        },
        [companyNetwork],
      ),
    ).toMatchObject({
      tailnet,
      tailnetAddr: null,
      banner: 'none',
      via: 'embedded',
    });
  });

  it('waits without a conflict when the embedded summary is missing', () => {
    expect(
      judgeNetwork(
        {
          ...netCompanyVpn,
          tailnet: { ...routeEmbeddedRunning, embedded: null },
        },
        [companyNetwork],
      ),
    ).toMatchObject({
      tailnet: 'unknown',
      banner: 'none',
      via: 'embedded',
    });
  });

  it('keeps the system VPN conflict when the system route is active', () => {
    expect(
      judgeNetwork(
        {
          ...netCompanyVpn,
          tailnet: {
            ...routeEmbeddedRunning,
            active: 'system',
            embedded: null,
          },
        },
        [companyNetwork],
      ),
    ).toMatchObject({ banner: 'vpnConflict', via: 'system' });
  });

  it('labels an online system route only when a summary is present', () => {
    expect(
      judgeNetwork(
        {
          ...netTailnetOk,
          tailnet: { ...routeEmbeddedRunning, active: 'system' },
        },
        [companyNetwork],
      ),
    ).toMatchObject({ tailnet: 'up', banner: 'none', via: 'system' });
    expect(judgeNetwork(netTailnetOk, noNetworks)).not.toHaveProperty('via');
    expect(judgeNetwork(netCompanyVpn, noNetworks)).not.toHaveProperty('via');
  });

  it('preserves the office network and prefers the embedded node address', () => {
    const verdict = judgeNetwork(
      {
        ...netCompanyVpnEmbedded,
        ifaces: [
          ...netCompanyVpnEmbedded.ifaces,
          { name: 'en1', addrs: ['10.30.5.6'], isVpn: false },
          { name: 'utun4', addrs: ['100.64.0.7'], isVpn: true },
        ],
      },
      [companyNetwork],
    );
    expect(verdict.atCompany).toBe(companyNetwork);
    expect(verdict.tailnetAddr).toBe('100.64.0.21');
  });
});
