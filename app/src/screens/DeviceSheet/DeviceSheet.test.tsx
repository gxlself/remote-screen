import {
  cleanup,
  fireEvent,
  screen,
  waitFor,
  within,
} from '@testing-library/react-native';
import { StackActions } from '@react-navigation/native';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import { installFakeAdapters } from '../../features/adapters/testing/fakeAdapters';
import type { FallbackGuideState } from '../../features/fallback/useRemoteFallback';
import { HISTORY_KEY } from '../../features/sessions/history';
import type { HistoryEntry } from '../../features/sessions/history';
import { MemoryKv } from '../../features/storage/kv';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { createFakePlatform } from '../../native/platform/fakePlatform';
import { FIXTURE_NOW } from '../../native/rscore/fixtures';
import type { FakeCore } from '../../native/rscore/fakeCore';
import type { RootScreenProps } from '../../app/navigation/routes';
import DeviceSheetScreen from '.';
import { formatLastConnection } from './useDeviceSheet';

// useConnect navigates through the root ref.
const mockRootNavigate = jest.fn();
const mockRootDispatch = jest.fn();
jest.mock('../../app/navigation/navigationRef', () => ({
  navigationRef: {
    isReady: () => true,
    navigate: (...args: unknown[]) => mockRootNavigate(...args),
    dispatch: (...args: unknown[]) => mockRootDispatch(...args),
  },
}));

// C7 never routes a Host computer to an external app, so a guide is injected
// to check that the sheet renders the one useConnect returns.
let mockGuide: FallbackGuideState = null;
const mockCloseGuide = jest.fn();
jest.mock('../../features/native/useConnect', () => {
  const actual = jest.requireActual<
    typeof import('../../features/native/useConnect')
  >('../../features/native/useConnect');
  return {
    ...actual,
    useConnect: () => {
      const result = actual.useConnect();
      return mockGuide === null
        ? result
        : { ...result, guide: mockGuide, closeGuide: mockCloseGuide };
    },
  };
});

const STUDIO_HOST_ID = 'h7v2m9q3k8x4tb1a';
const OFFICE_HOST_ID = 'w3k8p2x7m4q9tb6c';
const MACBOOK_HOST_ID = 'm5q1z8r2k7v3xa9d';
const MINUTE_US = 60 * 1_000_000;
const DAY_US = 24 * 60 * MINUTE_US;

/** A session this phone ran with a computer, as the local history keeps it. */
function historyEntry(
  endedAtUs: number,
  overrides: Partial<HistoryEntry> = {},
): HistoryEntry {
  return {
    id: `session-${endedAtUs}`,
    kind: 'remote',
    targetId: STUDIO_HOST_ID,
    name: 'Studio Mac mini',
    startedAtUs: endedAtUs - 90 * 1_000_000,
    endedAtUs,
    durationS: 90,
    ...overrides,
  };
}

/** The last Studio session, today at 10:12 for 42 minutes. */
const STUDIO_LAST = historyEntry(FIXTURE_NOW - 46 * MINUTE_US, {
  durationS: 42 * 60,
});

function route(
  hostId = STUDIO_HOST_ID,
): RootScreenProps<'DeviceSheet'>['route'] {
  return mockRoute('DeviceSheet', { hostId });
}

/** This phone's storage with the local session history. */
function historyKv(history: HistoryEntry[] = [STUDIO_LAST]): MemoryKv {
  const kv = new MemoryKv();
  kv.setJSON(HISTORY_KEY, history);
  return kv;
}

async function renderSheet(
  hostId = STUDIO_HOST_ID,
  core: FakeCore = createFakeCore(),
  history?: HistoryEntry[],
) {
  const navigation = mockNavigation<'DeviceSheet'>();
  const rendered = await renderWithProviders(
    <DeviceSheetScreen navigation={navigation} route={route(hostId)} />,
    // Fixture computers are online through the tailnet: 外网访问 on.
    {
      core,
      kv: historyKv(history),
      settings: { 'tailnet.enabled': true },
      localDirectory: { peers: [] },
    },
  );
  return { ...rendered, navigation };
}

