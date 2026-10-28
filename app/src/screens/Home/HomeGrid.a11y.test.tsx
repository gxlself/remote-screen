import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from '@testing-library/react-native';
import {
  AccessibilityInfo,
  PixelRatio,
  Platform,
  StyleSheet,
} from 'react-native';
import type { TabScreenProps } from '../../app/navigation/routes';
import { nativeCardModel } from '../../features/directory/cardModel';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { FIXTURE_NOW, liveSession } from '../../native/rscore/fixtures';
import { mockNavigation } from '../../test/navigation';
import { mockWindowDimensions } from '../../test/dimensions';
import { renderWithProviders } from '../../test/render';
import { hit } from '../../theme/tokens';
import { HomeNarrow } from './HomeNarrow';
import { LiveSessionBar } from './LiveSessionBar';
import { SearchField } from './SearchField';
import HomeScreen from './index';

jest.setTimeout(30_000);

const OFFICE_ID = 'w3k8p2x7m4q9tb6c';
const MACBOOK_ID = 'm5q1z8r2k7v3xa9d';

const actions = {
  add: jest.fn(),
  installHost: jest.fn(),
  backToSession: jest.fn(),
  longPressCard: jest.fn(),
  openCard: jest.fn(),
  openServer: jest.fn(),
  openSettings: jest.fn(),
  openTerminal: jest.fn(),
};

function narrowProps(overrides: Record<string, unknown> = {}) {
  return {
    actions,
    banners: [],
    cards: [],
    headerStatus: { tone: 'good' as const, text: '1 台在线' },
    loadError: null,
    isEmpty: false,
    isLoading: false,
    liveSession: null,
    onRefresh: jest.fn(),
    query: '',
    refreshing: false,
    servers: [],
    setQuery: jest.fn(),
    ...overrides,
  };
}

async function renderWide(options?: {
  core?: ReturnType<typeof createFakeCore>;
}) {
  const navigation =
    mockNavigation<'MainTabs'>() as unknown as TabScreenProps<'Home'>['navigation'];
  const result = await renderWithProviders(
    <HomeScreen
      navigation={navigation}
      route={{ key: 'Home-a11y-test', name: 'Home' }}
    />,
    { core: options?.core, localDirectory: {} },
  );
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
  return { ...result, navigation };
}

let restoreDimensions: (() => void) | undefined;

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(FIXTURE_NOW / 1000);
});

afterEach(async () => {
  await cleanup();
  restoreDimensions?.();
  restoreDimensions = undefined;
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('exposes refresh as an accessibility action on the narrow computer header', async () => {
  const onRefresh = jest.fn();
  await renderWithProviders(<HomeNarrow {...narrowProps({ onRefresh })} />);

  const header = screen
    .getAllByLabelText('电脑')
    .find(element =>
      element.props.accessibilityActions?.some(
        (action: { name: string }) => action.name === 'refresh',
      ),
    );
  expect(header).toBeDefined();
  await fireEvent(header!, 'accessibilityAction', {
    nativeEvent: { actionName: 'refresh' },
  });
  expect(onRefresh).toHaveBeenCalledTimes(1);
});

it('keeps the live-session wrapper out of the accessibility tree', async () => {
  const rendered = await render(
    <LiveSessionBar
      bottom={20}
      onPress={jest.fn()}
      session={liveSession as never}
    />,
  );

  expect(rendered.getByTestId('live-session-bar')).toHaveProp(
    'accessible',
    false,
  );
  expect(rendered.getByLabelText('Studio Mac mini，控制中')).toBeOnTheScreen();
  expect(
    rendered.getByLabelText('回到 Studio Mac mini 的远控会话'),
  ).toBeOnTheScreen();
});

it('uses a scalable minimum height for the search field', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  const setQuery = jest.fn();
  const rendered = await render(
    <SearchField query="studio" setQuery={setQuery} />,
  );

  const field = rendered.getByTestId('home-search');
  expect(StyleSheet.flatten(field.props.style)).toMatchObject({
    minHeight: hit.min,
  });
  expect(StyleSheet.flatten(field.props.style)).not.toHaveProperty('height');
});

it('uses Android input colors and a full-size clear target', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const rendered = await render(
    <SearchField query="studio" setQuery={jest.fn()} />,
  );

  const input = rendered.getByTestId('home-search');
  expect(input).toHaveProp('underlineColorAndroid', 'transparent');
  expect(input).toHaveProp('cursorColor');
  expect(input).toHaveProp('selectionColor');
  expect(
    rendered.getByTestId('home-search-clear').props.hitSlop,
  ).toBeGreaterThanOrEqual((hit.min - 32) / 2);
});

