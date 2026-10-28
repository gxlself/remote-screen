import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import { AccessibilityInfo, PixelRatio, Platform } from 'react-native';
import type { CoreEventOf } from '../../native/rscore/types';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { addByAddress } from '../../i18n/zh/addByAddress';
import { renderWithProviders } from '../../test/render';
import { mockNavigation, mockRoute } from '../../test/navigation';
import type { RootScreenProps } from '../../app/navigation/routes';
import AddByAddressScreen from './index';
import { ProbeResultCard } from './ProbeResultCard';
import type { AddrProbe } from '../../features/servers/addressProbe';

jest.setTimeout(20_000);

function keyboardBehavior(instance: { unstable_fiber?: unknown }) {
  let fiber = instance.unstable_fiber as
    | { memoizedProps?: Record<string, unknown>; return?: unknown }
    | null
    | undefined;
  while (fiber != null) {
    if (fiber.memoizedProps?.behavior !== undefined) {
      return fiber.memoizedProps.behavior;
    }
    fiber = fiber.return as typeof fiber;
  }
  return undefined;
}

const emptyProbe: AddrProbe = {
  state: 'done',
  rs: {
    type: 'probe.result',
    probeId: 'probe-1',
    hostId: null,
    path: 'intranet',
    addr: '10.30.12.48',
    port: 7400,
    transport: 'tcp',
    outcome: 'ok',
    ms: 12,
    last: true,
  },
  hello: {
    hostId: 'unknown-host',
    name: '新电脑',
    os: 'windows',
    hostVersion: '0.3.0',
    fp: 'fp',
  },
  rdp: 'closed',
  vnc: null,
  ssh: 'closed',
  discover: false,
  startedAtMs: 0,
  elapsedMs: 12,
};

function props(): RootScreenProps<'AddByAddress'> {
  return {
    navigation: mockNavigation<'AddByAddress'>(),
    route: mockRoute('AddByAddress'),
  } as RootScreenProps<'AddByAddress'>;
}

function probeEvent(
  probeId: string,
  port: number,
  outcome: CoreEventOf<'probe.result'>['outcome'],
): CoreEventOf<'probe.result'> {
  return {
    type: 'probe.result',
    probeId,
    hostId: null,
    path: 'intranet',
    addr: '10.30.12.48',
    port,
    transport: 'tcp',
    outcome,
    ms: 12,
    last: true,
  };
}

async function advance() {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
}

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('keeps the page still on iOS and insets only the form scroll', async () => {
  jest.replaceProperty(Platform, 'OS', 'ios');
  const view = await renderWithProviders(<AddByAddressScreen {...props()} />);
  expect(
    keyboardBehavior(view.getByTestId('add-by-address-form')),
  ).toBeUndefined();
  const scroll = view.getByTestId('add-by-address-form-scroll');
  expect(scroll).toHaveProp('automaticallyAdjustKeyboardInsets', true);
  expect(scroll).toHaveProp('keyboardDismissMode', 'interactive');
  expect(scroll).toHaveProp('keyboardShouldPersistTaps', 'handled');
  // The back chevron is pinned above the scroll area so it never scrolls away.
  expect(scroll).not.toContainElement(view.getByLabelText('返回'));
  expect(scroll).toContainElement(view.getByTestId('add-address-host'));
  expect(scroll).toContainElement(view.getByTestId('add-address-find'));
});

it('uses padding keyboard avoidance on android', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const view = await renderWithProviders(<AddByAddressScreen {...props()} />);
  expect(keyboardBehavior(view.getByTestId('add-by-address-form'))).toBe(
    'padding',
  );
});

it('drops the unusable code row and offers scanning instead', async () => {
  jest.useFakeTimers();
  const core = createFakeCore();
  const navigation = mockNavigation<'AddByAddress'>();
  await renderWithProviders(
    <AddByAddressScreen
      navigation={navigation}
      route={mockRoute('AddByAddress')}
    />,
    { core },
  );
  await fireEvent.changeText(
    screen.getByLabelText('IP 地址或主机名'),
    '10.30.12.48',
  );
  await fireEvent.press(screen.getByRole('button', { name: '查找' }));
  await act(async () => {
    core.emit(probeEvent('p1', 7400, 'ok'));
    core.emit(probeEvent('p2', 3389, 'ok'));
    core.emit(probeEvent('p3', 22, 'timeout'));
    await Promise.resolve();
  });
  await advance();
  await fireEvent.press(screen.getByRole('button', { name: '当作新电脑配对' }));

  // Without a Host id the code cannot be checked: no dead row, just the
  // reason and the scan button.
  expect(
    screen.queryByRole('button', {
      name: [addByAddress.pair.code, addByAddress.pair.missingHello].join('，'),
    }),
  ).toBeNull();
  expect(screen.getByText(addByAddress.pair.missingHello)).toBeOnTheScreen();
  await fireEvent.press(screen.getByRole('button', { name: '去扫码' }));
  expect(navigation.navigate).toHaveBeenCalledWith('PairAdd', { tab: 'scan' });
});

it('does not label the probe card and announces completed results only on iOS', async () => {
  const announce = jest
    .spyOn(AccessibilityInfo, 'announceForAccessibility')
    .mockImplementation(() => {});
  jest.replaceProperty(Platform, 'OS', 'ios');
  const view = await renderWithProviders(
    <ProbeResultCard
      match={null}
      port={7400}
      probe={emptyProbe}
      verdict="new"
    />,
  );
  expect(view.getByTestId('probe-result-card')).not.toHaveProp(
    'accessibilityLabel',
  );
  expect(announce).toHaveBeenCalledWith('找到 Host 0.3.0，新电脑');
  announce.mockClear();
  jest.replaceProperty(Platform, 'OS', 'android');
  await view.rerender(
    <ProbeResultCard
      match={null}
      port={7400}
      probe={emptyProbe}
      verdict="notFound"
    />,
  );
  expect(announce).not.toHaveBeenCalled();
});

it('stacks the address fields at a large font scale', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  const view = await renderWithProviders(<AddByAddressScreen {...props()} />);
  expect(
    view.getByTestId('add-address-host').parent?.parent?.parent,
  ).toHaveStyle({
    flexDirection: 'column',
    alignItems: 'stretch',
  });
  expect(view.getByTestId('add-address-port').parent?.parent).toHaveStyle({
    width: undefined,
  });
});