async function renderLoaded(
  hostId = STUDIO_HOST_ID,
  core?: FakeCore,
  history?: HistoryEntry[],
) {
  const rendered = await renderSheet(hostId, core, history);
  const title =
    hostId === STUDIO_HOST_ID
      ? 'Studio Mac mini'
      : hostId === OFFICE_HOST_ID
      ? '工位 Windows'
      : '小明的 MacBook Pro';
  await screen.findByText(title);
  return rendered;
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(new Date(FIXTURE_NOW / 1000));
  jest.spyOn(Date.prototype, 'getTimezoneOffset').mockReturnValue(-480);
});

afterEach(async () => {
  await cleanup();
  mockRootNavigate.mockReset();
  mockRootDispatch.mockReset();
  mockGuide = null;
  mockCloseGuide.mockReset();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('renders the host identity, chips and connection summary', async () => {
  await renderLoaded();

  expect(screen.getByText('Studio Mac mini')).toBeOnTheScreen();
  // H53: the subtitle is the node name only; addresses live in the row below.
  expect(screen.getByText('studio-mac')).toBeOnTheScreen();
  expect(screen.getByText('macOS 26.1')).toBeOnTheScreen();
  expect(screen.getByText('Host 0.3.1')).toBeOnTheScreen();
  expect(screen.getByText('在线')).toBeOnTheScreen();
  expect(screen.getByText('今天 10:12 · 42 分钟')).toBeOnTheScreen();
  expect(screen.getByText('在线 · 2 条地址')).toBeOnTheScreen();
  // Permissions were only known through a Hub.
  expect(screen.queryByText('权限齐全')).toBeNull();
  expect(screen.queryByText('缺少权限')).toBeNull();
});

it('takes the last connection from the newest remote session with this computer', async () => {
  await renderLoaded(STUDIO_HOST_ID, undefined, [
    historyEntry(FIXTURE_NOW - 10 * MINUTE_US, {
      id: 'ssh',
      kind: 'ssh',
      targetId: STUDIO_HOST_ID,
    }),
    historyEntry(FIXTURE_NOW - 20 * MINUTE_US, {
      id: 'office',
      targetId: OFFICE_HOST_ID,
    }),
    historyEntry(FIXTURE_NOW - DAY_US, { id: 'older', durationS: 5 * 60 }),
    STUDIO_LAST,
  ]);

  expect(screen.getByText('上次连接')).toBeOnTheScreen();
  expect(screen.getByText('今天 10:12 · 42 分钟')).toBeOnTheScreen();
});

it('hides the last connection when this phone never connected', async () => {
  await renderLoaded(OFFICE_HOST_ID);

  expect(screen.queryByText('上次连接')).toBeNull();
});

it('formats yesterday and older last-connection dates', () => {
  const timezone = 480;

  expect(
    formatLastConnection(
      historyEntry(FIXTURE_NOW - DAY_US),
      FIXTURE_NOW,
      timezone,
    ),
  ).toBe('昨天 10:58 · 1 分钟');
  expect(
    formatLastConnection(
      historyEntry(FIXTURE_NOW - 3 * DAY_US),
      FIXTURE_NOW,
      timezone,
    ),
  ).toBe('10月17日 10:58 · 1 分钟');
});

it('selects a display and passes it to a control session', async () => {
  const { navigation } = await renderLoaded();

  expect(screen.getByText('选择显示器')).toBeOnTheScreen();
  // H45: displays go by the names the computer reports.
  expect(screen.getByText('Studio Display')).toBeOnTheScreen();
  expect(screen.getByText('竖屏显示器')).toBeOnTheScreen();
  expect(screen.getByText('2560×1600 · 主显示器')).toBeOnTheScreen();
  expect(screen.getByText('2160×3840 · 竖放')).toBeOnTheScreen();
  expect(screen.getByText('控制「Studio Display」')).toBeOnTheScreen();
  expect(
    screen.getByTestId('display-option-1').props.accessibilityState,
  ).toMatchObject({ checked: true, selected: true });

  await fireEvent.press(screen.getByText('竖屏显示器'));
  expect(screen.getByText('控制「竖屏显示器」')).toBeOnTheScreen();
  expect(
    screen.getByTestId('display-option-2').props.accessibilityState,
  ).toMatchObject({ checked: true, selected: true });

  await fireEvent.press(screen.getByText('控制「竖屏显示器」'));
  expect(navigation.replace).toHaveBeenCalledWith('Session', {
    hostId: STUDIO_HOST_ID,
    displayId: 2,
  });
});

it('starts a view-only session and opens the computer terminal', async () => {
  const { navigation } = await renderLoaded();

  await fireEvent.press(screen.getByText('仅观看'));
  expect(navigation.replace).toHaveBeenCalledWith('Session', {
    hostId: STUDIO_HOST_ID,
    viewOnly: true,
    displayId: 1,
  });

  await fireEvent.press(screen.getByText('终端'));
  expect(navigation.replace).toHaveBeenCalledWith('Terminal', {
    serverId: 'studio-mac',
  });
});

it('offers no file transfer until the Files tab can take this computer', async () => {
  await renderLoaded();

  // H20: the always-disabled 传文件 action is gone.
  expect(screen.queryByTestId('quick-action-file')).toBeNull();
  expect(screen.queryByLabelText('传文件')).toBeNull();
  expect(screen.getByTestId('quick-action-view-only')).toBeOnTheScreen();
  expect(screen.getByTestId('quick-action-terminal')).toBeOnTheScreen();
});

it('locks a host that has a live session', async () => {
  const { core } = await renderLoaded();

  await fireEvent.press(screen.getByTestId('quick-action-lock'));
  expect(core.calls).toContainEqual({
    method: 'sessions.action',
    args: ['s-live-1', 'lock', undefined],
  });
  expect(await screen.findByText('已锁定')).toBeOnTheScreen();
});

it('disables the lock action when the host has no live session', async () => {
  await renderLoaded(OFFICE_HOST_ID);

  expect(
    screen.getByTestId('quick-action-lock').props.accessibilityState,
  ).toMatchObject({
    disabled: true,
  });
});

it('says why locking is unavailable when the host has no live session', async () => {
  await renderLoaded(OFFICE_HOST_ID);
  const lock = screen.getByTestId('quick-action-lock');

  expect(lock).toHaveProp('accessibilityHint', '有会话进行中时才能锁定');
  await fireEvent.press(lock);
  expect(await screen.findByText('有会话进行中时才能锁定')).toBeOnTheScreen();
});

it('opens device routes from the connection address row', async () => {
  const { navigation } = await renderLoaded();

  await fireEvent.press(screen.getByLabelText('连接地址，在线 · 2 条地址'));
  expect(navigation.navigate).toHaveBeenCalledWith('DeviceRoutes', {
    hostId: STUDIO_HOST_ID,
  });
});

it('updates the sensitive host and lock-on-end settings', async () => {
  const rendered = await renderLoaded();
  const sensitive = screen.getByLabelText('连接前验证 Face ID');
  const lockOnEnd = screen.getByLabelText('会话结束后锁定远端屏幕');

  await fireEvent.press(sensitive);
  expect(rendered.settingsStore.get('lock.sensitiveHosts')).toEqual([
    STUDIO_HOST_ID,
  ]);
  expect(
    screen.getByLabelText('连接前验证 Face ID').props.accessibilityState,
  ).toMatchObject({
    checked: true,
  });

  await fireEvent.press(screen.getByLabelText('连接前验证 Face ID'));
  expect(rendered.settingsStore.get('lock.sensitiveHosts')).toEqual([]);

  await fireEvent.press(lockOnEnd);
  expect(
    rendered.settingsStore.get('hosts.prefs')[STUDIO_HOST_ID]?.lockOnEnd,
  ).toBe(true);
});

it('names Touch ID in the sensitive host switch', async () => {
  const platform = createFakePlatform();
  platform.authCapabilityResult = { biometry: 'touchId', passcode: true };
  await renderWithProviders(
    <DeviceSheetScreen navigation={mockNavigation()} route={route()} />,
    { platform, settings: { 'tailnet.enabled': true }, localDirectory: {} },
  );
  await screen.findByText('Studio Mac mini');

  expect(screen.getByLabelText('连接前验证 Touch ID')).toBeOnTheScreen();
});

it('forgets the host after confirmation and returns', async () => {
  const { core, navigation } = await renderLoaded();

  await fireEvent.press(screen.getByTestId('device-sheet-forget'));
  expect(screen.getByText('移除 Studio Mac mini？')).toBeOnTheScreen();
  const dialog = screen.getByTestId('forget-dialog-card');
  await fireEvent.press(within(dialog).getByText('移除这台电脑'));

  await waitFor(() => expect(navigation.goBack).toHaveBeenCalledTimes(1));
  expect(core.calls).toContainEqual({
    method: 'hosts.forget',
    args: [STUDIO_HOST_ID],
  });
});

it('keeps the host when the forget dialog is dismissed with 保留', async () => {
  const { core, navigation } = await renderLoaded();

  await fireEvent.press(screen.getByTestId('device-sheet-forget'));
  expect(
    screen.getByText('这台手机会忘掉它，之后要重新配对才能连接'),
  ).toBeOnTheScreen();
  await fireEvent.press(screen.getByTestId('forget-keep'));

  expect(screen.queryByTestId('forget-dialog-card')).toBeNull();
  expect(navigation.goBack).not.toHaveBeenCalled();
  expect(core.calls).not.toContainEqual(
    expect.objectContaining({ method: 'hosts.forget' }),
  );
});

it('shows the confirm button busy while forgetting and ignores a second tap', async () => {
  const core = createFakeCore();
  let finish: () => void = () => undefined;
  const forget = jest.spyOn(core.hosts, 'forget').mockImplementation(
    () =>
      new Promise<void>(resolve => {
        finish = resolve;
      }),
  );
  const { navigation } = await renderLoaded(STUDIO_HOST_ID, core);

  await fireEvent.press(screen.getByTestId('device-sheet-forget'));
  await fireEvent.press(screen.getByTestId('forget-confirm'));
  expect(
    screen.getByTestId('forget-confirm').props.accessibilityState,
  ).toMatchObject({ busy: true });
  expect(screen.getByTestId('forget-confirm-spinner')).toBeOnTheScreen();
  expect(screen.getByTestId('forget-keep')).toBeDisabled();
  await fireEvent.press(screen.getByTestId('forget-confirm'));
  expect(forget).toHaveBeenCalledTimes(1);
  expect(navigation.goBack).not.toHaveBeenCalled();

  finish();
  await waitFor(() => expect(navigation.goBack).toHaveBeenCalledTimes(1));
});

it('shows a toast and stays open when forgetting fails', async () => {
  const core = createFakeCore();
  core.failNext('hosts.forget', new Error('forget failed'));
  const { navigation } = await renderLoaded(STUDIO_HOST_ID, core);

  await fireEvent.press(screen.getByTestId('device-sheet-forget'));
  const dialog = screen.getByTestId('forget-dialog-card');
  await fireEvent.press(within(dialog).getByText('移除这台电脑'));

  expect(await screen.findByText('没能移除，稍后再试')).toBeOnTheScreen();
  expect(navigation.goBack).not.toHaveBeenCalled();
});

it('hides display selection and changes the primary label for one display', async () => {
  await renderLoaded(OFFICE_HOST_ID);

  expect(screen.queryByText('选择显示器')).toBeNull();
  expect(screen.getByText('开始控制')).toBeOnTheScreen();
});

it('disables the primary button for an offline host and says why', async () => {
  await renderLoaded(MACBOOK_HOST_ID);

  // H17: an offline computer cannot be reached or woken from here, so the
  // primary button no longer reads 连接.
  expect(screen.queryByText('连接')).toBeNull();
  expect(screen.getByText('电脑离线')).toBeOnTheScreen();
  expect(screen.getByTestId('device-sheet-start')).toBeDisabled();
  expect(screen.getByTestId('device-sheet-offline-reason')).toHaveTextContent(
    '等这台电脑开机联网后再连，App 还不能远程唤醒它。',
  );
  for (const id of ['quick-action-view-only', 'quick-action-terminal']) {
    const action = screen.getByTestId(id);
    expect(action.props.accessibilityState).toMatchObject({ disabled: true });
    expect(action).toHaveProp('accessibilityHint', '电脑离线，暂时用不了');
  }
});

it('shows a not-found state for an unknown host', async () => {
  await renderSheet('missing-host');

  expect(await screen.findByText('找不到这台设备')).toBeOnTheScreen();
  expect(screen.getByText('关闭')).toBeOnTheScreen();
});

it('closes from the sheet close button', async () => {
  const { navigation } = await renderLoaded();

  await fireEvent.press(screen.getByTestId('device-sheet-close'));
  expect(navigation.goBack).toHaveBeenCalledTimes(1);
});

it('offers no wake for an offline computer, which needs a wake relay', async () => {
  await renderLoaded(MACBOOK_HOST_ID);

  expect(screen.getByText('电脑离线')).toBeOnTheScreen();
  expect(screen.queryByText('唤醒这台电脑')).toBeNull();
});

it('does not offer wake for an online host', async () => {
  await renderLoaded(STUDIO_HOST_ID);

  expect(screen.queryByText('唤醒这台电脑')).toBeNull();
});

async function renderWithSettings(
  hostId: string,
  settings: NonNullable<Parameters<typeof renderWithProviders>[1]>['settings'],
  core: FakeCore = createFakeCore(),
) {
  const navigation = mockNavigation<'DeviceSheet'>();
  const rendered = await renderWithProviders(
    <DeviceSheetScreen navigation={navigation} route={route(hostId)} />,
    {
      core,
      settings: { 'tailnet.enabled': true, ...settings },
      kv: historyKv(),
      localDirectory: { peers: [] },
    },
  );
  await screen.findByText(
    hostId === OFFICE_HOST_ID ? '工位 Windows' : 'Studio Mac mini',
  );
  return { ...rendered, navigation };
}

describe('connection mode (V10 C8)', () => {
  it('shows the effective mode and writes each choice to hosts.prefs', async () => {
    const { settingsStore } = await renderLoaded(OFFICE_HOST_ID);
    const via = () =>
      settingsStore.get('hosts.prefs')[OFFICE_HOST_ID]?.connectVia;

    expect(
      screen.getByLabelText('连接方式，自动（先用系统共享）'),
    ).toBeOnTheScreen();

    await fireEvent.press(screen.getByTestId('connect-via-row'));
    const dialog = screen.getByTestId('connect-via-dialog-card');
    expect(within(dialog).getByText('只用远程桌面')).toBeOnTheScreen();
    expect(
      within(dialog).getByTestId('connect-via-auto').props.accessibilityState,
    ).toMatchObject({ checked: true, selected: true });
    await fireEvent.press(within(dialog).getByTestId('connect-via-native'));
    expect(via()).toBe('native');
    expect(screen.queryByTestId('connect-via-dialog-card')).toBeNull();
    expect(screen.getByLabelText('连接方式，只用远程桌面')).toBeOnTheScreen();

    await fireEvent.press(screen.getByTestId('connect-via-row'));
    expect(
      screen.getByTestId('connect-via-native').props.accessibilityState,
    ).toMatchObject({ checked: true, selected: true });
    await fireEvent.press(screen.getByTestId('connect-via-host'));
    expect(via()).toBe('host');
    expect(screen.getByLabelText('连接方式，只用 Host')).toBeOnTheScreen();

    await fireEvent.press(screen.getByTestId('connect-via-row'));
    await fireEvent.press(screen.getByTestId('connect-via-auto'));
    expect(via()).toBe('auto');
    expect(
      screen.getByLabelText('连接方式，自动（先用系统共享）'),
    ).toBeOnTheScreen();
  });

  it('names the Mac tool and says Host goes first when that is the global preference', async () => {
    await renderWithSettings(STUDIO_HOST_ID, { 'connect.prefer': 'host' });

    expect(
      screen.getByLabelText('连接方式，自动（先用 Host）'),
    ).toBeOnTheScreen();
    await fireEvent.press(screen.getByTestId('connect-via-row'));
    expect(screen.getByText('只用屏幕共享')).toBeOnTheScreen();
  });

  it('routes the primary button through useConnect to Remote Desktop', async () => {
    const core = createFakeCore();
    installFakeAdapters(core);
    const probe = jest.spyOn(core.discover!, 'probe').mockResolvedValue({
      addr: '100.64.0.12',
      port: 3389,
      outcome: 'open',
      ms: 3,
    });
    const { navigation } = await renderLoaded(OFFICE_HOST_ID, core);

    await fireEvent.press(screen.getByTestId('device-sheet-start'));
    // The session replaces the sheet, as the Host path does.
    await waitFor(() =>
      expect(mockRootDispatch).toHaveBeenCalledWith(
        StackActions.replace('AdapterSession', {
          target: {
            protocol: 'rdp',
            hostId: OFFICE_HOST_ID,
            addr: '100.64.0.12',
          },
          name: '工位 Windows',
          addrKind: 'tailnet',
          context: 'native',
        }),
      ),
    );
    expect(mockRootDispatch.mock.calls[0][0].type).toBe('REPLACE');
    expect(mockRootNavigate).not.toHaveBeenCalled();
    expect(probe).toHaveBeenCalledWith('100.64.0.12', 3389, 'rdp', 1500);
    expect(navigation.replace).not.toHaveBeenCalled();
  });

  it('falls back to the Host session when Remote Desktop is not reachable', async () => {
    const core = createFakeCore();
    installFakeAdapters(core);
    await renderLoaded(OFFICE_HOST_ID, core);

    await fireEvent.press(screen.getByTestId('device-sheet-start'));
    await waitFor(() =>
      expect(mockRootDispatch).toHaveBeenCalledWith(
        StackActions.replace('Session', {
          hostId: OFFICE_HOST_ID,
          notice: '没打开远程桌面，已改用 Host 连接',
        }),
      ),
    );
    expect(mockRootNavigate).not.toHaveBeenCalled();
    expect(core.calls).toContainEqual({
      method: 'discover.probe',
      args: ['100.64.0.12', 3389, 'rdp', 1500],
    });
  });

  it('keeps the Host session and display when the computer uses only Host', async () => {
    const core = createFakeCore();
    installFakeAdapters(core);
    const probe = jest.spyOn(core.discover!, 'probe');
    const { navigation } = await renderWithSettings(
      OFFICE_HOST_ID,
      { 'hosts.prefs': { [OFFICE_HOST_ID]: { connectVia: 'host' } } },
      core,
    );

    expect(screen.getByLabelText('连接方式，只用 Host')).toBeOnTheScreen();
    await fireEvent.press(screen.getByTestId('device-sheet-start'));
    expect(navigation.replace).toHaveBeenCalledWith('Session', {
      hostId: OFFICE_HOST_ID,
      displayId: 1,
    });
    expect(probe).not.toHaveBeenCalled();
    expect(mockRootNavigate).not.toHaveBeenCalled();
    expect(mockRootDispatch).not.toHaveBeenCalled();
  });

  it('shows the external-app guide that useConnect returns', async () => {
    mockGuide = { kind: 'rdp' };
    await renderLoaded(OFFICE_HOST_ID);
    const guide = screen.getByTestId('fallback-guide-card');
    expect(within(guide).getByText('需要 Windows App')).toBeOnTheScreen();
    expect(screen.getByTestId('device-sheet')).toBeOnTheScreen();

    await fireEvent.press(within(guide).getByRole('button', { name: '取消' }));
    expect(mockCloseGuide).toHaveBeenCalledTimes(1);
  });

  it('returns to the live Host session even with screen sharing available', async () => {
    const core = createFakeCore();
    installFakeAdapters(core);
    const probe = jest.spyOn(core.discover!, 'probe');
    const { navigation } = await renderLoaded(STUDIO_HOST_ID, core);

    await fireEvent.press(screen.getByTestId('device-sheet-start'));
    expect(navigation.replace).toHaveBeenCalledWith('Session', {
      hostId: STUDIO_HOST_ID,
      displayId: 1,
    });
    expect(probe).not.toHaveBeenCalled();
  });
});
