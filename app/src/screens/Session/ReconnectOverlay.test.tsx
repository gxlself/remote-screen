import { act, fireEvent } from '@testing-library/react-native';
import { Dimensions, StyleSheet } from 'react-native';
import type { SessionRecord } from '../../features/session/store';
import {
  computers,
  liveSession,
  sshServers,
} from '../../native/rscore/fixtures';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import { renderWithProviders } from '../../test/render';
import { ReconnectOverlay } from './ReconnectOverlay';
import type { SessionCtx } from './slots';

type Options = NonNullable<Parameters<typeof renderWithProviders>[1]>;

const BEIJING_102431_MS = Date.UTC(2026, 9, 21, 2, 24, 31);

function record(overrides: Partial<SessionRecord> = {}): SessionRecord {
  return {
    ...liveSession,
    controller: null,
    viewers: [],
    inputPausedLocal: false,
    stats: null,
    displays: [],
    dial: [],
    approvalTimeoutS: null,
    derpRegion: null,
    reconnectReason: 'pathLost',
    idleWarning: null,
    controlRequest: null,
    lastError: null,
    clipboard: [],
    phase: 'reconnecting',
    ...overrides,
  };
}

function context(
  rec: SessionRecord,
  actions: { openTerminal: jest.Mock; disconnect: jest.Mock },
): SessionCtx {
  return {
    hostId: rec.hostId,
    hostName: rec.hostName,
    os: 'macos',
    host: undefined,
    myDeviceId: null,
    videoRef: { current: null },
    sessionId: rec.id,
    rec,
    view: {
      overlay: 'reconnecting',
      end: { kind: 'stay' },
      role: 'controller',
      inputBlocked: true,
      shared: null,
      localUse: false,
      relay: false,
      tcp: false,
      idle: false,
      toolbar: {
        displayLabel: null,
        displayA11y: null,
        qualityLabel: '自动',
        qualityA11y: '画质：自动',
        showCtrlAltDel: false,
        showModeSwitch: true,
        keyboardEnabled: false,
      },
    },
    hud: {
      path: 'tailnetDirect',
      pathText: '直连',
      latencyMs: 6,
      fps: 60,
      tcp: false,
      alertText: null,
      a11y: '连接质量：直连 6 毫秒，60 帧',
    },
    isPad: false,
    reduceMotion: true,
    mode: 'trackpad',
    setMode: jest.fn(),
    quality: 'auto',
    setQuality: jest.fn(),
    startedAtMs: BEIJING_102431_MS,
    phaseAtMs: { reconnecting: BEIJING_102431_MS },
    firstFrameAtMs: null,
    openSheet: jest.fn(),
    closeSheet: jest.fn(),
    openCoach: jest.fn(),
    closeCoach: jest.fn(),
    keyboardOpen: false,
    toggleKeyboard: jest.fn(),
    toolbarHidden: false,
    collapse: jest.fn(),
    disconnect: actions.disconnect,
    cancelConnect: jest.fn(),
    openTerminal: actions.openTerminal,
    showToast: jest.fn(),
  };
}

async function mount(
  rec: SessionRecord,
  opts: Options & {
    openTerminal?: jest.Mock;
    disconnect?: jest.Mock;
  } = {},
) {
  const openTerminal = opts.openTerminal ?? jest.fn();
  const disconnect = opts.disconnect ?? jest.fn();
  const core = opts.core ?? createFakeCore({ sessions: [liveSession] });
  if (opts.core === undefined) {
    core.failNext('network.status', new RsCoreError('unsupported', 'test'));
  }
  const rendered = await renderWithProviders(
    <ReconnectOverlay ctx={context(rec, { openTerminal, disconnect })} />,
    { ...opts, core },
  );
  return { ...rendered, core, openTerminal, disconnect };
}

let renderedIds: Array<{ queryClient: { clear(): void } }> = [];

async function advance(ms: number) {
  await act(async () => {
    jest.advanceTimersByTime(ms);
  });
}

