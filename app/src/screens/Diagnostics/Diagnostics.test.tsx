import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import type { RootScreenProps } from '../../app/navigation/routes';
import type { NetworkEntry } from '../../features/settings/schema';
import { MemoryKv } from '../../features/storage/kv';
import type { KvStore } from '../../features/storage/kv';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import {
  FIXTURE_NOW,
  netCompanyVpn,
  netCompanyVpnEmbedded,
  routeEmbeddedNeedsLogin,
  tailnetNeedsLogin,
  tailnetRunning,
} from '../../native/rscore/fixtures';
import type {
  CoreEvent,
  NetworkStatus,
  ProbeOutcome,
} from '../../native/rscore/types';
import { diagnostics } from '../../i18n/zh/diagnostics';
import { mockNavigation, mockRoute } from '../../test/navigation';
import type { LocalDirectorySeed } from '../../test/localDirectory';
import { renderWithProviders } from '../../test/render';
import DiagnosticsScreen from '.';

jest.setTimeout(30_000);

const OFFICE_HOST_ID = 'w3k8p2x7m4q9tb6c';
const STUDIO_HOST_ID = 'h7v2m9q3k8x4tb1a';
const OFFICE_ADDR = '100.64.0.12';
const INTRANET_ADDR = '10.30.12.48';

const COMPANY_NETWORK: NetworkEntry = {
  id: 'company',
  name: '公司网络',
  cidrs: ['10.30.0.0/16'],
  vpnPools: ['10.99.0.0/16'],
};

function props(hostId?: string) {
  return {
    navigation: mockNavigation<'Diagnostics'>(),
    route: mockRoute(
      'Diagnostics',
      hostId === undefined ? undefined : { hostId },
    ),
  } as RootScreenProps<'Diagnostics'>;
}

function probe(
  core: FakeCore,
  probeId: string,
  event: Omit<CoreEvent & { type: 'probe.result' }, 'type' | 'probeId'>,
) {
  core.emit({
    type: 'probe.result',
    probeId,
    ...event,
  } as CoreEvent);
}

function result(
  path: 'tailnetDirect' | 'tailnetDerp' | 'intranet',
  transport: 'quic' | 'tcp',
  addr: string,
  outcome: CoreEventOfOutcome,
  ms: number | null,
  reason?: string,
): Omit<CoreEvent & { type: 'probe.result' }, 'type' | 'probeId'> {
  return {
    hostId: OFFICE_HOST_ID,
    path,
    addr,
    port: 7400,
    transport,
    outcome,
    ms,
    reason,
    last: false,
  };
}

type CoreEventOfOutcome = ProbeOutcome;

async function emitFour(core: FakeCore, probeId: string) {
  const events = [
    result('tailnetDirect', 'quic', OFFICE_ADDR, 'timeout', 3000, '打洞失败'),
    result('tailnetDerp', 'quic', OFFICE_ADDR, 'ok', 48),
    result('intranet', 'quic', INTRANET_ADDR, 'refused', null),
    result('intranet', 'tcp', INTRANET_ADDR, 'ok', 23),
  ];
  for (const [index, event] of events.entries()) {
    await act(async () => {
      probe(core, probeId, {
        ...event,
        last: index === events.length - 1,
      });
    });
  }
}

async function flush() {
  await act(async () => {});
}

async function advance(ms: number) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

async function renderScreen(options?: {
  hostId?: string;
  core?: FakeCore;
  network?: NetworkStatus;
  settings?: { networks?: NetworkEntry[] };
  kv?: KvStore;
  /** Defaults to the paired Hosts with the in-app tailnet naming them. */
  localDirectory?: LocalDirectorySeed;
}) {
  const screenProps = props(options?.hostId);
  const core = options?.core ?? createFakeCore();
  const rendered = await renderWithProviders(
    <DiagnosticsScreen {...screenProps} />,
    {
      core,
      // The tailnet rows need 外网访问 (Tailscale) on.
      settings: { 'tailnet.enabled': true, ...options?.settings },
      kv: options?.kv,
      localDirectory: options?.localDirectory ?? { peers: [] },
    },
  );
  await flush();
  if (options?.network !== undefined) {
    await act(async () => {
      core.emit({ type: 'network.changed', status: options.network! });
    });
  }
  return {
    ...rendered,
    core,
    navigation: screenProps.navigation,
    route: screenProps.route,
  };
}

