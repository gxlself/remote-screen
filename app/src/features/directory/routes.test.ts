import {
  hostDetail,
  netCompanyVpn,
  netNoTailnet,
  tailnetRunning,
} from '../../native/rscore/fixtures';
import type {
  HostDetail,
  NetPath,
  NetworkStatus,
  Transport,
} from '../../native/rscore/types';
import { deviceRoutes } from '../../i18n/zh/deviceRoutes';
import { judgeNetwork, localHostAddrs } from '../network/judge';
import type { NetworkEntry } from '../settings/schema';
import {
  autoPolicySubtitle,
  buildRouteRows,
  fastestRoute,
  routeAccessibilityLabel,
  summaryText,
} from './routes';
import type { RouteResult } from './pathStore';

const HOST_ID = 'w3k8p2x7m4q9tb6c';
const companyNetwork: NetworkEntry = {
  id: 'company',
  name: '公司网络',
  cidrs: ['10.30.0.0/16'],
  vpnPools: ['10.99.0.0/16'],
};

function result(
  path: NetPath,
  addr: string,
  ms: number | null,
  outcome: RouteResult['outcome'] = 'ok',
  transport: Transport = 'quic',
): RouteResult {
  return {
    path,
    addr,
    port: 7400,
    transport,
    outcome,
    ms,
    atUs: 1,
  };
}

function detail(
  addresses = hostDetail(HOST_ID).addresses,
): Pick<HostDetail, 'node' | 'addresses'> {
  return { node: 'office-win', addresses };
}

function base(overrides: Partial<Parameters<typeof buildRouteRows>[0]> = {}) {
  return {
    detail: detail(),
    verdict: judgeNetwork(netCompanyVpn, [companyNetwork]),
    networks: [companyNetwork],
    localAddrs: ['10.99.4.27', '192.168.31.45'],
    routes: {},
    inUsePath: null,
    testing: false,
    ...overrides,
  } satisfies Parameters<typeof buildRouteRows>[0];
}

