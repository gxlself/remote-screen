import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from '@testing-library/react-native';
import { AccessibilityInfo, Platform } from 'react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import DeviceRoutesScreen from '.';

jest.setTimeout(15000);

const HOST_ID = 'w3k8p2x7m4q9tb6c';

async function renderScreen(core = createFakeCore()) {
  const navigation = mockNavigation<'DeviceRoutes'>();
  const route = mockRoute('DeviceRoutes', { hostId: HOST_ID });
  await renderWithProviders(
    <DeviceRoutesScreen navigation={navigation} route={route} />,
    { core, settings: { 'tailnet.enabled': true } },
  );
  return { core, navigation };
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

function probeCalls(core: ReturnType<typeof createFakeCore>) {
  return core.calls.filter(call => call.method === 'diag.probe');
}

afterEach(() => {
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('reports the checked policy with its title label and subtitle hint', async () => {
  await renderScreen();

  const auto = screen.getByTestId('policy-auto');
  expect(auto).toHaveProp('accessibilityLabel', '自动选最快（推荐）');
  expect(auto).toHaveProp('accessibilityHint', '测完后显示哪条最快');
  expect(auto.props.accessibilityState).toMatchObject({
    checked: true,
    selected: true,
  });

  const intranet = screen.getByTestId('policy-intranet');
  expect(intranet).toHaveProp('accessibilityLabel', '只用内网');
  expect(intranet).toHaveProp(
    'accessibilityHint',
    '不走 tailnet，没连 VPN 时就连不上',
  );
  expect(intranet.props.accessibilityState).toMatchObject({
    checked: false,
    selected: false,
  });
});

it('offers retest on the address header and probes without pull-to-refresh', async () => {
  jest.useFakeTimers();
  const { core } = await renderScreen();
  await waitFor(() => expect(probeCalls(core)).toHaveLength(1));

  const header = screen.getByLabelText('地址，2 个 · 测试中…');
  expect(header).toHaveProp('accessibilityRole', 'header');
  expect(header).toHaveProp('accessibilityLiveRegion', 'polite');
  expect(header.props.accessibilityActions).toEqual([
    { name: 'retest', label: '重新测试' },
  ]);

  await fireEvent(header, 'accessibilityAction', {
    nativeEvent: { actionName: 'retest' },
  });
  await waitFor(() => expect(probeCalls(core)).toHaveLength(2));
});

it('announces probe failure on iOS only', async () => {
  const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
  announce.mockClear();
  const core = createFakeCore();
  core.failNext('diag.probe', new Error('probe failed'));

  await renderScreen(core);
  await waitFor(() =>
    expect(screen.getByText('测试失败，稍后再试')).toBeOnTheScreen(),
  );
  expect(announce).toHaveBeenCalledWith('测试失败，稍后再试');
});

it('does not announce probe failure on Android', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
  announce.mockClear();
  const core = createFakeCore();
  core.failNext('diag.probe', new Error('probe failed'));

  await renderScreen(core);
  await waitFor(() =>
    expect(screen.getByText('测试失败，稍后再试')).toBeOnTheScreen(),
  );
  expect(announce).not.toHaveBeenCalled();
});

it('announces the completed summary once on iOS and not on first render', async () => {
  const announce = jest.spyOn(AccessibilityInfo, 'announceForAccessibility');
  announce.mockClear();
  const { core } = await renderScreen();
  await waitFor(() => expect(probeCalls(core)).toHaveLength(1));
  expect(announce).not.toHaveBeenCalled();

  await act(() => core.emit(probe('p1', 'intranet', '10.30.12.48', 23, false)));
  expect(announce).not.toHaveBeenCalled();

  await act(() =>
    core.emit(probe('p1', 'tailnetDirect', '100.64.0.12', 48, true)),
  );
  expect(announce).toHaveBeenCalledTimes(1);
  expect(announce).toHaveBeenCalledWith('2 个 · 刚刚测过');
});

it('adds Android ripple to policy options only on Android', async () => {
  await renderScreen();
  const iosOption = screen.getByTestId('policy-auto');
  expect(iosOption.props.nativeBackgroundAndroid).toBeUndefined();

  await cleanup();
  jest.replaceProperty(Platform, 'OS', 'android');
  await renderScreen();

  const androidOption = screen.getByTestId('policy-auto');
  expect(androidOption.props.nativeBackgroundAndroid).toMatchObject({
    type: 'RippleAndroid',
    borderless: false,
  });
});
