import { act, fireEvent, screen } from '@testing-library/react-native';
import { AppState } from 'react-native';
import type { AppStateStatus } from 'react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import {
  netCompanyVpnEmbedded,
  netNoTailnet,
  netTailnetOk,
} from '../../native/rscore/fixtures';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { MemoryKv } from '../../features/storage/kv';
import { onboarding } from '../../i18n/zh/onboarding';
import { tailnet } from '../../i18n/zh/tailnet';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import OnboardTailnetScreen from './index';

type AppStateListener = (state: AppStateStatus) => void;

/** This step only exists while 外网访问 (Tailscale) is on. */
const ON = { 'tailnet.enabled': true } as const;

function spyOnAppState(): AppStateListener[] {
  const listeners: AppStateListener[] = [];
  jest.spyOn(AppState, 'addEventListener').mockImplementation((_type, cb) => {
    listeners.push(cb as AppStateListener);
    return { remove: jest.fn() };
  });
  return listeners;
}

afterEach(() => {
  jest.restoreAllMocks();
});

it('goes straight on to finding computers while Tailscale is switched off', async () => {
  const navigation = mockNavigation<'OnboardTailnet'>();
  await renderWithProviders(
    <OnboardTailnetScreen
      navigation={navigation}
      route={mockRoute('OnboardTailnet')}
    />,
  );
  expect(navigation.replace).toHaveBeenCalledWith('OnboardDiscover');
  expect(screen.queryByText('连接到你的 tailnet')).toBeNull();
  expect(screen.queryAllByText(/Tailscale|Headscale/)).toHaveLength(0);
});

it('shows a connected tailnet and records the selected mode', async () => {
  const core = createFakeCore();
  const platform = new FakePlatform();
  const kv = new MemoryKv();
  const navigation = mockNavigation<'OnboardTailnet'>();
  await renderWithProviders(
    <OnboardTailnetScreen
      navigation={navigation}
      route={mockRoute('OnboardTailnet')}
    />,
    { core, platform, kv, settings: ON },
  );

  await act(() => core.emit({ type: 'network.changed', status: netTailnetOk }));
  expect(screen.getByText('已连接')).toBeOnTheScreen();
  expect(
    screen.getByText('检测到 Tailscale 正在运行。本机地址 100.64.0.21'),
  ).toBeOnTheScreen();
  expect(screen.queryByText('打开 Tailscale')).toBeNull();

  await fireEvent.press(screen.getByText('继续'));
  expect(kv.getJSON('settings.tailnet.mode')).toBe('system');
  expect(navigation.navigate).toHaveBeenCalledWith('OnboardDiscover');
});

it('shows the disconnected state and opens Tailscale', async () => {
  const core = createFakeCore();
  const platform = new FakePlatform();
  await renderWithProviders(
    <OnboardTailnetScreen
      navigation={mockNavigation<'OnboardTailnet'>()}
      route={mockRoute('OnboardTailnet')}
    />,
    { core, platform, settings: ON },
  );

  await act(() => core.emit({ type: 'network.changed', status: netNoTailnet }));
  expect(screen.getByText('未连接')).toBeOnTheScreen();
  expect(
    screen.getByText(
      '没有检测到 Tailscale。它只用来在外面连回电脑；只在同一网络里用的话，直接点「继续」。',
    ),
  ).toBeOnTheScreen();

  await fireEvent.press(screen.getByText('打开 Tailscale'));
  expect(platform.opened).toContain('tailscale://');
});

