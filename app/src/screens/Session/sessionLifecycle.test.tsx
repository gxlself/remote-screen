import {
  act,
  cleanup,
  fireEvent,
  screen,
  within,
} from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import { setRecordingUi } from '../../features/recording/sessionRecording';
import { setViewport } from '../../features/session/videoStore';
import { MemoryKv } from '../../features/storage/kv';
import { session } from '../../i18n/zh/session';
import { resetKeyboardInset } from '../../native/keyboard/keyboardInset';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import type { SessionId, SessionPhase } from '../../native/rscore/types';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import SessionScreen from './index';
import * as ClipboardSync from './ClipboardSync';
import { TOOLBAR_HINT_KEY } from './useSessionController';
import type { SessionController } from './useSessionController';
import { TOOLBAR_IDLE_MS } from './useToolbarIdle';
import { belowTopStack } from './topStack';
import { StyleSheet } from 'react-native';

jest.mock('../../native/session/RemoteSessionView', () =>
  require('../../test/mocks/RemoteSessionViewMock'),
);

// 软键盘本身有原生输入组件；这里只关心会话页传给它的 leading 内容。
jest.mock('../../native/keyboard/SessionKeyboard', () => {
  const React = jest.requireActual<typeof import('react')>('react');
  const { View: MockView } =
    jest.requireActual<typeof import('react-native')>('react-native');
  function SessionKeyboard(props: { open: boolean; leading?: unknown }) {
    return props.open
      ? React.createElement(
          MockView,
          { testID: 'session-keyboard-mock' },
          props.leading as never,
        )
      : null;
  }
  return { __esModule: true, default: SessionKeyboard, SessionKeyboard };
});

const hostId = 'h7v2m9q3k8x4tb1a';
const clients: QueryClient[] = [];

async function advance(ms = 1) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

/** 截下传给插槽的 ctx（就是控制器本身），用来直接调动作；有会话就一直渲染。 */
function captureController(): () => SessionController {
  let ctx: SessionController | undefined;
  jest.spyOn(ClipboardSync, 'default').mockImplementation(props => {
    ctx = props.ctx as SessionController;
    return null;
  });
  return () => {
    if (ctx === undefined) {
      throw new Error('session slots were not rendered');
    }
    return ctx;
  };
}

async function mount(
  core: FakeCore,
  kv = new MemoryKv(),
  opts: { coachShown?: boolean } = {},
) {
  const navigation = mockNavigation<'Session'>();
  const route = mockRoute('Session', { hostId });
  const view = await renderWithProviders(
    <SessionScreen navigation={navigation} route={route} />,
    opts.coachShown === true
      ? { core, kv, settings: { 'session.coachShown': true } }
      : { core, kv },
  );
  clients.push(view.queryClient);
  await advance();
  return { ...view, navigation, kv };
}

async function state(core: FakeCore, phase: SessionPhase, id = 's1') {
  await act(() =>
    core.emit({ type: 'session.state', sessionId: id, hostId, phase }),
  );
}

async function stream(core: FakeCore) {
  await state(core, 'streaming');
  await fireEvent(
    screen.getByTestId('remote-session-view-placeholder'),
    'onFirstFrame',
  );
}

function closes(core: FakeCore): unknown[][] {
  return core.calls
    .filter(call => call.method === 'sessions.close')
    .map(call => call.args);
}

beforeEach(() => {
  jest.useFakeTimers();
  resetKeyboardInset();
});

afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  setRecordingUi({ sessionId: null });
  jest.restoreAllMocks();
  jest.useRealTimers();
});

describe('R01 no leftover background session', () => {
  it('closes a session whose open finishes after the user cancelled', async () => {
    const core = createFakeCore({ sessions: [] });
    let resolveOpen: (id: SessionId) => void = () => undefined;
    jest.spyOn(core.sessions, 'open').mockImplementation(
      () =>
        new Promise<SessionId>(resolve => {
          resolveOpen = resolve;
        }),
    );
    const view = await mount(core);

    await fireEvent.press(
      within(screen.getByTestId('connecting-overlay')).getByLabelText('取消'),
    );
    expect(view.navigation.goBack).toHaveBeenCalledTimes(1);
    expect(closes(core)).toEqual([]);

    await act(async () => {
      resolveOpen('s9');
    });
    await advance();
    expect(closes(core)).toEqual([['s9']]);
  });

  it('closes a session that is still connecting when the screen goes away', async () => {
    const core = createFakeCore({ sessions: [] });
    const view = await mount(core);
    await state(core, 'connecting');
    expect(closes(core)).toEqual([]);

    await act(async () => {
      await view.unmount();
    });
    expect(closes(core)).toEqual([['s1']]);
  });

  it('keeps a streaming session alive when the screen is collapsed', async () => {
    const core = createFakeCore({ sessions: [] });
    const ctx = captureController();
    const view = await mount(core);
    await stream(core);
    await act(() => ctx().collapse());
    await act(async () => {
      await view.unmount();
    });
    expect(closes(core)).toEqual([]);
  });

  it('keeps a session collapsed while connecting', async () => {
    const core = createFakeCore({ sessions: [] });
    const view = await mount(core);
    await state(core, 'connecting');
    await fireEvent.press(screen.getByTestId('connect-collapse'));
    await act(async () => {
      await view.unmount();
    });
    expect(closes(core)).toEqual([]);
  });
});

