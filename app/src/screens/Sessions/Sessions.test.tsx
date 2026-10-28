import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
  within,
} from '@testing-library/react-native';
import type { ReactElement } from 'react';
import type { TabScreenProps } from '../../app/navigation/routes';
import {
  adapterLive,
  resetAdapterLive,
} from '../../features/adapters/liveStore';
import type { CollapsedAdapter } from '../../features/adapters/liveStore';
import { installFakeAdapters } from '../../features/adapters/testing/fakeAdapters';
import HistoryRecorder from '../../features/sessions/HistoryRecorder';
import {
  appendHistory,
  HISTORY_KEY,
  readHistory,
} from '../../features/sessions/history';
import type { HistoryEntry } from '../../features/sessions/history';
import { resetSshStore } from '../../features/sessions/sshStore';
import { MemoryKv } from '../../features/storage/kv';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { FIXTURE_NOW } from '../../native/rscore/fixtures';
import type { CoreEventOf } from '../../native/rscore/types';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import SessionsScreen from '.';

// useConnect navigates through the root ref; route it to the screen's mock.
let mockRootNavigate: jest.Mock | undefined;
jest.mock('../../app/navigation/navigationRef', () => ({
  navigationRef: {
    isReady: () => true,
    navigate: (...args: unknown[]) => mockRootNavigate?.(...args),
  },
}));

const STUDIO_HOST_ID = 'h7v2m9q3k8x4tb1a';
const OFFICE_HOST_ID = 'w3k8p2x7m4q9tb6c';
const LIVE_SESSION_ID = 's-live-1';
const MINUTE = 60 * 1000 * 1000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

const stats: CoreEventOf<'session.stats'> = {
  type: 'session.stats',
  sessionId: LIVE_SESSION_ID,
  rttMs: 6,
  fps: 60,
  bitrateKbps: 8000,
  decodeMs: 3,
  lossPct: 0,
  width: 2560,
  height: 1440,
  codec: 'hevc',
  path: 'tailnetDirect',
  transport: 'quic',
};

function historyEntry(overrides: Partial<HistoryEntry> = {}): HistoryEntry {
  return {
    id: 'remote:office',
    kind: 'remote',
    targetId: OFFICE_HOST_ID,
    name: '工位 Windows',
    startedAtUs: FIXTURE_NOW - 2 * HOUR,
    endedAtUs: FIXTURE_NOW - DAY,
    durationS: 4320,
    path: 'tailnetDerp',
    derpRegion: '上海',
    ...overrides,
  };
}

function seededKv(
  entries: HistoryEntry[] = [
    historyEntry(),
    historyEntry({
      id: 'ssh:prod-api',
      kind: 'ssh',
      targetId: 'prod-api-01',
      name: 'prod-api-01',
      endedAtUs: FIXTURE_NOW - 2 * DAY,
      durationS: 18 * 60,
      path: undefined,
      derpRegion: undefined,
    }),
  ],
): MemoryKv {
  const kv = new MemoryKv();
  for (const entry of entries) {
    appendHistory(kv, entry);
  }
  return kv;
}

async function emit(
  core: ReturnType<typeof createFakeCore>,
  event: Parameters<typeof core.emit>[0],
) {
  await act(async () => {
    core.emit(event);
    await jest.advanceTimersByTimeAsync(1);
  });
}

async function advance(ms: number) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

async function mount(
  opts: {
    core?: ReturnType<typeof createFakeCore>;
    kv?: MemoryKv;
    ssh?: boolean;
  } = {},
) {
  const core = opts.core ?? createFakeCore();
  const navigation = mockNavigation<'MainTabs'>();
  mockRootNavigate = navigation.navigate as jest.Mock;
  const route = mockRoute('MainTabs');
  const props = {
    navigation,
    route,
  } as unknown as TabScreenProps<'Sessions'>;
  const ui: ReactElement =
    opts.ssh === false ? (
      <SessionsScreen {...props} />
    ) : (
      <>
        <HistoryRecorder />
        <SessionsScreen {...props} />
      </>
    );
  const view = await renderWithProviders(ui, {
    core,
    kv: opts.kv ?? seededKv(),
    // The paired fixture computers, which history entries reconnect to.
    localDirectory: {},
  });
  // Lets the computer list load, which names sessions and history entries.
  await act(async () => {
    await jest.advanceTimersByTimeAsync(1);
  });
  return { ...view, navigation };
}