function probeCalls(core: FakeCore) {
  return core.calls.filter(call => call.method === 'diag.probe');
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(FIXTURE_NOW / 1000);
  jest.spyOn(Date.prototype, 'getTimezoneOffset').mockReturnValue(-480);
});

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

describe('Diagnostics', () => {
  it('starts probing the route host and shows four pending rows', async () => {
    const { core } = await renderScreen({ hostId: OFFICE_HOST_ID });

    expect(probeCalls(core)).toEqual([
      { method: 'diag.probe', args: [OFFICE_HOST_ID] },
    ]);
    expect(screen.getByLabelText('返回')).toBeOnTheScreen();
    expect(screen.getByText('设置')).toBeOnTheScreen();
    expect(screen.getByText('工位 Windows')).toBeOnTheScreen();
    expect(screen.getByText('office-win · Host 0.3.1')).toBeOnTheScreen();
    expect(screen.getAllByText('测试中')).toHaveLength(4);
    expect(screen.getByText('正在测，约 10 秒')).toBeOnTheScreen();
    expect(
      screen.getByText('每条路最多等 3 秒，测完给出建议。'),
    ).toBeOnTheScreen();
    expect(screen.getByText('停止')).toBeOnTheScreen();
    expect(screen.getByLabelText('拷贝结果')).toBeDisabled();
    expect(
      screen.getByText('Remote Screen 0.3.1 · RSP/1 · Host ≥ 0.3.0'),
    ).toBeOnTheScreen();
  });

  it('renders results, advice and finished header, then copies text', async () => {
    const { core, platform } = await renderScreen({
      hostId: OFFICE_HOST_ID,
      network: netCompanyVpn,
      settings: { networks: [COMPANY_NETWORK] },
    });

    await emitFour(core, 'p1');

    expect(screen.getAllByText('不通')).toHaveLength(2);
    expect(screen.getByText('打洞失败 · 3.0 s')).toBeOnTheScreen();
    expect(screen.getByText('48 ms')).toBeOnTheScreen();
    expect(screen.getByText('23 ms')).toBeOnTheScreen();
    expect(
      screen.getByText(
        '会走内网 TCP 7400，23 ms 最快。公司网络拦了 UDP，所以是「TCP 模式」。',
      ),
    ).toBeOnTheScreen();
    expect(screen.getByText('10:58 测完 · 4 条路')).toBeOnTheScreen();
    expect(screen.getByText('重新测试')).toBeOnTheScreen();
    expect(screen.getByText('本机 · 公司 VPN 10.99.4.27')).toBeOnTheScreen();
    expect(
      screen.getByLabelText('内网 TCP 7400，10.30.12.48，23 毫秒'),
    ).toBeOnTheScreen();
    expect(
      screen.getByLabelText('tailnet 直连，100.64.0.12，不通，打洞失败'),
    ).toBeOnTheScreen();

    await fireEvent.press(screen.getByLabelText('拷贝结果'));
    await flush();
    expect(platform.copied).toHaveLength(1);
    expect(platform.copied[0].split('\n')[0]).toMatch(
      /^Remote Screen 诊断 · 工位 Windows（office-win）/,
    );
    expect(platform.copied[0]).toContain('内网 TCP 7400 10.30.12.48：23 ms');
    expect(platform.copied[0]).toContain(
      'tailnet 直连 100.64.0.12：不通（打洞失败，3.0 s）',
    );
    expect(screen.getByText('已复制')).toBeOnTheScreen();
  });

  it('moves an embedded relay result to the named DERP row', async () => {
    const core = createFakeCore({
      tailnetAvailable: true,
      tailnet: tailnetRunning,
      network: netCompanyVpnEmbedded,
    });
    await renderScreen({
      hostId: OFFICE_HOST_ID,
      core,
      network: netCompanyVpnEmbedded,
      settings: { networks: [COMPANY_NETWORK] },
    });

    await act(async () => {
      probe(core, 'p1', {
        ...result('tailnetDirect', 'quic', OFFICE_ADDR, 'ok', 48),
        last: true,
      });
    });

    expect(screen.getByText('DERP 上海')).toBeOnTheScreen();
    expect(screen.getByText('48 ms')).toBeOnTheScreen();
    expect(
      screen.getByLabelText('tailnet 直连，100.64.0.12，不通，打洞失败'),
    ).toBeOnTheScreen();
    expect(screen.getByText('会走DERP 上海，48 ms 最快。')).toBeOnTheScreen();
    expect(
      screen.getByText('本机 · 公司 VPN 10.99.4.27 · App 内 100.64.0.21'),
    ).toBeOnTheScreen();
  });

  it('copies embedded relay rows with the regional DERP name', async () => {
    const core = createFakeCore({
      tailnetAvailable: true,
      tailnet: tailnetRunning,
      network: netCompanyVpnEmbedded,
    });
    const { platform } = await renderScreen({
      hostId: OFFICE_HOST_ID,
      core,
      network: netCompanyVpnEmbedded,
      settings: { networks: [COMPANY_NETWORK] },
    });

    await act(async () => {
      probe(core, 'p1', {
        ...result('tailnetDirect', 'quic', OFFICE_ADDR, 'ok', 48),
        last: true,
      });
    });
    await fireEvent.press(screen.getByLabelText('拷贝结果'));
    await flush();

    expect(platform.copied).toHaveLength(1);
    expect(platform.copied[0]).toContain('DERP 上海 100.64.0.12：48 ms');
    expect(platform.copied[0]).toContain(
      '本机 · 公司 VPN 10.99.4.27 · App 内 100.64.0.21',
    );
  });

  it('does not expose a tailnet login URL in copied diagnostics', async () => {
    const core = createFakeCore({
      tailnetAvailable: true,
      tailnet: tailnetNeedsLogin,
      network: { ...netCompanyVpn, tailnet: routeEmbeddedNeedsLogin },
    });
    const { platform } = await renderScreen({
      hostId: OFFICE_HOST_ID,
      core,
      network: { ...netCompanyVpn, tailnet: routeEmbeddedNeedsLogin },
      settings: { networks: [COMPANY_NETWORK] },
      // Keeps the tailnet waiting for login.
      localDirectory: {},
    });

    await fireEvent.press(screen.getByLabelText('停止'));
    await fireEvent.press(screen.getByLabelText('拷贝结果'));
    await flush();

    expect(platform.copied).toHaveLength(1);
    expect(platform.copied[0]).not.toContain('/register/');
    expect(platform.copied[0]).not.toContain('nodekey');
  });

  it('does not query tailnet status when embedded connect is unavailable', async () => {
    const { core } = await renderScreen({
      hostId: OFFICE_HOST_ID,
      localDirectory: {},
    });

    expect(core.calls.some(call => call.method === 'tailnet.status')).toBe(
      false,
    );
    expect(screen.getAllByText('测试中')).toHaveLength(4);
    expect(screen.getByText('本机 · tailnet 100.64.0.21')).toBeOnTheScreen();
  });

  it('retests and finishes all pending rows after the 12 second deadline', async () => {
    const { core } = await renderScreen({ hostId: OFFICE_HOST_ID });
    await emitFour(core, 'p1');

    await fireEvent.press(screen.getByLabelText('重新测试'));
    expect(probeCalls(core)).toHaveLength(2);
    expect(probeCalls(core)[1]).toEqual({
      method: 'diag.probe',
      args: [OFFICE_HOST_ID],
    });

    await advance(12_000);

    expect(screen.getAllByText('不通')).toHaveLength(4);
    expect(
      screen.getByText(
        '每条路都不通。看看电脑是否在线、Remote Host 是否在运行。',
      ),
    ).toBeOnTheScreen();
  });

  it('marks every row failed when the probe call throws', async () => {
    const core = createFakeCore();
    core.failNext('diag.probe', new Error('boom'));
    await renderScreen({ hostId: OFFICE_HOST_ID, core });

    expect(screen.getAllByText('不通')).toHaveLength(4);
    expect(screen.getAllByText('boom')).toHaveLength(4);
    expect(
      screen.getByText(
        '每条路都不通。看看电脑是否在线、Remote Host 是否在运行。',
      ),
    ).toBeOnTheScreen();
  });

  it('stops a run and ignores events from the old probe', async () => {
    const { core } = await renderScreen({ hostId: OFFICE_HOST_ID });

    await fireEvent.press(screen.getByLabelText('停止'));
    expect(screen.getAllByText('不通')).toHaveLength(4);
    expect(screen.getByText('重新测试')).toBeOnTheScreen();
    expect(screen.getByLabelText('拷贝结果')).toBeEnabled();

    await act(async () => {
      probe(core, 'p1', {
        ...result('tailnetDirect', 'quic', OFFICE_ADDR, 'ok', 11),
        last: true,
      });
    });

    expect(screen.getAllByText('不通')).toHaveLength(4);
    expect(screen.queryByText('11 ms')).not.toBeOnTheScreen();
  });

  it('defaults to the first online computer and switches hosts', async () => {
    const { core } = await renderScreen();

    expect(probeCalls(core)[0]).toEqual({
      method: 'diag.probe',
      args: [STUDIO_HOST_ID],
    });
    expect(screen.getByText('Studio Mac mini')).toBeOnTheScreen();
    expect(screen.getAllByText('192.168.31.20')).toHaveLength(2);
    expect(screen.getAllByText('局域网 UDP 7400')).toHaveLength(1);
    expect(screen.getAllByText('局域网 TCP 7400')).toHaveLength(1);

    await fireEvent.press(screen.getByLabelText('换一台'));
    expect(screen.getByText('选一台电脑')).toBeOnTheScreen();
    expect(screen.getAllByText('离线').length).toBeGreaterThanOrEqual(2);

    await fireEvent.press(
      screen.getByLabelText(
        [
          '工位 Windows',
          diagnostics.connection.subtitle('office-win', '0.3.1'),
        ].join('，'),
      ),
    );

    expect(probeCalls(core)).toContainEqual({
      method: 'diag.probe',
      args: [OFFICE_HOST_ID],
    });
    expect(screen.getByText('工位 Windows')).toBeOnTheScreen();
  });

  it('shows the empty state when the directory has no computers', async () => {
    const core = createFakeCore();
    await renderScreen({
      hostId: OFFICE_HOST_ID,
      core,
      localDirectory: { computers: [] },
    });

    expect(screen.getByText('还没有电脑')).toBeOnTheScreen();
    expect(probeCalls(core)).toHaveLength(0);
  });

  it('exports masked logs by default and follows the mask switch', async () => {
    const { core, platform } = await renderScreen({ hostId: OFFICE_HOST_ID });

    await fireEvent.press(screen.getByLabelText('导出 zip'));

    await flush();
    expect(core.calls).toContainEqual({
      method: 'logs.export',
      args: [{ mask: true }],
    });
    expect(platform.calls).toContainEqual({
      method: 'share',
      args: [
        {
          url: 'file:///tmp/rs-logs-masked.zip',
          title: 'Remote Screen 日志',
        },
      ],
    });

    await fireEvent.press(screen.getByLabelText('导出时地址打码'));
    await fireEvent.press(screen.getByLabelText('导出 zip'));

    await flush();
    expect(core.calls).toContainEqual({
      method: 'logs.export',
      args: [{ mask: false }],
    });
  });

  it('shows an export failure toast', async () => {
    const core = createFakeCore();
    core.failNext('logs.export', new Error('disk full'));
    await renderScreen({ hostId: OFFICE_HOST_ID, core });

    await fireEvent.press(screen.getByLabelText('导出 zip'));
    await flush();

    expect(screen.getByText('导出失败：disk full')).toBeOnTheScreen();
  });

  it('offers no crash upload, even with old Hub settings in storage', async () => {
    const kv = new MemoryKv();
    kv.setJSON('settings.hub.url', 'http://rs-hub:7300');
    kv.setJSON('settings.crash.upload', true);
    await renderScreen({ hostId: OFFICE_HOST_ID, kv });

    expect(screen.getByText('本机日志')).toBeOnTheScreen();
    expect(screen.getByLabelText('导出 zip')).toBeOnTheScreen();
    expect(screen.queryByText(/崩溃报告/)).toBeNull();
    expect(screen.queryByText(/Hub/)).toBeNull();
    expect(screen.queryByTestId('diagnostics-crash-upload')).toBeNull();
  });
});