describe('buildRouteRows', () => {
  it('orders intranet before tailnet and names the registered network', () => {
    const rows = buildRouteRows(base());
    expect(rows.map(row => row.kind)).toEqual(['intranet', 'tailnet']);
    expect(rows[0]).toMatchObject({
      title: '公司网络',
      addrText: '10.30.12.48:7400',
      subtitle: deviceRoutes.vpnConnected,
      latencyMs: null,
      mark: 'intranet',
      state: 'untested',
    });
  });

  it('falls back to 公司内网 and reports a missing company VPN', () => {
    const rows = buildRouteRows(
      base({
        networks: [],
        verdict: judgeNetwork(netNoTailnet, []),
      }),
    );
    expect(rows[0].title).toBe('公司内网');
    expect(rows[0].subtitle).toBe(deviceRoutes.vpnDisconnected);
  });

  it('shows the intranet latency and relay mark', () => {
    const rows = buildRouteRows(
      base({
        routes: {
          '10.30.12.48:7400': result('intranet', '10.30.12.48', 23),
          '100.64.0.12:7400': result('tailnetDerp', '100.64.0.12', 48),
        },
        derpRegion: '上海',
      }),
    );
    expect(rows[0].latencyMs).toBe(23);
    expect(rows[0].state).toBe('ok');
    expect(rows[1]).toMatchObject({
      title: 'tailnet',
      addrText: '100.64.0.12',
      subtitle: 'office-win · 经上海中继',
      mark: 'tailnetDerp',
      latencyMs: 48,
      state: 'ok',
    });
  });

  it('uses the direct tailnet subtitle when a probe succeeds', () => {
    const rows = buildRouteRows(
      base({
        routes: {
          '100.64.0.12:7400': result('tailnetDirect', '100.64.0.12', 12),
        },
      }),
    );
    expect(rows[1].subtitle).toBe('office-win · 直连');
    expect(rows[1].mark).toBe('tailnetDirect');
  });

  it('marks only the path in use when it is reachable', () => {
    const rows = buildRouteRows(
      base({
        routes: {
          '10.30.12.48:7400': result('intranet', '10.30.12.48', 23),
          '100.64.0.12:7400': result('tailnetDirect', '100.64.0.12', 48),
        },
        inUsePath: 'intranet',
      }),
    );
    expect(rows.map(row => row.inUse)).toEqual([true, false]);
  });

  it('does not mark the tailnet row when the intranet path is selected', () => {
    const rows = buildRouteRows(
      base({
        routes: {
          '10.30.12.48:7400': result('intranet', '10.30.12.48', 23),
          '100.64.0.12:7400': result('tailnetDerp', '100.64.0.12', 48),
        },
        inUsePath: 'tailnetDerp',
        derpRegion: '上海',
      }),
    );
    expect(rows.map(row => row.inUse)).toEqual([false, true]);
  });

  it('reports LAN reachability from local addresses without latency', () => {
    const lan = {
      kind: 'lan' as const,
      addr: '192.168.31.20',
      port: 7400,
      cidr: '192.168.31.0/24',
    };
    const localStatus: NetworkStatus = {
      ifaces: [{ name: 'en0', addrs: ['192.168.31.45/24'], isVpn: false }],
    };
    const rows = buildRouteRows(
      base({
        detail: detail([
          { kind: 'tailnet', addr: '100.64.0.12', port: 7400 },
          lan,
        ]),
        localAddrs: localHostAddrs(localStatus),
      }),
    );
    const lanRow = rows.find(row => row.kind === 'lan');
    expect(lanRow).toMatchObject({
      title: '局域网',
      addrText: '192.168.31.20:7400',
      subtitle: '同一网络',
      latencyMs: null,
      mark: 'lan',
    });

    const away = buildRouteRows(
      base({
        detail: detail([
          { kind: 'tailnet', addr: '100.64.0.12', port: 7400 },
          lan,
        ]),
        localAddrs: ['10.30.5.6'],
      }),
    ).find(row => row.kind === 'lan');
    expect(away).toMatchObject({
      subtitle: '不在同一网络 · 人在工位时才用得上',
      latencyMs: null,
    });
  });

  it('reports a failed probe as unreachable', () => {
    const rows = buildRouteRows(
      base({
        routes: {
          '10.30.12.48:7400': result(
            'intranet',
            '10.30.12.48',
            null,
            'timeout',
          ),
        },
      }),
    );
    expect(rows[0]).toMatchObject({ state: 'unreachable', latencyMs: null });
  });

  it('uses testing only while a row has no result', () => {
    const rows = buildRouteRows(
      base({
        testing: true,
        routes: {
          '10.30.12.48:7400': result('intranet', '10.30.12.48', 23),
        },
      }),
    );
    expect(rows[0].state).toBe('ok');
    expect(rows[1].state).toBe('testing');
  });

  it('keeps a live tailnet session in use without a probe result', () => {
    const rows = buildRouteRows(
      base({
        routes: {
          '100.64.0.12:7400': result(
            'tailnetDirect',
            '100.64.0.12',
            null,
            'timeout',
          ),
        },
        inUsePath: 'tailnetDirect',
        hasLiveSession: true,
      }),
    );
    expect(rows[1]).toMatchObject({
      subtitle: 'office-win · 直连',
      state: 'unreachable',
      inUse: true,
    });
  });

  it('leaves the tailnet subtitle as the node without any result', () => {
    const rows = buildRouteRows(base());
    expect(rows[1]).toMatchObject({
      title: 'tailnet',
      subtitle: 'office-win',
      mark: 'tailnetDirect',
      latencyMs: null,
    });
  });

  it('uses a live session relay region when the probe has no result', () => {
    const rows = buildRouteRows(
      base({
        inUsePath: 'tailnetDerp',
        hasLiveSession: true,
        derpRegion: '上海',
      }),
    );
    expect(rows[1]).toMatchObject({
      subtitle: 'office-win · 经上海中继',
      mark: 'tailnetDerp',
      state: 'untested',
      inUse: true,
    });
  });

  it('marks a relayed embedded peer and keeps the probe latency', () => {
    const rows = buildRouteRows(
      base({
        routes: {
          '100.64.0.12:7400': result('tailnetDirect', '100.64.0.12', 48),
        },
        embedded: { active: true, status: tailnetRunning },
      }),
    );
    expect(rows[1]).toMatchObject({
      title: 'tailnet · App 内',
      mark: 'tailnetDerp',
      subtitle: 'office-win · 经上海中继',
      latencyMs: 48,
    });
  });

  it('marks a directly connected embedded peer', () => {
    const rows = buildRouteRows(
      base({
        detail: {
          node: 'studio-mac',
          addresses: [{ kind: 'tailnet', addr: '100.64.0.7', port: 7400 }],
        },
        embedded: { active: true, status: tailnetRunning },
      }),
    );
    expect(rows[0]).toMatchObject({
      mark: 'tailnetDirect',
      subtitle: 'studio-mac · 直连',
    });
  });

  it('marks an offline embedded peer', () => {
    const rows = buildRouteRows(
      base({
        embedded: {
          active: true,
          status: {
            ...tailnetRunning,
            peers: tailnetRunning.peers.map(peer =>
              peer.ips.includes('100.64.0.12')
                ? { ...peer, online: false }
                : peer,
            ),
          },
        },
      }),
    );
    expect(rows[1]).toMatchObject({
      mark: 'offline',
      subtitle: 'office-win · 对方不在线',
    });
  });

  it('uses the relay code when the embedded region is remote', () => {
    const rows = buildRouteRows(
      base({
        embedded: {
          active: true,
          status: {
            ...tailnetRunning,
            peers: tailnetRunning.peers.map(peer =>
              peer.ips.includes('100.64.0.12')
                ? { ...peer, relay: 'fra' }
                : peer,
            ),
          },
        },
      }),
    );
    expect(rows[1].subtitle).toBe('office-win · 经fra中继');
  });

  it('changes only the title for an address without a peer', () => {
    const input = base({
      detail: {
        node: 'office-win',
        addresses: [{ kind: 'tailnet', addr: '100.64.0.99', port: 7400 }],
      },
      routes: {
        '100.64.0.99:7400': result('tailnetDirect', '100.64.0.99', 48),
      },
    });
    const withoutEmbedded = buildRouteRows(input)[0];
    const withEmbedded = buildRouteRows({
      ...input,
      embedded: { active: true, status: tailnetRunning },
    })[0];
    expect(withEmbedded.title).toBe('tailnet · App 内');
    expect({ ...withEmbedded, title: withoutEmbedded.title }).toEqual(
      withoutEmbedded,
    );
  });

  it('leaves the rows unchanged when embedded routing is inactive', () => {
    const input = base({
      routes: {
        '100.64.0.12:7400': result('tailnetDirect', '100.64.0.12', 48),
      },
    });
    expect(
      buildRouteRows({
        ...input,
        embedded: { active: false, status: tailnetRunning },
      }),
    ).toEqual(buildRouteRows(input));
  });
});