it('leaves the embedded mode out when the build has no tailnet', async () => {
  await renderWithProviders(
    <OnboardTailnetScreen
      navigation={mockNavigation<'OnboardTailnet'>()}
      route={mockRoute('OnboardTailnet')}
    />,
    { settings: ON },
  );
  expect(screen.getByText('1 / 3')).toBeOnTheScreen();
  expect(screen.getByLabelText('第 1 步，共 3 步')).toBeOnTheScreen();
  expect(screen.getByLabelText('接入方式')).toHaveProp(
    'accessibilityRole',
    'radiogroup',
  );

  expect(screen.getByLabelText('使用手机上的 Tailscale')).toBeSelected();
  expect(screen.queryByLabelText('在 App 内连接')).toBeNull();
  expect(screen.queryByTestId('tailnet-mode-embedded')).toBeNull();
  expect(screen.queryByText('实验性')).toBeNull();
  expect(screen.queryByText(/Headscale/)).toBeNull();
  expect(screen.queryByText(/不占用系统 VPN/)).toBeNull();
});

it('shows detecting while no network status is known', async () => {
  const core = createFakeCore();
  core.failNext('network.status', new Error('network unavailable'));
  await renderWithProviders(
    <OnboardTailnetScreen
      navigation={mockNavigation<'OnboardTailnet'>()}
      route={mockRoute('OnboardTailnet')}
    />,
    { core, settings: ON },
  );
  expect(screen.getByText('正在检测…')).toBeOnTheScreen();
  expect(screen.queryByText('打开 Tailscale')).toBeNull();
});

it('refreshes network status when the app becomes active', async () => {
  const listeners = spyOnAppState();
  const core = createFakeCore();
  await renderWithProviders(
    <OnboardTailnetScreen
      navigation={mockNavigation<'OnboardTailnet'>()}
      route={mockRoute('OnboardTailnet')}
    />,
    { core, settings: ON },
  );

  await act(() => core.emit({ type: 'network.changed', status: netNoTailnet }));
  expect(screen.getByText('未连接')).toBeOnTheScreen();

  core.setFixtures({ network: netTailnetOk });
  const readCount = core.calls.filter(
    call => call.method === 'network.status',
  ).length;
  await act(async () => {
    for (const listener of listeners) {
      listener('active');
    }
  });
  expect(
    core.calls.filter(call => call.method === 'network.status'),
  ).toHaveLength(readCount + 1);
  expect(screen.getByText('已连接')).toBeOnTheScreen();
});

it('selects embedded mode and reveals the control URL field', async () => {
  const core = createFakeCore({ tailnetAvailable: true });
  await renderWithProviders(
    <OnboardTailnetScreen
      navigation={mockNavigation<'OnboardTailnet'>()}
      route={mockRoute('OnboardTailnet')}
    />,
    { core, settings: ON },
  );
  await act(() => core.emit({ type: 'network.changed', status: netNoTailnet }));

  const embedded = screen.getByLabelText('在 App 内连接');
  expect(embedded.props.accessibilityState.disabled).not.toBe(true);
  await fireEvent.press(embedded);
  expect(screen.getByLabelText('在 App 内连接')).toBeSelected();
  expect(screen.getByLabelText(tailnet.controlUrl.label)).toBeOnTheScreen();
  expect(screen.getByText(tailnet.controlUrl.note)).toBeOnTheScreen();
  expect(screen.queryByText('打开 Tailscale')).toBeNull();
});

it('rejects an invalid control URL without navigating', async () => {
  const core = createFakeCore({ tailnetAvailable: true });
  const navigation = mockNavigation<'OnboardTailnet'>();
  await renderWithProviders(
    <OnboardTailnetScreen
      navigation={navigation}
      route={mockRoute('OnboardTailnet')}
    />,
    { core, settings: ON },
  );
  await fireEvent.press(screen.getByLabelText('在 App 内连接'));
  await fireEvent.changeText(
    screen.getByLabelText(tailnet.controlUrl.label),
    'ftp://x',
  );
  await fireEvent.press(screen.getByText(onboarding.tailnet.continue));
  await act(async () => {
    await Promise.resolve();
  });
  expect(screen.getByText(tailnet.controlUrl.invalid)).toBeOnTheScreen();
  expect(navigation.navigate).not.toHaveBeenCalled();
});