it('announces a newly empty narrow search result only on iOS', async () => {
  const card = {
    hostId: 'host-test',
    group: 0 as const,
    look: 'live' as const,
    name: 'Test host',
    tap: { kind: 'open' as const },
    thumbnailUrl: null,
    displays: 1,
  };
  const announce = jest
    .spyOn(AccessibilityInfo, 'announceForAccessibility')
    .mockImplementation(() => undefined);
  const first = narrowProps({ cards: [card], query: 'test' });
  const rendered = await renderWithProviders(<HomeNarrow {...first} />);
  await act(async () => {
    rendered.rerender(
      <HomeNarrow {...narrowProps({ cards: [], query: 'missing' })} />,
    );
  });
  expect(announce).toHaveBeenCalledWith('没有匹配的电脑');

  announce.mockClear();
  jest.replaceProperty(Platform, 'OS', 'android');
  const android = await renderWithProviders(
    <HomeNarrow {...narrowProps({ cards: [], query: 'missing' })} />,
  );
  expect(announce).not.toHaveBeenCalled();
  expect(screen.getByText('没有匹配的电脑')).toHaveProp(
    'accessibilityLiveRegion',
    'polite',
  );
  android.unmount();
});

it('marks the wide title as a header and makes button previews non-accessible containers', async () => {
  restoreDimensions = mockWindowDimensions({ width: 1194, height: 834 });
  await renderWide();

  expect(
    screen.getByRole('header', { name: 'Studio Mac mini' }),
  ).toBeOnTheScreen();
  await fireEvent.press(screen.getByTestId(`wide-computer-${OFFICE_ID}`));
  expect(screen.getByTestId('wide-preview')).toHaveProp('accessible', false);
  await fireEvent.press(screen.getByTestId(`wide-computer-${MACBOOK_ID}`));
  expect(screen.getByTestId('wide-preview')).toHaveProp('accessible', false);
});

it('moves accessibility focus after a user selects a sidebar computer', async () => {
  restoreDimensions = mockWindowDimensions({ width: 1194, height: 834 });
  const send = jest
    .spyOn(AccessibilityInfo, 'sendAccessibilityEvent')
    .mockImplementation(() => undefined);
  await renderWide();
  expect(send).not.toHaveBeenCalled();
  await fireEvent.press(screen.getByTestId(`wide-computer-${OFFICE_ID}`));
  await act(async () => {
    await jest.advanceTimersByTimeAsync(0);
  });
  expect(send).toHaveBeenCalledWith(expect.anything(), 'focus');
});

it('offers a longpress accessibility action for sidebar details', async () => {
  restoreDimensions = mockWindowDimensions({ width: 1194, height: 834 });
  const { navigation } = await renderWide();
  const row = screen.getByTestId(`wide-computer-${OFFICE_ID}`);
  expect(row.props.accessibilityActions).toEqual([
    { name: 'longpress', label: '更多操作' },
  ]);
  await fireEvent(row, 'accessibilityAction', {
    nativeEvent: { actionName: 'longpress' },
  });
  expect(navigation.navigate).toHaveBeenCalledWith('DeviceSheet', {
    hostId: OFFICE_ID,
  });
});

it('combines info-cell content on Android', async () => {
  restoreDimensions = mockWindowDimensions({ width: 1194, height: 834 });
  jest.replaceProperty(Platform, 'OS', 'android');
  await renderWide({ core: createFakeCore() });
  await fireEvent.press(screen.getByTestId(`wide-computer-${OFFICE_ID}`));
  expect(screen.getByTestId('wide-info-displays')).toHaveProp(
    'accessibilityLabel',
  );
  expect(
    screen.getByTestId('wide-info-displays').props.accessibilityLabel,
  ).toContain('显示器，');
});

it('reads a host-less card as name, tool, reach and badge with a details action', async () => {
  const card = nativeCardModel(
    {
      kind: 'manual',
      key: 'manual:lab-pc',
      manual: {
        id: 'lab-pc',
        kind: 'computer',
        host: '192.168.31.50',
        port: 3389,
        hostId: null,
        name: 'Lab PC',
        networkId: null,
        addedAtUs: FIXTURE_NOW,
        os: 'windows',
        protocol: 'rdp',
      },
    },
    'unknown',
  );
  const longPressCard = jest.fn();
  const openCard = jest.fn();
  await renderWithProviders(
    <HomeNarrow
      {...narrowProps({
        actions: { ...actions, longPressCard, openCard },
        cards: [card],
      })}
    />,
  );

  const element = screen.getByTestId('card-manual:lab-pc');
  expect(element).toHaveProp(
    'accessibilityLabel',
    'Lab PC，Windows · 远程桌面，在线状态未知，系统共享',
  );
  expect(element).toHaveProp(
    'accessibilityHint',
    '轻点两下开始远控，长按看详情',
  );
  expect(element.props.accessibilityActions).toEqual([
    { name: 'longpress', label: '查看详情' },
  ]);
  await fireEvent(element, 'accessibilityAction', {
    nativeEvent: { actionName: 'longpress' },
  });
  expect(longPressCard).toHaveBeenCalledWith(card);
  await fireEvent.press(element);
  expect(openCard).toHaveBeenCalledWith(card);
});