describe('summaryText', () => {
  const rows = buildRouteRows(
    base({
      detail: detail([
        { kind: 'intranet', addr: '10.30.12.48', port: 7400 },
        { kind: 'tailnet', addr: '100.64.0.12', port: 7400 },
      ]),
    }),
  );

  it('counts routes while testing', () => {
    expect(summaryText(rows, true)).toBe('2 个 · 测试中…');
  });

  it('counts routes after testing', () => {
    expect(summaryText(rows, false)).toBe('2 个 · 刚刚测过');
  });
});

describe('route helpers', () => {
  const rows = buildRouteRows(
    base({
      routes: {
        '10.30.12.48:7400': result('intranet', '10.30.12.48', 23),
        '100.64.0.12:7400': result('tailnetDerp', '100.64.0.12', 48),
      },
      derpRegion: '上海',
    }),
  );

  it('selects the fastest reachable route', () => {
    expect(fastestRoute(rows)?.kind).toBe('intranet');
  });

  it('describes the automatic policy from the fastest route', () => {
    expect(autoPolicySubtitle(rows)).toBe('现在是公司内网 23 ms 最快');
  });

  it('describes an untested automatic policy', () => {
    expect(autoPolicySubtitle([])).toBe('测完后显示哪条最快');
  });

  it('builds the spoken row with latency and current state', () => {
    expect(
      routeAccessibilityLabel({
        ...rows[1],
        inUse: true,
      }),
    ).toBe('tailnet，100.64.0.12，office-win · 经上海中继，48 毫秒，当前在用');
  });
});
