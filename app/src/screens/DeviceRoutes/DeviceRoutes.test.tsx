import { act, fireEvent, screen, waitFor } from '@testing-library/react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import {
  netCompanyVpn,
  netCompanyVpnEmbedded,
  tailnetRunning,
} from '../../native/rscore/fixtures';
import type { SettingValue } from '../../features/settings/schema';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import DeviceRoutesScreen from '.';

jest.setTimeout(15000);

const HOST_ID = 'w3k8p2x7m4q9tb6c';
// Pairing keeps bare addresses; the registered network makes 10.30.x intranet.
const OFFICE_NETWORKS: SettingValue<'networks'> = [
  { id: 'office', name: '公司网络', cidrs: ['10.30.0.0/16'], vpnPools: [] },
];

async function renderScreen(
  core = createFakeCore(),
  settings?: Partial<{
    networks: SettingValue<'networks'>;
    'tailnet.enabled': boolean;
  }>,
) {
  const navigation = mockNavigation<'DeviceRoutes'>();
  const route = mockRoute('DeviceRoutes', { hostId: HOST_ID });
  const rendered = await renderWithProviders(
    <DeviceRoutesScreen navigation={navigation} route={route} />,
    // The tailnet rows and policy need 外网访问 (Tailscale) on.
    {
      core,
      settings: {
        networks: OFFICE_NETWORKS,
        'tailnet.enabled': true,
        ...settings,
      },
    },
  );
  return {
    core,
    rendered,
    navigation,
  };
}

function probe(
  probeId: string,
  path: 'intranet' | 'tailnetDirect' | 'tailnetDerp',
  addr: string,
  ms: number,
  last: boolean,
) {
  return {
    type: 'probe.result' as const,
    probeId,
    hostId: HOST_ID,
    path,
    addr,
    port: 7400,
    transport: 'quic' as const,
    outcome: 'ok' as const,
    ms,
    last,
  };
}

afterEach(() => {
  jest.useRealTimers();
});

it('probes on focus and shows each route result', async () => {
  const { core } = await renderScreen();

  await waitFor(() => {
    expect(core.calls).toContainEqual({
      method: 'diag.probe',
      args: [HOST_ID],
    });
  });
  expect(await screen.findByText('2 个 · 测试中…')).toBeOnTheScreen();

  await act(() => core.emit(probe('p1', 'intranet', '10.30.12.48', 23, false)));
  await act(() =>
    core.emit(probe('p1', 'tailnetDirect', '100.64.0.12', 48, true)),
  );

  expect(screen.getByText('23 ms')).toBeOnTheScreen();
  expect(screen.getByText('48 ms')).toBeOnTheScreen();
  expect(screen.getByText('2 个 · 刚刚测过')).toBeOnTheScreen();
  expect(screen.getByText('现在是公司内网 23 ms 最快')).toBeOnTheScreen();
});

it('renders the route row with its spoken label', async () => {
  const core = createFakeCore();
  core.setFixtures({ network: netCompanyVpn });
  const { core: renderedCore } = await renderScreen(core, {
    networks: [
      {
        id: 'company',
        name: '公司网络',
        cidrs: ['10.30.0.0/16'],
        vpnPools: ['10.99.0.0/16'],
      },
    ],
  });
  expect(renderedCore).toBe(core);
  await act(() => core.emit(probe('p1', 'intranet', '10.30.12.48', 23, true)));

  expect(
    screen.getByLabelText(
      '公司网络，10.30.12.48:7400，需要公司 VPN · 已连接，23 毫秒，当前在用',
    ),
  ).toBeOnTheScreen();
});

it('shows the current chip when the path is in use', async () => {
  const core = createFakeCore();
  core.setFixtures({
    sessions: [
      {
        id: 's-live',
        hostId: HOST_ID,
        hostName: '工位 Windows',
        phase: 'streaming',
        role: 'controller',
        path: 'intranet',
        transport: 'quic',
        startedAt: 1,
        displayId: 1,
      },
    ],
  });
  const { core: renderedCore } = await renderScreen(core);
  expect(renderedCore).toBe(core);

  await act(() => core.emit(probe('p1', 'intranet', '10.30.12.48', 23, true)));
  expect(screen.getByText('当前在用')).toBeOnTheScreen();
});