async function openTerminals(core: ReturnType<typeof createFakeCore>) {
  await emit(core, {
    type: 'ssh.state',
    sshId: 'ssh-train',
    serverId: 'gpu-train-02',
    state: 'open',
  });
  await emit(core, {
    type: 'ssh.state',
    sshId: 'ssh-api',
    serverId: 'prod-api-01',
    state: 'open',
  });
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(new Date(FIXTURE_NOW / 1000));
  jest.spyOn(Date.prototype, 'getTimezoneOffset').mockReturnValue(-480);
  resetSshStore();
});

afterEach(async () => {
  await cleanup();
  mockRootNavigate = undefined;
  resetSshStore();
  resetAdapterLive();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('shows the live total, remote card and ticking clock', async () => {
  const view = await mount();
  await openTerminals(view.core);
  await emit(view.core, stats);

  expect(screen.getByText('进行中 · 3')).toBeOnTheScreen();
  expect(screen.getAllByText('Studio Mac mini').length).toBeGreaterThan(0);
  expect(screen.getByText('控制中')).toBeOnTheScreen();
  expect(screen.getByText('42:18')).toBeOnTheScreen();
  expect(screen.getByText('直连')).toBeOnTheScreen();
  expect(screen.getByText('6 ms')).toBeOnTheScreen();
  expect(screen.getByText('· 60 fps')).toBeOnTheScreen();

  await advance(2000);
  expect(screen.getByText('42:20')).toBeOnTheScreen();
});

it('navigates back from both the preview and the return button', async () => {
  const view = await mount();
  await openTerminals(view.core);

  await fireEvent.press(
    screen.getByLabelText('回到 Studio Mac mini 的远控会话'),
  );
  expect(view.navigation.navigate).toHaveBeenCalledWith('Session', {
    hostId: STUDIO_HOST_ID,
    sessionId: LIVE_SESSION_ID,
  });

  await fireEvent.press(screen.getByText('回到会话'));
  expect(view.navigation.navigate).toHaveBeenCalledWith('Session', {
    hostId: STUDIO_HOST_ID,
    sessionId: LIVE_SESSION_ID,
  });
});

it('disconnects a single remote session after confirming', async () => {
  const view = await mount();
  await openTerminals(view.core);

  await fireEvent.press(
    screen.getByLabelText('断开 Studio Mac mini 的远控会话'),
  );
  // H25: one tap no longer ends the session; it asks first.
  const dialog = screen.getByTestId('disconnect-one-dialog-card');
  expect(within(dialog).getByText('断开 Studio Mac mini？')).toBeOnTheScreen();
  expect(within(dialog).getByText('要再用时得重新连接。')).toBeOnTheScreen();
  expect(view.core.calls).not.toContainEqual(
    expect.objectContaining({ method: 'sessions.close' }),
  );

  await fireEvent.press(screen.getByTestId('disconnect-one-confirm'));
  expect(view.core.calls).toContainEqual({
    method: 'sessions.close',
    args: [LIVE_SESSION_ID],
  });
  expect(screen.queryByTestId('disconnect-one-dialog-card')).toBeNull();
});

it('keeps the remote session when the single disconnect is cancelled', async () => {
  const view = await mount();

  await fireEvent.press(
    screen.getByLabelText('断开 Studio Mac mini 的远控会话'),
  );
  const dialog = screen.getByTestId('disconnect-one-dialog-card');
  await fireEvent.press(within(dialog).getByText('取消'));

  expect(screen.queryByTestId('disconnect-one-dialog-card')).toBeNull();
  expect(view.core.calls).not.toContainEqual(
    expect.objectContaining({ method: 'sessions.close' }),
  );
});

it('opens a live terminal at its SSH id', async () => {
  const view = await mount();
  await openTerminals(view.core);

  expect(screen.getByText('gpu-train-02')).toBeOnTheScreen();
  expect(screen.getAllByText('tmux · 断网后自动接回')).toHaveLength(2);

  await fireEvent.press(screen.getByTestId('ssh-row-ssh-train'));
  expect(view.navigation.navigate).toHaveBeenCalledWith('Terminal', {
    serverId: 'gpu-train-02',
    sshId: 'ssh-train',
  });
});

it('reconnects a recent host manually without a session id', async () => {
  const view = await mount();

  expect(screen.getByText('最近')).toBeOnTheScreen();
  expect(
    screen.getByLabelText('重新连接工位 Windows，远控，1 小时 12 分，昨天'),
  ).toBeOnTheScreen();
  await fireEvent.press(
    screen.getByLabelText('重新连接工位 Windows，远控，1 小时 12 分，昨天'),
  );
  // H26: the entry goes through useConnect, like tapping the computer on Home.
  await waitFor(() =>
    expect(view.navigation.navigate).toHaveBeenCalledWith('Session', {
      hostId: OFFICE_HOST_ID,
    }),
  );
});

it('opens a recent terminal at its server without an SSH id', async () => {
  const view = await mount({
    core: createFakeCore({ sessions: [] }),
    kv: seededKv([
      historyEntry({
        id: 'ssh:build',
        kind: 'ssh',
        targetId: 'manual-build-srv',
        name: 'build-srv',
        endedAtUs: FIXTURE_NOW - 3 * DAY,
        durationS: 18 * 60,
        path: undefined,
        derpRegion: undefined,
        unclean: true,
      }),
    ]),
  });

  expect(screen.getByText('build-srv')).toBeOnTheScreen();
  expect(screen.getByText('终端 · 18 分钟 · 上次没正常结束')).toBeOnTheScreen();
  expect(screen.getByText('10月17日')).toBeOnTheScreen();
  await fireEvent.press(screen.getByTestId('history-row-ssh:build'));
  expect(view.navigation.navigate).toHaveBeenCalledWith('Terminal', {
    serverId: 'manual-build-srv',
  });
});

it('shows the current display for a multi-display session', async () => {
  const view = await mount();
  await openTerminals(view.core);
  await act(async () => {
    view.core.emit({
      type: 'session.displays',
      sessionId: LIVE_SESSION_ID,
      currentId: 2,
      displays: [
        {
          id: 1,
          name: 'Studio Display',
          widthPx: 2560,
          heightPx: 1600,
          primary: true,
        },
        {
          id: 2,
          name: '竖屏显示器',
          widthPx: 2160,
          heightPx: 3840,
          primary: false,
        },
      ],
    });
  });

  expect(screen.getByText('显示器 2 / 2')).toBeOnTheScreen();
});

it('confirms and disconnects every live remote and terminal', async () => {
  const view = await mount();
  await openTerminals(view.core);

  expect(screen.getByText('进行中 · 3')).toBeOnTheScreen();
  await fireEvent.press(screen.getByLabelText('全部断开'));

  const dialog = screen.getByTestId('disconnect-all-dialog-card');
  expect(within(dialog).getByText('断开全部 3 个会话？')).toBeOnTheScreen();
  expect(
    within(dialog).getByText('远控和终端都会断开，tmux 里的程序不受影响。'),
  ).toBeOnTheScreen();

  await fireEvent.press(within(dialog).getByText('全部断开'));
  expect(
    view.core.calls.filter(call => call.method === 'sessions.close'),
  ).toEqual([{ method: 'sessions.close', args: [LIVE_SESSION_ID] }]);
  expect(view.core.calls.filter(call => call.method === 'ssh.close')).toEqual([
    { method: 'ssh.close', args: ['ssh-train'] },
    { method: 'ssh.close', args: ['ssh-api'] },
  ]);
});

it('shows the empty state and sends the user to devices', async () => {
  const view = await mount({
    core: createFakeCore({ sessions: [] }),
    kv: new MemoryKv(),
    ssh: false,
  });

  expect(screen.getByText('还没有会话')).toBeOnTheScreen();
  expect(
    screen.getByText('在「设备」里点一台电脑开始远控，或点服务器打开终端。'),
  ).toBeOnTheScreen();
  expect(screen.queryByLabelText('全部断开')).toBeNull();

  await fireEvent.press(screen.getByText('去设备'));
  expect(view.navigation.navigate).toHaveBeenCalledWith('Home');
});

it('keeps the history section visible with no live sessions', async () => {
  await mount({
    core: createFakeCore({ sessions: [] }),
    kv: seededKv([
      historyEntry(),
      historyEntry({
        id: 'ssh:build',
        kind: 'ssh',
        targetId: 'manual-build-srv',
        name: 'build-srv',
        endedAtUs: FIXTURE_NOW - 3 * DAY,
        durationS: 18 * 60,
        path: undefined,
        derpRegion: undefined,
        unclean: true,
      }),
    ]),
  });

  expect(screen.queryByText(/进行中/)).toBeNull();
  expect(screen.getByText('最近')).toBeOnTheScreen();
  expect(screen.getByText('工位 Windows')).toBeOnTheScreen();
  expect(screen.getByText(/上次没正常结束/)).toBeOnTheScreen();
  expect(screen.queryByLabelText('全部断开')).toBeNull();
});

describe('recent history', () => {
  /** Remote sessions with the office PC, newest first, a minute apart. */
  function officeRuns(count: number): HistoryEntry[] {
    return Array.from({ length: count }, (_, index) =>
      historyEntry({
        id: `remote:office-${index}`,
        endedAtUs: FIXTURE_NOW - (count - index) * MINUTE,
        startedAtUs: FIXTURE_NOW - (count - index) * MINUTE - 60_000_000,
        durationS: 60,
      }),
    );
  }

  it('shows the spinner on the entry it is reconnecting and holds the others', async () => {
    const core = createFakeCore({ sessions: [] });
    installFakeAdapters(core);
    // Remote Desktop's reachability probe never answers.
    jest
      .spyOn(core.discover!, 'probe')
      .mockReturnValue(new Promise(() => undefined));
    const view = await mount({
      core,
      ssh: false,
      kv: seededKv([
        historyEntry(),
        historyEntry({
          id: 'remote:studio',
          targetId: STUDIO_HOST_ID,
          name: 'Studio Mac mini',
          endedAtUs: FIXTURE_NOW - 2 * DAY,
        }),
      ]),
    });

    await fireEvent.press(screen.getByTestId('history-row-remote:office'));
    expect(
      await screen.findByTestId('history-busy-remote:office'),
    ).toBeOnTheScreen();
    expect(screen.getByTestId('history-row-remote:studio')).toBeDisabled();
    await fireEvent.press(screen.getByTestId('history-row-remote:studio'));
    expect(view.navigation.navigate).not.toHaveBeenCalled();
  });

  it('says so when the computer of an entry is no longer listed', async () => {
    const view = await mount({
      core: createFakeCore({ sessions: [] }),
      ssh: false,
      kv: seededKv([
        historyEntry({
          id: 'adapter:gone',
          kind: 'adapter',
          protocol: 'rdp',
          targetId: 'manual:gone-pc',
          name: '旧电脑',
          path: undefined,
          derpRegion: undefined,
        }),
      ]),
    });

    expect(screen.getByText('旧电脑')).toBeOnTheScreen();
    await fireEvent.press(screen.getByTestId('history-row-adapter:gone'));
    expect(await screen.findByText('这台电脑已不在列表里')).toBeOnTheScreen();
    expect(view.navigation.navigate).not.toHaveBeenCalled();
  });

  it('shows ten entries until asked for the rest', async () => {
    await mount({
      core: createFakeCore({ sessions: [] }),
      ssh: false,
      kv: seededKv(officeRuns(12)),
    });

    expect(screen.getAllByTestId(/^history-row-/)).toHaveLength(10);
    await fireEvent.press(screen.getByText('显示全部 12 条'));
    expect(screen.getAllByTestId(/^history-row-/)).toHaveLength(12);
    await fireEvent.press(screen.getByText('收起'));
    expect(screen.getAllByTestId(/^history-row-/)).toHaveLength(10);
  });

  it('does not offer more for ten entries or fewer', async () => {
    await mount({
      core: createFakeCore({ sessions: [] }),
      ssh: false,
      kv: seededKv(officeRuns(10)),
    });

    expect(screen.getAllByTestId(/^history-row-/)).toHaveLength(10);
    expect(screen.queryByTestId('history-more')).toBeNull();
  });

  it('clears the history from this phone after confirming', async () => {
    const kv = seededKv();
    await mount({ core: createFakeCore({ sessions: [] }), ssh: false, kv });

    await fireEvent.press(screen.getByTestId('history-clear'));
    const dialog = screen.getByTestId('history-clear-dialog-card');
    expect(within(dialog).getByText('清除最近记录？')).toBeOnTheScreen();
    expect(
      within(dialog).getByText('只清除这台手机上的记录，不影响电脑。'),
    ).toBeOnTheScreen();
    await fireEvent.press(screen.getByTestId('history-clear-confirm'));

    expect(screen.queryByText('最近')).toBeNull();
    expect(kv.getString(HISTORY_KEY)).toBeUndefined();
    expect(screen.getByText('还没有会话')).toBeOnTheScreen();
  });

  it('keeps the history when clearing is cancelled', async () => {
    const kv = seededKv();
    await mount({ core: createFakeCore({ sessions: [] }), ssh: false, kv });

    await fireEvent.press(screen.getByTestId('history-clear'));
    const dialog = screen.getByTestId('history-clear-dialog-card');
    await fireEvent.press(within(dialog).getByText('取消'));

    expect(screen.getByText('最近')).toBeOnTheScreen();
    expect(screen.getAllByTestId(/^history-row-/)).toHaveLength(2);
  });
});

describe('folded-away screen sharing (V33)', () => {
  const ADAPTER_ID = '4611686018427387905';
  const target = { protocol: 'vnc' as const, hostId: 'mac-lab', addr: '100.64.0.7' };

  function collapsed(overrides: Partial<CollapsedAdapter> = {}): CollapsedAdapter {
    return {
      sessionId: ADAPTER_ID,
      params: { target, name: '实验室 Mac', addrKind: 'tailnet' },
      name: '实验室 Mac',
      phase: 'streaming',
      error: null,
      screens: [],
      screenIndex: 0,
      mode: 'trackpad',
      startedAtUs: FIXTURE_NOW - 5 * MINUTE,
      collapsedAtUs: FIXTURE_NOW,
      history: {
        live: {
          id: `adapter:${ADAPTER_ID}`,
          kind: 'adapter',
          targetId: 'host:mac-lab',
          name: '实验室 Mac',
          startedAtUs: FIXTURE_NOW - 5 * MINUTE,
          protocol: 'vnc',
        },
        stoppedAtUs: null,
        error: null,
        name: '实验室 Mac',
        targetId: 'host:mac-lab',
      },
      ...overrides,
    };
  }

  async function mountWithAdapter(overrides: Partial<CollapsedAdapter> = {}) {
    const core = createFakeCore({ sessions: [] });
    const fake = installFakeAdapters(core);
    adapterLive.put(collapsed(overrides));
    const kv = new MemoryKv();
    const view = await mount({ core, kv });
    return { ...view, fake, kv };
  }

  it('lists it under 进行中 and returns to it without a new connection', async () => {
    const view = await mountWithAdapter();

    expect(screen.getByText('进行中 · 1')).toBeOnTheScreen();
    expect(screen.getByText('实验室 Mac')).toBeOnTheScreen();
    expect(screen.getByText('屏幕共享')).toBeOnTheScreen();
    expect(screen.getByText('控制中')).toBeOnTheScreen();
    expect(screen.getByText('05:00')).toBeOnTheScreen();

    await fireEvent.press(screen.getByText('回到会话'));
    expect(view.navigation.navigate).toHaveBeenCalledWith('AdapterSession', {
      target,
      name: '实验室 Mac',
      addrKind: 'tailnet',
      sessionId: ADAPTER_ID,
    });
  });

  it('says when it dropped while away', async () => {
    await mountWithAdapter({
      phase: 'ended',
      error: { code: 'timeout', rsCode: 200, message: 'lost' },
    });

    expect(screen.getByText('已断开')).toBeOnTheScreen();
  });

  it('disconnects it only after confirming, and writes 最近', async () => {
    const view = await mountWithAdapter();

    await fireEvent.press(screen.getByLabelText('断开 实验室 Mac 的远控会话'));
    const dialog = screen.getByTestId('disconnect-one-dialog-card');
    expect(within(dialog).getByText('断开 实验室 Mac？')).toBeOnTheScreen();
    expect(view.fake.calls.filter(call => call.method === 'close')).toEqual([]);

    await fireEvent.press(screen.getByTestId('disconnect-one-confirm'));
    expect(view.fake.calls.filter(call => call.method === 'close')).toEqual([
      { method: 'close', args: [ADAPTER_ID] },
    ]);
    expect(adapterLive.peek(ADAPTER_ID)).toBeUndefined();
    expect(screen.queryByTestId(`remote-card-${ADAPTER_ID}`)).toBeNull();
    // It moves to 最近.
    expect(readHistory(view.kv)).toEqual([
      expect.objectContaining({ kind: 'adapter', name: '实验室 Mac' }),
    ]);
  });

  it('全部断开 includes it', async () => {
    const view = await mountWithAdapter();

    await fireEvent.press(screen.getByLabelText('全部断开'));
    await fireEvent.press(
      within(screen.getByTestId('disconnect-all-dialog-card')).getByText(
        '全部断开',
      ),
    );
    expect(view.fake.calls.filter(call => call.method === 'close')).toEqual([
      { method: 'close', args: [ADAPTER_ID] },
    ]);
    expect(adapterLive.peek(ADAPTER_ID)).toBeUndefined();
  });

  it('a 最近 row for the same computer returns to it', async () => {
    const core = createFakeCore({ sessions: [] });
    installFakeAdapters(core);
    adapterLive.put(collapsed());
    const view = await mount({
      core,
      kv: seededKv([
        historyEntry({
          id: 'adapter:old',
          kind: 'adapter',
          protocol: 'vnc',
          targetId: 'host:mac-lab',
          name: '实验室 Mac',
          path: undefined,
          derpRegion: undefined,
        }),
      ]),
    });

    await fireEvent.press(screen.getByTestId('history-row-adapter:old'));
    expect(view.navigation.navigate).toHaveBeenCalledWith(
      'AdapterSession',
      expect.objectContaining({ sessionId: ADAPTER_ID }),
    );
  });
});