it('saves the control URL, switches to embedded mode, and continues', async () => {
  const core = createFakeCore({ tailnetAvailable: true });
  const kv = new MemoryKv();
  const navigation = mockNavigation<'OnboardTailnet'>();
  await renderWithProviders(
    <OnboardTailnetScreen
      navigation={navigation}
      route={mockRoute('OnboardTailnet')}
    />,
    { core, kv, settings: ON },
  );
  await fireEvent.press(screen.getByLabelText('在 App 内连接'));
  await fireEvent.changeText(
    screen.getByLabelText(tailnet.controlUrl.label),
    'hs.example.com',
  );
  await fireEvent.press(screen.getByText(onboarding.tailnet.continue));
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
  expect(kv.getJSON('settings.tailnet.mode')).toBe('embedded');
  expect(kv.getJSON('settings.tailnet.controlUrl')).toBe(
    'https://hs.example.com',
  );
  expect(navigation.navigate).toHaveBeenCalledWith('OnboardDiscover');
});

it('does not count an embedded tailnet address as system Tailscale', async () => {
  const core = createFakeCore({ tailnetAvailable: true });
  await renderWithProviders(
    <OnboardTailnetScreen
      navigation={mockNavigation<'OnboardTailnet'>()}
      route={mockRoute('OnboardTailnet')}
    />,
    { core, settings: ON },
  );
  await act(() =>
    core.emit({ type: 'network.changed', status: netCompanyVpnEmbedded }),
  );
  expect(screen.getByText('未连接')).toBeOnTheScreen();
  expect(
    screen.getByText(onboarding.tailnet.system.disconnectedDetail),
  ).toBeOnTheScreen();
});

async function chooseEmbedded(core: ReturnType<typeof createFakeCore>) {
  const navigation = mockNavigation<'OnboardTailnet'>();
  await renderWithProviders(
    <OnboardTailnetScreen
      navigation={navigation}
      route={mockRoute('OnboardTailnet')}
    />,
    { core, settings: ON },
  );
  await fireEvent.press(screen.getByLabelText('在 App 内连接'));
  await fireEvent.changeText(
    screen.getByLabelText(tailnet.controlUrl.label),
    'hs.example.com',
  );
  return navigation;
}

it('holds the continue button while the switch is checked', async () => {
  const core = createFakeCore({ tailnetAvailable: true });
  const navigation = await chooseEmbedded(core);
  const available = jest
    .spyOn(core.tailnet, 'available')
    .mockImplementation(() => new Promise(() => {}));
  await fireEvent.press(screen.getByTestId('onboard-tailnet-continue'));
  await act(async () => {
    await Promise.resolve();
  });
  expect(screen.getByTestId('onboard-tailnet-continue')).toBeDisabled();
  await fireEvent.press(screen.getByTestId('onboard-tailnet-continue'));
  expect(available).toHaveBeenCalledTimes(1);
  expect(navigation.navigate).not.toHaveBeenCalled();
});

it('says why it fell back to system Tailscale', async () => {
  const core = createFakeCore({ tailnetAvailable: true });
  const navigation = await chooseEmbedded(core);
  jest.spyOn(core.tailnet, 'available').mockResolvedValue(false);
  await fireEvent.press(screen.getByTestId('onboard-tailnet-continue'));
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
  expect(screen.getByTestId('onboard-tailnet-error')).toHaveTextContent(
    onboarding.tailnet.embeddedUnavailable,
  );
  expect(screen.getByLabelText('使用手机上的 Tailscale')).toBeSelected();
  expect(screen.getByTestId('onboard-tailnet-continue')).not.toBeDisabled();
  expect(navigation.navigate).not.toHaveBeenCalled();
});

it('offers no disabled embedded card to explain', async () => {
  await renderWithProviders(
    <OnboardTailnetScreen
      navigation={mockNavigation<'OnboardTailnet'>()}
      route={mockRoute('OnboardTailnet')}
    />,
    { core: createFakeCore(), settings: ON },
  );
  expect(screen.queryByTestId('tailnet-mode-embedded-disabled')).toBeNull();
  expect(screen.queryByText('暂不可用')).toBeNull();
});