describe('R02 disconnect confirmation', () => {
  it('ends a connecting session right away without asking', async () => {
    const core = createFakeCore({ sessions: [] });
    const ctx = captureController();
    await mount(core);
    await state(core, 'connecting');
    await act(() => ctx().disconnect());
    expect(screen.queryByTestId('disconnect-confirm')).toBeNull();
    expect(closes(core)).toEqual([['s1']]);
  });

  it('asks before disconnecting a connected session and can be cancelled', async () => {
    const core = createFakeCore({ sessions: [] });
    const ctx = captureController();
    await mount(core);
    await stream(core);

    await act(() => ctx().disconnect());
    expect(screen.getByText('断开 Studio Mac mini？')).toBeOnTheScreen();
    expect(screen.getByText(session.disconnect.body)).toBeOnTheScreen();
    await fireEvent.press(
      within(screen.getByTestId('disconnect-confirm-actions')).getByText(
        '取消',
      ),
    );
    expect(screen.queryByText('断开 Studio Mac mini？')).toBeNull();
    expect(closes(core)).toEqual([]);

    await act(() => ctx().disconnect());
    await fireEvent.press(
      within(screen.getByTestId('disconnect-confirm-actions')).getByText(
        '断开',
      ),
    );
    expect(closes(core)).toEqual([['s1']]);
  });

  it('says the recording will stop when disconnecting while recording', async () => {
    const core = createFakeCore({ sessions: [] });
    const ctx = captureController();
    await mount(core);
    await stream(core);
    await act(async () => {
      setRecordingUi({ sessionId: 's1' });
      setRecordingUi({ sessionId: 's1', state: 'recording', durationMs: 5000 });
    });

    await act(() => ctx().disconnect());
    expect(
      screen.getByText(session.disconnect.bodyRecording),
    ).toBeOnTheScreen();
  });
});

describe('R03 hidden toolbar', () => {
  it('leaves a handle after three-finger hide and hints only the first time', async () => {
    const core = createFakeCore({ sessions: [] });
    const ctx = captureController();
    const view = await mount(core);
    await stream(core);
    expect(screen.queryByTestId('toolbar-handle')).toBeNull();

    await act(() => ctx().onGesture({ kind: 'threeFingerTap' }));
    expect(ctx().toolbarHidden).toBe(true);
    expect(screen.getByTestId('toolbar-handle')).toBeOnTheScreen();
    expect(screen.getByText(session.toolbar.hiddenHint)).toBeOnTheScreen();
    expect(view.kv.getString(TOOLBAR_HINT_KEY)).toBe('1');

    await fireEvent.press(screen.getByLabelText(session.toolbar.handle));
    expect(ctx().toolbarHidden).toBe(false);
    expect(screen.queryByTestId('toolbar-handle')).toBeNull();

    await advance(4000);
    await act(() => ctx().onGesture({ kind: 'threeFingerTap' }));
    expect(screen.getByTestId('toolbar-handle')).toBeOnTheScreen();
    expect(screen.queryByText(session.toolbar.hiddenHint)).toBeNull();
  });
});

describe('R09 quality changes', () => {
  it('rolls back and explains when the quality change fails', async () => {
    const core = createFakeCore({ sessions: [] });
    const ctx = captureController();
    const view = await mount(core);
    await stream(core);
    core.failNext('sessions.setQuality', new Error('closed'));

    await act(() => ctx().setQuality('eco'));
    expect(ctx().quality).toBe('auto');
    expect(screen.getByText('画质没改成，还是「自动」')).toBeOnTheScreen();
    expect(view.kv.getString(`session.quality.${hostId}`)).toBeUndefined();

    // 失败提示带「重试」，点了再改一次。
    await fireEvent.press(screen.getByTestId('toast-action'));
    expect(ctx().quality).toBe('eco');
    expect(
      core.calls.filter(call => call.method === 'sessions.setQuality'),
    ).toHaveLength(2);
    expect(view.kv.getString(`session.quality.${hostId}`)).toBe('eco');
  });

  it('confirms the change and remembers it for the next connection', async () => {
    const core = createFakeCore({ sessions: [] });
    const ctx = captureController();
    const kv = new MemoryKv();
    const view = await mount(core, kv);
    await stream(core);

    await act(() => ctx().setQuality('eco'));
    expect(ctx().quality).toBe('eco');
    expect(screen.getByText('画质已改为「省流量」')).toBeOnTheScreen();
    await act(async () => {
      await view.unmount();
    });

    const next = createFakeCore({ sessions: [] });
    await mount(next, kv);
    const open = next.calls.find(call => call.method === 'sessions.open');
    expect(open?.args[1]).toMatchObject({ quality: 'eco' });
  });
});