beforeEach(() => {
  jest.spyOn(Date.prototype, 'getTimezoneOffset').mockReturnValue(-480);
  jest.useFakeTimers();
  jest.setSystemTime(BEIJING_102431_MS);
});

afterEach(async () => {
  for (const rendered of renderedIds.splice(0)) {
    rendered.queryClient.clear();
  }
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('shows the paused clock, reconnecting copy and the elapsed wait', async () => {
  const view = await mount(record());
  renderedIds.push(view);

  expect(
    view.getByLabelText('画面已暂停在 10:24:31', {
      includeHiddenElements: true,
    }),
  ).toBeOnTheScreen();
  expect(view.getByRole('header')).toHaveTextContent('正在重新连接');
  expect(
    view.getByText('连接断了一下。会话还在 Mac 上保持着。'),
  ).toBeOnTheScreen();
  expect(view.getByTestId('reconnect-dialog')).toHaveProp(
    'accessibilityViewIsModal',
    true,
  );

  await advance(2400);
  expect(view.getByText(/^已等 2 秒/)).toBeOnTheScreen();
});

it('routes the terminal action through the session context', async () => {
  // R13：只有这台电脑有 SSH 服务器时才给「改用终端」。
  const core = createFakeCore({
    sessions: [liveSession],
    sshServers: [{ ...sshServers[0]!, serverId: computers[0].node }],
  });
  core.failNext('network.status', new RsCoreError('unsupported', 'test'));
  const openTerminal = jest.fn();
  const disconnect = jest.fn();
  const view = await renderWithProviders(
    <ReconnectOverlay
      ctx={{
        ...context(record(), { openTerminal, disconnect }),
        host: computers[0],
      }}
    />,
    { core },
  );
  renderedIds.push(view);
  await advance(0);

  await fireEvent.press(view.getByLabelText('改用终端'));
  expect(openTerminal).toHaveBeenCalledTimes(1);
  expect(disconnect).not.toHaveBeenCalled();
});

it('hides the terminal action when the computer has no SSH server', async () => {
  const view = await mount(record());
  renderedIds.push(view);
  await advance(0);
  expect(view.queryByLabelText('改用终端')).toBeNull();
  expect(view.getByLabelText('断开')).toBeOnTheScreen();
});

it('collapses while reconnecting and fits a narrow window', async () => {
  jest
    .spyOn(Dimensions, 'get')
    .mockReturnValue({ width: 320, height: 600, scale: 2, fontScale: 1 });
  const collapse = jest.fn();
  const view = await renderWithProviders(
    <ReconnectOverlay
      ctx={{
        ...context(record(), {
          openTerminal: jest.fn(),
          disconnect: jest.fn(),
        }),
        collapse,
      }}
    />,
  );
  renderedIds.push(view);
  expect(
    StyleSheet.flatten(view.getByTestId('reconnect-dialog').props.style).width,
  ).toBe(288);
  await fireEvent.press(view.getByTestId('reconnect-collapse'));
  expect(collapse).toHaveBeenCalledTimes(1);
});

it('hides the terminal action while the host updates', async () => {
  const view = await mount(
    record({ reconnectReason: 'hostUpdating', phase: 'ended' }),
  );
  renderedIds.push(view);

  expect(view.getByRole('header')).toHaveTextContent('电脑正在更新');
  expect(view.queryByLabelText('改用终端')).toBeNull();
  expect(view.getByLabelText('断开')).toBeOnTheScreen();
});

it('uses the generic host wording on Windows', async () => {
  const openTerminal = jest.fn();
  const disconnect = jest.fn();
  const rec = record();
  const view = await renderWithProviders(
    <ReconnectOverlay
      ctx={{
        ...context(rec, { openTerminal, disconnect }),
        os: 'windows',
      }}
    />,
  );
  renderedIds.push(view);

  expect(view.queryByText(/Mac/)).toBeNull();
  expect(view.getByText(/还在电脑上保持着/)).toBeOnTheScreen();
});