it('stops showing testing after five seconds without a final result', async () => {
  jest.useFakeTimers();
  await renderScreen();

  expect(await screen.findByText('2 个 · 测试中…')).toBeOnTheScreen();
  await act(() => {
    jest.advanceTimersByTime(5000);
  });
  expect(screen.queryByText('2 个 · 测试中…')).toBeNull();
  expect(screen.getByText('2 个 · 刚刚测过')).toBeOnTheScreen();
});

it('lists no tailnet route or policy while Tailscale is switched off', async () => {
  await renderScreen(createFakeCore(), { 'tailnet.enabled': false });
  await act(async () => {
    await Promise.resolve();
  });
  expect(screen.getByTestId('policy-auto')).toBeOnTheScreen();
  expect(screen.getByTestId('policy-intranet')).toBeOnTheScreen();
  expect(screen.queryByTestId('policy-tailnet')).toBeNull();
  expect(screen.queryAllByText(/tailnet|Tailscale/)).toHaveLength(0);
});

it('selects the automatic policy by default and persists a policy change', async () => {
  const { rendered } = await renderScreen();

  expect(screen.getByTestId('policy-auto')).toBeSelected();
  await fireEvent.press(screen.getByLabelText('只用内网'));
  expect(screen.getByTestId('policy-intranet')).toBeSelected();
  expect(rendered.settingsStore.get('hosts.prefs')).toEqual({
    [HOST_ID]: { routePolicy: 'intranet' },
  });
});

it('persists the TCP fallback switch', async () => {
  const { rendered } = await renderScreen();

  expect(screen.getByTestId('tcp-fallback')).toBeChecked();
  await fireEvent.press(screen.getByTestId('tcp-fallback'));
  expect(screen.getByTestId('tcp-fallback')).not.toBeChecked();
  expect(rendered.settingsStore.get('hosts.prefs')).toEqual({
    [HOST_ID]: { tcpFallback: false },
  });
});

it('navigates to add an address and goes back', async () => {
  const { navigation } = await renderScreen();

  await fireEvent.press(screen.getByTestId('add-address'));
  expect(navigation.navigate).toHaveBeenCalledWith('AddByAddress');

  await fireEvent.press(screen.getByLabelText('返回设备详情'));
  expect(navigation.goBack).toHaveBeenCalledTimes(1);
});

it('shows the probe error banner when probing fails', async () => {
  const core = createFakeCore();
  core.failNext('diag.probe', new Error('probe failed'));
  await renderScreen(core);

  await waitFor(() => {
    expect(screen.getByText('测试失败，稍后再试')).toBeOnTheScreen();
  });
  expect(screen.queryByText('2 个 · 测试中…')).toBeNull();
});

it('labels the tailnet row from the embedded peer status', async () => {
  const core = createFakeCore({
    tailnetAvailable: true,
    tailnet: tailnetRunning,
    network: netCompanyVpnEmbedded,
  });
  await renderScreen(core);

  await waitFor(() => {
    expect(screen.getByText('tailnet · App 内')).toBeOnTheScreen();
    expect(screen.getByText('office-win · 经上海中继')).toBeOnTheScreen();
  });
  expect(core.calls).toContainEqual({ method: 'tailnet.status', args: [] });
});

it('does not query tailnet status without embedded routing', async () => {
  const core = createFakeCore();
  await renderScreen(core);

  await waitFor(() => {
    expect(core.calls).toContainEqual({
      method: 'tailnet.available',
      args: [],
    });
  });
  expect(screen.queryByText('tailnet · App 内')).toBeNull();
  expect(core.calls).not.toContainEqual({
    method: 'tailnet.status',
    args: [],
  });
});