describe('R06 retry a slow connection', () => {
  it('closes the old session and opens a new one', async () => {
    const core = createFakeCore({ sessions: [] });
    const ctx = captureController();
    await mount(core);
    await state(core, 'streaming');

    await act(() => ctx().retryConnect());
    await advance();
    expect(closes(core)).toEqual([['s1']]);
    expect(
      core.calls.filter(call => call.method === 'sessions.open'),
    ).toHaveLength(2);
    expect(ctx().sessionId).toBe('s2');
  });
});

describe('R24 toolbar fades when idle', () => {
  it('fades after idle, lets touches through and comes back on touch', async () => {
    const core = createFakeCore({ sessions: [] });
    // 首次连接会弹手势说明，那时不淡出；这里按已看过说明来测。
    await mount(core, new MemoryKv(), { coachShown: true });
    await stream(core);
    const layer = () => screen.getByTestId('layer-toolbar');
    expect(layer()).toHaveProp('pointerEvents', 'box-none');

    await advance(TOOLBAR_IDLE_MS);
    expect(layer()).toHaveProp('pointerEvents', 'none');
    expect(layer()).toHaveStyle({ opacity: 0 });

    await fireEvent(screen.getByTestId('session-root'), 'touchStart');
    expect(layer()).toHaveProp('pointerEvents', 'box-none');
    expect(layer()).not.toHaveStyle({ opacity: 0 });
  });

  it('stays visible while the keyboard is open', async () => {
    const core = createFakeCore({ sessions: [] });
    const ctx = captureController();
    await mount(core, new MemoryKv(), { coachShown: true });
    await stream(core);
    await act(() => ctx().toggleKeyboard());
    await advance(TOOLBAR_IDLE_MS * 2);
    expect(screen.getByTestId('layer-toolbar')).toHaveProp(
      'pointerEvents',
      'box-none',
    );
  });

  it('stays visible while the gesture help is showing', async () => {
    const core = createFakeCore({ sessions: [] });
    const ctx = captureController();
    await mount(core);
    await stream(core);
    expect(ctx().coachVisible).toBe(true);
    await advance(TOOLBAR_IDLE_MS * 2);
    expect(screen.getByTestId('layer-toolbar')).toHaveProp(
      'pointerEvents',
      'box-none',
    );
  });
});

describe('R42 follow-zoom chip', () => {
  it('only appears in the key bar while the picture is zoomed', async () => {
    const core = createFakeCore({ sessions: [] });
    const ctx = captureController();
    await mount(core);
    await stream(core);
    await act(() => ctx().toggleKeyboard());
    expect(screen.queryByTestId('follow-zoom-chip')).toBeNull();

    await act(async () => {
      setViewport('s1', { zoom: 2, x: 0.5, y: 0.5 });
    });
    expect(screen.getByTestId('follow-zoom-chip')).toBeOnTheScreen();
  });
});

/** 往上找第一个绝对定位的容器，取它的 top。 */
function absoluteTop(node: ReturnType<typeof screen.getByText>): unknown {
  let current: typeof node | null = node;
  while (current !== null) {
    const style = StyleSheet.flatten(current.props.style) as
      | { position?: string; top?: number }
      | undefined;
    if (style?.position === 'absolute' && style.top !== undefined) {
      return style.top;
    }
    current = current.parent;
  }
  return undefined;
}

describe('R37 top notices do not overlap', () => {
  it('puts the toast below the measured notice stack', async () => {
    const core = createFakeCore({ sessions: [] });
    const ctx = captureController();
    await mount(core, new MemoryKv(), { coachShown: true });
    await stream(core);

    await act(() => ctx().setQuality('eco'));
    expect(absoluteTop(screen.getByText('画质已改为「省流量」'))).toBe(
      belowTopStack(0),
    );

    await fireEvent(screen.getByTestId('top-stack'), 'layout', {
      nativeEvent: { layout: { x: 0, y: 0, width: 400, height: 66 } },
    });
    expect(absoluteTop(screen.getByText('画质已改为「省流量」'))).toBe(
      belowTopStack(66),
    );
  });
});
