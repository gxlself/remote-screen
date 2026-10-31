import {
  act,
  cleanup,
  fireEvent,
  renderHook,
  screen,
  within,
} from '@testing-library/react-native';
import type { RefObject } from 'react';
import { BackHandler, PixelRatio, Platform } from 'react-native';
import type { QueryClient } from '@tanstack/react-query';
import type { RootStackParamList } from '../../app/navigation/routes';
import { adapters } from '../../i18n/zh/adapters';
import { coach } from '../../i18n/zh/coach';
import { ipadSession } from '../../i18n/zh/ipadSession';
import { sessionEnded } from '../../i18n/zh/sessionEnded';
import {
  resetKeyboardInset,
  setKeyboardInset,
} from '../../native/keyboard/keyboardInset';
import type { SessionShortcutEvent } from '../../native/keyboard/KeyCapture';
import type { SessionKeyboardProps } from '../../native/keyboard/SessionKeyboard';
import { FakePlatform } from '../../native/platform/fakePlatform';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import {
  FIXTURE_NOW,
  computers,
  directory,
  liveSession,
} from '../../native/rscore/fixtures';
import { ADAPTER_RS_E } from '../../native/rscore/types';
import type {
  AdapterErrorInfo,
  AdapterPhase,
  AdapterTarget,
  CoreEventOf,
  Directory,
} from '../../native/rscore/types';
import type {
  RemoteSessionHandle,
  RemoteSessionViewProps,
} from '../../native/session/RemoteSessionView';
import {
  clearPointerLock,
  setPointerLock,
} from '../../native/session/pointerLockStore';
import { mockWindowDimensions } from '../../test/dimensions';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { mockAnimateSpace } from '../../test/mocks/RemoteSessionViewMock';
import { renderWithProviders } from '../../test/render';
import { errorCodeLabel } from '../format/misc';
import { ADAPTER_LIVE_KEY, readHistory } from '../sessions/history';
import {
  markArrival,
  peekArrival,
  resetArrival,
} from '../session/spaces';
import {
  resetAdapterPages,
  useLiveSessionCount,
} from '../session/useLiveSessionCount';
import AdapterSessionScreen, {
  FIRST_FRAME_FALLBACK_MS,
  TAKEOVER_NOTE_MS,
} from './AdapterSessionScreen';
import {
  adapterLive,
  resetAdapterLive,
  showAdapterSession,
} from './liveStore';
import type { CollapsedAdapter } from './liveStore';
import { TOOLBAR_DOCK_KEY, TOOLBAR_HANDLE_HIDDEN_KEY } from './toolbarDock';
import { installFakeAdapters } from './testing/fakeAdapters';
import type { FakeAdapters } from './testing/fakeAdapters';

const mockVideoProps = jest.fn<void, [RemoteSessionViewProps]>();
const mockKeyboardProps = jest.fn<void, [SessionKeyboardProps]>();

// The host-ready banner needs the console state only a Hub reported (V37), so
// its cases hand that directory in; every other case reads the local one.
let mockDirectory: Directory | undefined;
jest.mock('../data/directoryQuery', () => {
  const actual = jest.requireActual<
    typeof import('../data/directoryQuery')
  >('../data/directoryQuery');
  return {
    ...actual,
    useDirectory: () => {
      const local = actual.useDirectory();
      return mockDirectory === undefined
        ? local
        : { ...local, directory: mockDirectory };
    },
  };
});

jest.mock('../../native/session/RemoteSessionView', () => {
  const React = jest.requireActual<typeof import('react')>('react');
  const shared = jest.requireActual<
    typeof import('../../test/mocks/RemoteSessionViewMock')
  >('../../test/mocks/RemoteSessionViewMock');
  function RemoteSessionView(props: RemoteSessionViewProps) {
    mockVideoProps(props);
    return React.createElement(shared.RemoteSessionView, props);
  }
  return { __esModule: true, default: RemoteSessionView, RemoteSessionView };
});

jest.mock('../../native/keyboard/SessionKeyboard', () => {
  const React = jest.requireActual<typeof import('react')>('react');
  const { View: MockView } =
    jest.requireActual<typeof import('react-native')>('react-native');
  function SessionKeyboard(props: SessionKeyboardProps) {
    mockKeyboardProps(props);
    return React.createElement(MockView, { testID: 'session-keyboard-mock' });
  }
  return { __esModule: true, default: SessionKeyboard, SessionKeyboard };
});

const ID = '4611686018427387905';
const SECOND_ID = '4611686018427387906';
const THIRD_ID = '4611686018427387907';
const FOURTH_ID = '4611686018427387908';
const DROP: AdapterErrorInfo = {
  code: 'timeout',
  rsCode: ADAPTER_RS_E.unreachable,
  message: 'link lost',
};
const vnc: AdapterTarget = {
  protocol: 'vnc',
  hostId: 'h1',
  addr: '100.64.0.7',
};
const rdp: AdapterTarget = {
  protocol: 'rdp',
  hostId: 'h1',
  addr: '100.64.0.9',
};
const defaults: RootStackParamList['AdapterSession'] = {
  target: vnc,
  name: 'Mac mini',
  addrKind: 'tailnet',
};
const clients: QueryClient[] = [];

type BackPressHandler = Parameters<typeof BackHandler.addEventListener>[1];

type MountOptions = {
  core?: FakeCore;
  platform?: FakePlatform;
  settings?: NonNullable<Parameters<typeof renderWithProviders>[1]>['settings'];
  setup?: (fake: FakeAdapters) => void;
  localDirectory?: NonNullable<
    Parameters<typeof renderWithProviders>[1]
  >['localDirectory'];
  kv?: NonNullable<Parameters<typeof renderWithProviders>[1]>['kv'];
};

async function advance(ms = 1) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

async function mount(
  params: Partial<RootStackParamList['AdapterSession']> = {},
  opts: MountOptions = {},
) {
  const core = opts.core ?? createFakeCore();
  const fake = installFakeAdapters(core);
  const route = mockRoute('AdapterSession', { ...defaults, ...params });
  const target = route.params.target;
  opts.setup?.(fake);
  const platform = opts.platform ?? new FakePlatform();
  const navigation = mockNavigation<'AdapterSession'>();
  const view = await renderWithProviders(
    <AdapterSessionScreen navigation={navigation} route={route} />,
    {
      core,
      platform,
      // The gesture coach has its own tests below.
      settings: { 'session.coachShown': true, ...opts.settings },
      localDirectory: opts.localDirectory,
      ...(opts.kv === undefined ? {} : { kv: opts.kv }),
    },
  );
  clients.push(view.queryClient);
  await advance();
  return { ...view, core, fake, platform, navigation, target };
}

async function emitState(
  core: FakeCore,
  phase: AdapterPhase,
  extra: Partial<CoreEventOf<'adapter.state'>> = {},
) {
  await act(async () => {
    core.emit({
      type: 'adapter.state',
      sessionId: ID,
      protocol: 'vnc',
      phase,
      credError: false,
      ...extra,
    });
  });
}

async function stream(core: FakeCore, id = ID) {
  await streamWithoutFrame(core, id);
  await firstFrame();
}

async function streamWithoutFrame(core: FakeCore, id = ID) {
  await emitState(core, 'streaming', {
    sessionId: id,
    width: 1920,
    height: 1080,
  });
}

async function firstFrame() {
  await act(async () => {
    lastVideoProps().onFirstFrame?.();
  });
}

async function endWith(core: FakeCore, error: AdapterErrorInfo, id = ID) {
  await emitState(core, 'ended', { sessionId: id, error });
}

function callsOf(fake: FakeAdapters, method: string) {
  return fake.calls.filter(call => call.method === method);
}

function lastVideoProps(): RemoteSessionViewProps {
  const props = mockVideoProps.mock.calls.at(-1)?.[0];
  if (props === undefined) {
    throw new Error('RemoteSessionView was not rendered');
  }
  return props;
}

type TreeNode = { type: unknown; parent: unknown };

/** Host element types from `element` up to the root (Modal shows as one). */
function hostTypesAbove(element: TreeNode): string[] {
  const types: string[] = [];
  let node: TreeNode | null = element;
  while (node !== null && node !== undefined) {
    types.push(String(node.type));
    node = node.parent as TreeNode | null;
  }
  return types;
}

function lastKeyboardProps(): SessionKeyboardProps {
  const props = mockKeyboardProps.mock.calls.at(-1)?.[0];
  if (props === undefined) {
    throw new Error('SessionKeyboard was not rendered');
  }
  return props;
}

function hostH1(loginWindow: boolean) {
  return {
    ...directory,
    computers: [
      {
        ...computers[0],
        hostId: 'h1',
        online: true,
        hostRunning: true,
        console: { loggedIn: !loginWindow, locked: false, loginWindow },
      },
    ],
  };
}

beforeEach(() => {
  jest.useFakeTimers();
  resetKeyboardInset();
  mockVideoProps.mockClear();
  mockKeyboardProps.mockClear();
});

afterEach(async () => {
  await cleanup();
  // A page unmounted on a live picture folds its session away (V33).
  resetAdapterLive();
  mockDirectory = undefined;
  for (const client of clients.splice(0)) {
    client.clear();
  }
  resetKeyboardInset();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('connecting shows the waiting layer and opens with the view size', async () => {
  // Restored in afterEach: swapping the hook mid-test changes its hook order.
  mockWindowDimensions({ width: 844, height: 390 });
  jest.spyOn(PixelRatio, 'get').mockReturnValue(3);
  const { core, fake } = await mount();

  expect(screen.getByText(adapters.connecting('Mac mini'))).toBeOnTheScreen();
  expect(screen.queryByText(adapters.unencrypted)).toBeNull();
  expect(screen.queryByText(adapters.rdpTakeover)).toBeNull();
  expect(screen.getByTestId('adapter-wait-disconnect')).toBeOnTheScreen();
  expect(callsOf(fake, 'open')).toEqual([
    {
      method: 'open',
      args: [
        vnc,
        { viewWidth: 844, viewHeight: 390, scale: 3, useSaved: true },
      ],
    },
  ]);

  await emitState(core, 'authenticating');
  expect(screen.getByText(adapters.authenticating)).toBeOnTheScreen();
  expect(screen.queryByText(adapters.connecting('Mac mini'))).toBeNull();
});

it('passes the entry context to open and falls back to the address as name', async () => {
  const { fake } = await mount({ name: undefined, context: 'loginWindow' });

  expect(screen.getByText(adapters.connecting('100.64.0.7'))).toBeOnTheScreen();
  expect(callsOf(fake, 'open')[0]?.args[1]).toMatchObject({
    useSaved: true,
    context: 'loginWindow',
  });
});

it('unencrypted_notice warns for VNC off the tailnet and RDP takeover', async () => {
  await mount({ addrKind: 'lan' }, { settings: { 'tailnet.enabled': true } });
  expect(screen.getByText(adapters.unencrypted)).toBeOnTheScreen();
  await cleanup();

  // With Tailscale switched off the warning does not name it.
  await mount({ addrKind: 'lan' });
  expect(screen.getByText(adapters.unencryptedLan)).toBeOnTheScreen();
  expect(screen.queryByText(/Tailscale/)).toBeNull();
  await cleanup();

  await mount({ target: rdp });
  expect(screen.getByText(adapters.rdpTakeover)).toBeOnTheScreen();
  expect(screen.queryByText(adapters.unencrypted)).toBeNull();
});

it('needs_credentials shows the sheet without remember and submits', async () => {
  const { core, fake } = await mount({}, { setup: f => f.setCanSave(false) });
  await emitState(core, 'needsCredentials', {
    credFields: ['user', 'password'],
  });

  expect(screen.getByTestId('adapter-creds')).toBeOnTheScreen();
  expect(screen.getByText(adapters.connecting('Mac mini'))).toBeOnTheScreen();
  expect(screen.queryByTestId('adapter-creds-remember')).toBeNull();

  await fireEvent.changeText(screen.getByTestId('adapter-creds-user'), 'alice');
  await fireEvent.changeText(
    screen.getByTestId('adapter-creds-password'),
    'pw',
  );
  await fireEvent.press(screen.getByTestId('adapter-creds-submit'));

  expect(callsOf(fake, 'submitCredentials')).toEqual([
    {
      method: 'submitCredentials',
      args: [ID, { user: 'alice', password: 'pw', save: false }],
    },
  ]);
});

it('needs_credentials prefills the saved user and cancels', async () => {
  const { core, fake } = await mount(
    {},
    { setup: f => f.setSaved(vnc, { saved: true, user: 'alice' }) },
  );
  await emitState(core, 'needsCredentials', {
    credFields: ['user', 'password'],
    credError: true,
  });

  expect(screen.getByTestId('adapter-creds-user')).toHaveProp('value', 'alice');
  expect(screen.getByTestId('adapter-creds-failed')).toBeOnTheScreen();
  expect(screen.getByTestId('adapter-creds-remember')).toBeOnTheScreen();
  await fireEvent.press(screen.getByTestId('adapter-creds-cancel'));
  expect(callsOf(fake, 'cancelCredentials')).toEqual([
    { method: 'cancelCredentials', args: [ID] },
  ]);
});

it('streaming_view shows the adapter picture, VNC toolbar and keyboard', async () => {
  const { core } = await mount();
  await stream(core);

  expect(
    screen.getByTestId('remote-session-view-placeholder'),
  ).toBeOnTheScreen();
  expect(screen.queryByTestId('adapter-waiting')).toBeNull();
  expect(lastVideoProps()).toMatchObject({
    sessionId: ID,
    mode: 'trackpad',
    toolbarCollapsed: false,
    keyboardInset: 0,
    accessibilityLabel: adapters.a11y.view('Mac mini'),
  });
  for (const id of ['keyboard', 'sendClipboard', 'mode', 'disconnect']) {
    expect(screen.getByTestId(`adapter-tb-${id}`)).toBeOnTheScreen();
  }
  expect(screen.queryByTestId('adapter-tb-ctrlAltDel')).toBeNull();
  expect(screen.queryByTestId('adapter-tb-typePassword')).toBeNull();
  expect(screen.queryByTestId('adapter-tb-forget')).toBeNull();
  expect(lastKeyboardProps()).toMatchObject({
    open: false,
    sessionId: ID,
    os: 'macos',
  });

  await fireEvent.press(screen.getByTestId('adapter-tb-mode'));
  expect(lastVideoProps().mode).toBe('touch');

  await fireEvent.press(screen.getByTestId('adapter-tb-keyboard'));
  expect(lastKeyboardProps().open).toBe(true);
  await act(async () => setKeyboardInset(240));
  expect(lastVideoProps().keyboardInset).toBe(240);

  await act(async () => lastKeyboardProps().onClose());
  expect(lastKeyboardProps().open).toBe(false);
  expect(lastVideoProps().keyboardInset).toBe(0);
});

it('lifts the toolbar above the keyboard bar while the keyboard is open', async () => {
  const { core } = await mount();
  await stream(core);
  const toolbar = () => screen.getByTestId('adapter-toolbar');
  expect(toolbar()).toHaveStyle({ bottom: 34 });

  await fireEvent.press(screen.getByTestId('adapter-tb-keyboard'));
  expect(lastKeyboardProps().open).toBe(true);
  await act(async () => setKeyboardInset(400));
  expect(toolbar()).toHaveStyle({ bottom: 412 });
  // V23: the toolbar folds into its handle above the keyboard bar.
  expect(toolbar()).toContainElement(screen.getByTestId('adapter-tb-expand'));
  expect(screen.queryByTestId('adapter-tb-keyboard')).toBeNull();
  expect(lastVideoProps().toolbarCollapsed).toBe(true);

  await act(async () => lastKeyboardProps().onClose());
  expect(lastKeyboardProps().open).toBe(false);
  expect(toolbar()).toHaveStyle({ bottom: 34 });
  expect(toolbar()).toContainElement(screen.getByTestId('adapter-tb-keyboard'));
});

it('rdp_toolbar has Ctrl+Alt+Del and clipboard but no password typing', async () => {
  const { core, fake } = await mount(
    { target: rdp },
    { setup: f => f.setSaved(rdp, { saved: true, user: 'alice' }) },
  );
  await stream(core);

  expect(screen.getByTestId('adapter-tb-sendClipboard')).toBeOnTheScreen();
  expect(screen.queryByTestId('adapter-tb-typePassword')).toBeNull();
  expect(lastKeyboardProps().os).toBe('windows');

  // V11: Ctrl+Alt+Del and forget live in 更多.
  await fireEvent.press(screen.getByTestId('adapter-tb-more'));
  expect(screen.getByTestId('adapter-more-ctrlAltDel')).toBeOnTheScreen();
  expect(screen.getByTestId('adapter-more-forget')).toBeOnTheScreen();
  await fireEvent.press(screen.getByTestId('adapter-more-ctrlAltDel'));
  expect(callsOf(fake, 'sendCtrlAltDel')).toEqual([
    { method: 'sendCtrlAltDel', args: [ID] },
  ]);
  expect(screen.queryByTestId('adapter-more')).toBeNull();
  expect(screen.getByText(adapters.toast.ctrlAltDelSent)).toBeOnTheScreen();
});

it('type_password types the saved password and confirms', async () => {
  const { core, fake } = await mount(
    {},
    { setup: f => f.setSaved(vnc, { saved: true, user: 'alice' }) },
  );
  await stream(core);

  await fireEvent.press(screen.getByTestId('adapter-tb-typePassword'));
  expect(callsOf(fake, 'typePassword')).toEqual([
    { method: 'typePassword', args: [ID] },
  ]);
  expect(screen.getByText(adapters.toast.passwordTyped)).toBeOnTheScreen();
});

it('type_password is hidden without a saved password', async () => {
  const { core } = await mount();
  await stream(core);

  expect(screen.queryByTestId('adapter-tb-typePassword')).toBeNull();
});

it('send_clipboard reports sent, empty and unsupported text', async () => {
  const platform = new FakePlatform();
  const { core } = await mount({}, { platform });
  await stream(core);

  platform.clipboardText = 'hi';
  await fireEvent.press(screen.getByTestId('adapter-tb-sendClipboard'));
  await advance();
  expect(core.calls).toContainEqual({
    method: 'sessions.clipboardSend',
    args: [ID, 'hi'],
  });
  expect(screen.getByText(adapters.toast.clipboardSent)).toBeOnTheScreen();

  platform.clipboardText = null;
  await fireEvent.press(screen.getByTestId('adapter-tb-sendClipboard'));
  await advance();
  expect(screen.getByText(adapters.toast.clipboardEmpty)).toBeOnTheScreen();

  platform.clipboardText = '你好';
  await fireEvent.press(screen.getByTestId('adapter-tb-sendClipboard'));
  await advance();
  expect(
    screen.getByText(adapters.toast.clipboardLatin1Only),
  ).toBeOnTheScreen();
  expect(
    core.calls.filter(call => call.method === 'sessions.clipboardSend'),
  ).toHaveLength(1);
});

it('send_clipboard logs a failed send and says so', async () => {
  const platform = new FakePlatform();
  const { core } = await mount({}, { platform });
  await stream(core);

  platform.clipboardText = 'hi';
  core.failNext('sessions.clipboardSend', new Error('gone'));
  await fireEvent.press(screen.getByTestId('adapter-tb-sendClipboard'));
  await advance();
  expect(core.calls).toContainEqual({
    method: 'logs.write',
    args: ['warn', 'adapters', 'clipboard send failed'],
  });
  expect(screen.getByText(adapters.toast.clipboardFailed)).toBeOnTheScreen();
});

it('rdp_clipboard_unicode sends Chinese text over RDP', async () => {
  const platform = new FakePlatform();
  const { core } = await mount({ target: rdp }, { platform });
  await stream(core);

  platform.clipboardText = '你好';
  await fireEvent.press(screen.getByTestId('adapter-tb-sendClipboard'));
  await advance();
  expect(core.calls).toContainEqual({
    method: 'sessions.clipboardSend',
    args: [ID, '你好'],
  });
  expect(screen.getByText(adapters.toast.clipboardSent)).toBeOnTheScreen();
});

it('remote_clipboard copies remote text only when auto sync is on', async () => {
  const platform = new FakePlatform();
  const { core } = await mount(
    {},
    { platform, settings: { 'clipboard.autoSync': true } },
  );
  await stream(core);
  await act(async () => {
    core.emit({ type: 'adapter.clipboard', sessionId: ID, text: 'x' });
  });
  await advance();
  expect(platform.copied).toEqual(['x']);
  expect(screen.getByText(adapters.toast.clipboardReceived)).toBeOnTheScreen();
  await cleanup();

  const quiet = new FakePlatform();
  const second = await mount(
    {},
    { platform: quiet, settings: { 'clipboard.autoSync': false } },
  );
  await stream(second.core);
  await act(async () => {
    second.core.emit({ type: 'adapter.clipboard', sessionId: ID, text: 'x' });
  });
  await advance();
  expect(quiet.copied).toEqual([]);
  expect(screen.queryByText(adapters.toast.clipboardReceived)).toBeNull();
});

it('cert_prompt trusts the new certificate and closes the dialog', async () => {
  const { core, fake } = await mount({ target: rdp });
  await act(async () => {
    core.emit({
      type: 'adapter.certPrompt',
      sessionId: ID,
      fp: 'aa:bb',
      oldFp: 'cc:dd',
    });
  });

  expect(screen.getByTestId('adapter-cert')).toBeOnTheScreen();
  expect(
    within(screen.getByTestId('adapter-cert-old')).getByText('cc:dd'),
  ).toBeOnTheScreen();
  await fireEvent.press(screen.getByText(adapters.cert.trust));
  expect(callsOf(fake, 'trustCert')).toEqual([
    { method: 'trustCert', args: [ID, true] },
  ]);
  expect(screen.queryByTestId('adapter-cert')).toBeNull();
});

it('ended_clean goes back exactly once', async () => {
  const { core, navigation } = await mount();
  await stream(core);
  await emitState(core, 'ended');
  await emitState(core, 'ended');

  expect(navigation.goBack).toHaveBeenCalledTimes(1);
  expect(screen.queryByTestId('adapter-error')).toBeNull();
});

it('ended_cancelled goes back without an error page', async () => {
  const { core, navigation } = await mount();
  await endWith(core, { code: 'cancelled', rsCode: 0, message: 'cancelled' });

  expect(navigation.goBack).toHaveBeenCalledTimes(1);
  expect(screen.queryByTestId('adapter-error')).toBeNull();
});

it('ended_error_unreachable retries and opens another app', async () => {
  const platform = new FakePlatform();
  const { core, fake, navigation } = await mount({}, { platform });
  await endWith(core, {
    code: 'timeout',
    rsCode: ADAPTER_RS_E.unreachable,
    message: 'timed out',
  });

  const page = within(screen.getByTestId('adapter-error'));
  expect(
    page.getByText(adapters.errors.title('timeout', 'Mac mini')),
  ).toBeOnTheScreen();
  expect(
    page.getByText(errorCodeLabel(ADAPTER_RS_E.unreachable)),
  ).toBeOnTheScreen();
  expect(page.getByText(adapters.title('vnc'))).toBeOnTheScreen();
  expect(page.getByText(adapters.errors.back)).toBeOnTheScreen();

  await fireEvent.press(page.getByText(adapters.errors.retry));
  await advance();
  expect(fake.calls.map(call => call.method)).toEqual([
    'available',
    'canSave',
    'open',
    'open',
  ]);
  expect(screen.queryByTestId('adapter-error')).toBeNull();
  expect(screen.getByText(adapters.connecting('Mac mini'))).toBeOnTheScreen();

  await endWith(
    core,
    { code: 'timeout', rsCode: ADAPTER_RS_E.unreachable, message: 'again' },
    SECOND_ID,
  );
  await fireEvent.press(screen.getByText(adapters.errors.otherApp));
  await advance();
  expect(platform.opened).toEqual(['vnc://100.64.0.7']);
  expect(navigation.navigate).not.toHaveBeenCalled();
  expect(callsOf(fake, 'open')).toHaveLength(2);
});

it('copies the error code from the error page', async () => {
  const platform = new FakePlatform();
  const { core } = await mount({}, { platform });
  await endWith(core, {
    code: 'serverClosed',
    rsCode: ADAPTER_RS_E.broken,
    message: 'closed',
  });

  await fireEvent.press(
    screen.getByLabelText(
      sessionEnded.copyCode(errorCodeLabel(ADAPTER_RS_E.broken)),
    ),
  );
  await advance();
  expect(platform.copied).toEqual([errorCodeLabel(ADAPTER_RS_E.broken)]);
  expect(screen.getByText(adapters.toast.codeCopied)).toBeOnTheScreen();
});

it('ended_error_service_off uses the refused title and RS-E205', async () => {
  const { core } = await mount();
  await endWith(core, {
    code: 'refused',
    rsCode: ADAPTER_RS_E.serviceOff,
    message: 'refused',
  });

  const page = within(screen.getByTestId('adapter-error'));
  expect(
    page.getByText(adapters.errors.title('refused', 'Mac mini')),
  ).toBeOnTheScreen();
  expect(
    page.getByText(errorCodeLabel(ADAPTER_RS_E.serviceOff)),
  ).toBeOnTheScreen();
  expect(page.getByText(adapters.errors.retry)).toBeOnTheScreen();
  expect(page.getByText(adapters.errors.otherApp)).toBeOnTheScreen();
});

it('ended_error_unsupported offers only another app and back', async () => {
  const { core } = await mount();
  await endWith(core, {
    code: 'unsupportedSecurity',
    rsCode: ADAPTER_RS_E.unsupported,
    message: 'security type 33',
  });

  const page = within(screen.getByTestId('adapter-error'));
  expect(page.getByText(adapters.errors.otherApp)).toBeOnTheScreen();
  expect(page.queryByText(adapters.errors.retry)).toBeNull();
  const tree = JSON.stringify(screen.toJSON());
  expect(tree.indexOf(adapters.errors.otherApp)).toBeLessThan(
    tree.indexOf(adapters.errors.back),
  );
});

it('ended_error_auth_failed offers retry and back only', async () => {
  const { core } = await mount();
  await endWith(core, {
    code: 'authFailed',
    rsCode: ADAPTER_RS_E.authFailed,
    message: 'three failures',
  });

  const page = within(screen.getByTestId('adapter-error'));
  expect(page.getByText(adapters.errors.retry)).toBeOnTheScreen();
  expect(page.getByText(adapters.errors.back)).toBeOnTheScreen();
  expect(page.queryByText(adapters.errors.otherApp)).toBeNull();
});

it('retries a failed open by opening again', async () => {
  const core = createFakeCore();
  const { fake } = await mount(
    {},
    {
      core,
      setup: f => f.failNext('open', new Error('bindings')),
    },
  );

  expect(screen.getByTestId('adapter-error')).toBeOnTheScreen();
  await fireEvent.press(screen.getByText(adapters.errors.retry));
  await advance();
  expect(callsOf(fake, 'open')).toHaveLength(2);
  expect(screen.getByText(adapters.connecting('Mac mini'))).toBeOnTheScreen();
});

it('host_ready_banner switches to Remote Host from the login window', async () => {
  const core = createFakeCore();
  mockDirectory = hostH1(false);
  const { fake, navigation } = await mount(
    { context: 'loginWindow' },
    { core },
  );
  await stream(core);
  await advance();

  expect(
    screen.getByText(adapters.hostReady.message('Mac mini')),
  ).toBeOnTheScreen();
  await fireEvent.press(screen.getByText(adapters.hostReady.action));
  expect(callsOf(fake, 'close')).toEqual([{ method: 'close', args: [ID] }]);
  expect(navigation.replace).toHaveBeenCalledWith('Session', { hostId: 'h1' });
});

it('host_ready_banner stays hidden for manual entries and the login window', async () => {
  const core = createFakeCore();
  mockDirectory = hostH1(false);
  await mount({ context: 'manual' }, { core });
  await stream(core);
  await advance();
  expect(screen.queryByText(adapters.hostReady.message('Mac mini'))).toBeNull();
  await cleanup();

  const atLogin = createFakeCore();
  mockDirectory = hostH1(true);
  await mount({ context: 'loginWindow' }, { core: atLogin });
  await stream(atLogin);
  await advance();
  expect(screen.queryByText(adapters.hostReady.message('Mac mini'))).toBeNull();
});

it('disconnect_and_back closes the session after confirming', async () => {
  const { core, fake, navigation } = await mount();
  await stream(core);

  // V10: a live session asks first.
  await fireEvent.press(screen.getByTestId('adapter-tb-disconnect'));
  expect(
    screen.getByText(adapters.disconnect.title('Mac mini')),
  ).toBeOnTheScreen();
  expect(callsOf(fake, 'close')).toEqual([]);
  await fireEvent.press(screen.getByTestId('adapter-disconnect-confirm'));
  expect(callsOf(fake, 'close')).toEqual([{ method: 'close', args: [ID] }]);
  expect(navigation.goBack).toHaveBeenCalledTimes(1);
});

it('keeps the session when the disconnect question is cancelled', async () => {
  const { core, fake, navigation } = await mount();
  await stream(core);

  await fireEvent.press(screen.getByTestId('adapter-tb-disconnect'));
  await fireEvent.press(screen.getByTestId('adapter-disconnect-cancel'));
  expect(screen.queryByText(adapters.disconnect.title('Mac mini'))).toBeNull();
  expect(callsOf(fake, 'close')).toEqual([]);
  expect(navigation.goBack).not.toHaveBeenCalled();
});

it('disconnect_and_back handles the Android back button', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const listeners: BackPressHandler[] = [];
  jest
    .spyOn(BackHandler, 'addEventListener')
    .mockImplementation((eventName, listener) => {
      if (eventName === 'hardwareBackPress') {
        listeners.push(listener);
      }
      return { remove: jest.fn() };
    });
  const { core, fake, navigation } = await mount();
  await stream(core);
  const pressBack = async () => {
    let handled: ReturnType<BackPressHandler> = false;
    await act(async () => {
      for (const listener of [...listeners].reverse()) {
        handled = listener({ type: 'hardwareBackPress', timeStamp: 0 });
        if (handled === true) {
          break;
        }
      }
    });
    return handled;
  };

  // V02: keyboard first, then the toolbar, then (V33) fold the session away.
  await fireEvent.press(screen.getByTestId('adapter-tb-keyboard'));
  expect(lastKeyboardProps().open).toBe(true);
  expect(await pressBack()).toBe(true);
  expect(lastKeyboardProps().open).toBe(false);
  expect(screen.getByTestId('adapter-tb-collapse')).toBeOnTheScreen();

  expect(await pressBack()).toBe(true);
  expect(screen.getByTestId('adapter-tb-expand')).toBeOnTheScreen();
  expect(screen.queryByText(adapters.disconnect.title('Mac mini'))).toBeNull();

  expect(await pressBack()).toBe(true);
  expect(screen.queryByText(adapters.disconnect.title('Mac mini'))).toBeNull();
  expect(navigation.popTo).toHaveBeenCalledWith('MainTabs', {
    screen: 'Home',
  });
  expect(navigation.goBack).not.toHaveBeenCalled();
  await cleanup();
  expect(callsOf(fake, 'close')).toEqual([]);
  expect(adapterLive.peek(ID)?.phase).toBe('streaming');
});

it('the Android back button cancels a connection that is still opening', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const listeners: BackPressHandler[] = [];
  jest
    .spyOn(BackHandler, 'addEventListener')
    .mockImplementation((eventName, listener) => {
      if (eventName === 'hardwareBackPress') {
        listeners.push(listener);
      }
      return { remove: jest.fn() };
    });
  const { fake, navigation } = await mount();

  await act(async () => {
    listeners.at(-1)?.({ type: 'hardwareBackPress', timeStamp: 0 });
  });
  expect(callsOf(fake, 'close')).toEqual([{ method: 'close', args: [ID] }]);
  expect(navigation.goBack).toHaveBeenCalledTimes(1);
});

it('disconnects from the waiting layer', async () => {
  const { fake, navigation } = await mount();

  // V13: while connecting the button is a plain 取消, with no question.
  expect(
    within(screen.getByTestId('adapter-wait-disconnect')).getByText(
      adapters.cancel,
    ),
  ).toBeOnTheScreen();
  await fireEvent.press(screen.getByTestId('adapter-wait-disconnect'));
  expect(callsOf(fake, 'close')).toEqual([{ method: 'close', args: [ID] }]);
  expect(navigation.goBack).toHaveBeenCalledTimes(1);
});

it('adapters_unavailable shows the error page without opening', async () => {
  const { fake } = await mount({}, { setup: f => f.setAvailable(false) });

  // V45: say the build lacks it and offer another app instead of retry.
  const page = within(screen.getByTestId('adapter-error'));
  expect(page.getByText(adapters.errors.unavailableTitle)).toBeOnTheScreen();
  expect(page.getByText(adapters.errors.otherApp)).toBeOnTheScreen();
  expect(page.queryByText(adapters.errors.retry)).toBeNull();
  expect(page.getByText(errorCodeLabel(ADAPTER_RS_E.broken))).toBeOnTheScreen();
  expect(callsOf(fake, 'open')).toEqual([]);
});

it('forget_saved removes the saved password buttons', async () => {
  const { core, fake, target } = await mount(
    {},
    { setup: f => f.setSaved(vnc, { saved: true, user: 'alice' }) },
  );
  await stream(core);

  expect(screen.getByTestId('adapter-tb-typePassword')).toBeOnTheScreen();
  await fireEvent.press(screen.getByTestId('adapter-tb-more'));
  await fireEvent.press(screen.getByTestId('adapter-more-forget'));
  // V11: forgetting asks first.
  expect(screen.getByText(adapters.forget.title)).toBeOnTheScreen();
  expect(callsOf(fake, 'forget')).toEqual([]);
  await fireEvent.press(screen.getByTestId('adapter-forget-confirm'));
  expect(callsOf(fake, 'forget')).toEqual([
    { method: 'forget', args: [target] },
  ]);
  expect(screen.getByText(adapters.toast.forgotten)).toBeOnTheScreen();
  expect(screen.queryByTestId('adapter-tb-typePassword')).toBeNull();
  await fireEvent.press(screen.getByTestId('adapter-tb-more'));
  expect(screen.queryByTestId('adapter-more-forget')).toBeNull();
});

it('forget_saved does nothing when the question is cancelled', async () => {
  const { core, fake } = await mount(
    {},
    { setup: f => f.setSaved(vnc, { saved: true, user: 'alice' }) },
  );
  await stream(core);

  await fireEvent.press(screen.getByTestId('adapter-tb-more'));
  await fireEvent.press(screen.getByTestId('adapter-more-forget'));
  await fireEvent.press(screen.getByTestId('adapter-forget-cancel'));
  expect(callsOf(fake, 'forget')).toEqual([]);
  expect(screen.getByTestId('adapter-tb-typePassword')).toBeOnTheScreen();
});

it('forget_saved keeps the buttons when forgetting fails', async () => {
  const { core, fake } = await mount(
    {},
    {
      setup: f => {
        f.setSaved(vnc, { saved: true, user: 'alice' });
        f.failNext('forget', new Error('keychain'));
      },
    },
  );
  await stream(core);

  await fireEvent.press(screen.getByTestId('adapter-tb-more'));
  await fireEvent.press(screen.getByTestId('adapter-more-forget'));
  await fireEvent.press(screen.getByTestId('adapter-forget-confirm'));
  expect(callsOf(fake, 'forget')).toHaveLength(1);
  expect(screen.getByTestId('adapter-tb-typePassword')).toBeOnTheScreen();
  expect(screen.getByText(adapters.toast.forgetFailed)).toBeOnTheScreen();
  await fireEvent.press(screen.getByTestId('adapter-tb-more'));
  expect(screen.getByTestId('adapter-more-forget')).toBeOnTheScreen();
  expect(core.calls).toContainEqual({
    method: 'logs.write',
    args: ['warn', 'adapters', 'forget failed'],
  });
});

it('resize_debounced sends only the last layout after 500 ms', async () => {
  jest.spyOn(PixelRatio, 'get').mockReturnValue(3);
  const { core, fake } = await mount({ target: rdp });
  await stream(core);
  const body = screen.getByTestId('adapter-session-body');
  const layout = async (width: number, height: number) => {
    await fireEvent(body, 'layout', {
      nativeEvent: { layout: { x: 0, y: 0, width, height } },
    });
  };

  await layout(844, 390);
  await advance(600);
  expect(callsOf(fake, 'resize')).toEqual([]);

  await layout(700, 390);
  await advance(100);
  await layout(600, 390);
  await advance(100);
  await layout(390, 844);
  await advance(499);
  expect(callsOf(fake, 'resize')).toEqual([]);
  await advance(1);
  expect(callsOf(fake, 'resize')).toEqual([
    { method: 'resize', args: [ID, 390, 844, 3] },
  ]);
});

describe('auto reconnect (V01)', () => {
  it('retries a dropped session with backoff, then shows the error page', async () => {
    const { core, fake } = await mount();
    await stream(core);
    await endWith(core, DROP);

    expect(screen.getByTestId('adapter-reconnecting')).toBeOnTheScreen();
    expect(
      screen.getByText(adapters.reconnect.attempt(1, 3)),
    ).toBeOnTheScreen();
    expect(screen.queryByTestId('adapter-error')).toBeNull();
    // The last picture stays under the overlay.
    expect(lastVideoProps().sessionId).toBe(ID);
    await advance(998);
    expect(callsOf(fake, 'open')).toHaveLength(1);
    await advance(2);
    expect(callsOf(fake, 'open')).toHaveLength(2);
    expect(screen.getByTestId('adapter-reconnecting')).toBeOnTheScreen();
    expect(screen.queryByTestId('adapter-waiting')).toBeNull();

    await endWith(core, DROP, SECOND_ID);
    expect(
      screen.getByText(adapters.reconnect.attempt(2, 3)),
    ).toBeOnTheScreen();
    await advance(1999);
    expect(callsOf(fake, 'open')).toHaveLength(2);
    await advance(1);
    expect(callsOf(fake, 'open')).toHaveLength(3);

    await endWith(core, DROP, THIRD_ID);
    expect(
      screen.getByText(adapters.reconnect.attempt(3, 3)),
    ).toBeOnTheScreen();
    await advance(4000);
    expect(callsOf(fake, 'open')).toHaveLength(4);

    await endWith(core, DROP, FOURTH_ID);
    expect(screen.queryByTestId('adapter-reconnecting')).toBeNull();
    expect(
      within(screen.getByTestId('adapter-error')).getByText(
        adapters.errors.title('timeout', 'Mac mini'),
      ),
    ).toBeOnTheScreen();
    await advance(10000);
    expect(callsOf(fake, 'open')).toHaveLength(4);
  });

  it('goes back to the picture when a retry connects', async () => {
    const { core, fake } = await mount();
    await stream(core);
    await endWith(core, { ...DROP, code: 'io', rsCode: ADAPTER_RS_E.broken });
    await advance(1000);
    expect(callsOf(fake, 'open')).toHaveLength(2);

    await stream(core, SECOND_ID);
    expect(screen.queryByTestId('adapter-reconnecting')).toBeNull();
    expect(screen.getByTestId('adapter-tb-disconnect')).toBeOnTheScreen();
    expect(lastVideoProps().sessionId).toBe(SECOND_ID);

    // A later drop starts counting from one again.
    await endWith(core, DROP, SECOND_ID);
    expect(
      screen.getByText(adapters.reconnect.attempt(1, 3)),
    ).toBeOnTheScreen();
  });

  it.each([
    ['authFailed', ADAPTER_RS_E.authFailed],
    ['certRejected', ADAPTER_RS_E.certRejected],
    ['serverClosed', ADAPTER_RS_E.broken],
  ] as const)('does not retry %s after streaming', async (code, rsCode) => {
    const { core, fake } = await mount();
    await stream(core);
    await endWith(core, { code, rsCode, message: code });

    expect(screen.queryByTestId('adapter-reconnecting')).toBeNull();
    expect(screen.getByTestId('adapter-error')).toBeOnTheScreen();
    await advance(10000);
    expect(callsOf(fake, 'open')).toHaveLength(1);
  });

  it('does not retry after the user disconnects', async () => {
    const { core, fake, navigation } = await mount();
    await stream(core);
    await fireEvent.press(screen.getByTestId('adapter-tb-disconnect'));
    await fireEvent.press(screen.getByTestId('adapter-disconnect-confirm'));
    await endWith(core, DROP);

    await advance(10000);
    expect(callsOf(fake, 'open')).toHaveLength(1);
    expect(navigation.goBack).toHaveBeenCalledTimes(1);
  });

  it('cancel stops at once and shows what went wrong', async () => {
    const { core, fake } = await mount();
    await stream(core);
    await endWith(core, DROP);

    await fireEvent.press(screen.getByTestId('adapter-reconnect-cancel'));
    expect(screen.queryByTestId('adapter-reconnecting')).toBeNull();
    expect(
      within(screen.getByTestId('adapter-error')).getByText(
        adapters.errors.title('timeout', 'Mac mini'),
      ),
    ).toBeOnTheScreen();
    await advance(10000);
    expect(callsOf(fake, 'open')).toHaveLength(1);
  });

  it('cancel during a retry closes the new attempt', async () => {
    const { core, fake } = await mount();
    await stream(core);
    await endWith(core, DROP);
    await advance(1000);
    await emitState(core, 'connecting', { sessionId: SECOND_ID });

    await fireEvent.press(screen.getByTestId('adapter-reconnect-cancel'));
    expect(callsOf(fake, 'close')).toEqual([
      { method: 'close', args: [SECOND_ID] },
    ]);
    expect(screen.getByTestId('adapter-error')).toBeOnTheScreen();
    await advance(10000);
    expect(callsOf(fake, 'open')).toHaveLength(2);
  });

  it('a drop before the first picture goes straight to the error page', async () => {
    const { core, fake } = await mount();
    await endWith(core, DROP);

    expect(screen.queryByTestId('adapter-reconnecting')).toBeNull();
    expect(screen.getByTestId('adapter-error')).toBeOnTheScreen();
    await advance(10000);
    expect(callsOf(fake, 'open')).toHaveLength(1);
  });
});

describe('waiting and first frame', () => {
  it('keeps the placeholder until the first frame arrives (V12)', async () => {
    const { core } = await mount();
    await streamWithoutFrame(core);

    expect(screen.getByTestId('adapter-waiting')).toBeOnTheScreen();
    expect(screen.getByText(adapters.loadingPicture)).toBeOnTheScreen();
    expect(screen.queryByTestId('adapter-toolbar')).toBeNull();
    expect(lastVideoProps().sessionId).toBe(ID);

    await firstFrame();
    expect(screen.queryByTestId('adapter-waiting')).toBeNull();
    expect(screen.getByTestId('adapter-toolbar')).toBeOnTheScreen();
  });

  it('stops waiting for a first frame that never comes', async () => {
    const { core } = await mount();
    await streamWithoutFrame(core);

    await advance(FIRST_FRAME_FALLBACK_MS);
    expect(screen.queryByTestId('adapter-waiting')).toBeNull();
    expect(core.calls).toContainEqual({
      method: 'logs.write',
      args: ['warn', 'adapters', 'first frame did not arrive'],
    });
  });

  it('shows how long it has waited (V13)', async () => {
    await mount();
    expect(screen.queryByTestId('adapter-waited')).toBeNull();

    await advance(3000);
    expect(screen.getByTestId('adapter-waited')).toHaveTextContent(
      adapters.waited(3),
    );
    await advance(62000);
    expect(screen.getByTestId('adapter-waited')).toHaveTextContent(
      adapters.waited(65),
    );
  });
});

describe('toolbar and gestures', () => {
  it('three fingers fold the toolbar, two-finger double tap switches mode (V03)', async () => {
    const { core } = await mount();
    await stream(core);

    await act(async () =>
      lastVideoProps().onGesture?.({ kind: 'threeFingerTap' }),
    );
    expect(screen.getByTestId('adapter-tb-expand')).toBeOnTheScreen();
    expect(lastVideoProps().toolbarCollapsed).toBe(true);
    await act(async () =>
      lastVideoProps().onGesture?.({ kind: 'threeFingerTap' }),
    );
    expect(screen.getByTestId('adapter-tb-collapse')).toBeOnTheScreen();

    await act(async () =>
      lastVideoProps().onGesture?.({ kind: 'twoFingerDoubleTap' }),
    );
    expect(lastVideoProps().mode).toBe('touch');
    expect(screen.getByTestId('adapter-tb-mode')).toHaveProp(
      'accessibilityLabel',
      adapters.toolbar.mode('touch'),
    );
  });

  it('folds and unfolds from the grip and the handle (V04)', async () => {
    const { core } = await mount();
    await stream(core);

    await fireEvent.press(screen.getByTestId('adapter-tb-collapse'));
    expect(screen.queryByTestId('adapter-tb-keyboard')).toBeNull();
    await fireEvent.press(screen.getByTestId('adapter-tb-expand'));
    expect(screen.getByTestId('adapter-tb-keyboard')).toBeOnTheScreen();
  });

  it('⌃⌥⌘T folds and unfolds the toolbar', async () => {
    const { core } = await mount();
    await stream(core);

    await act(async () =>
      lastKeyboardProps().onShortcut?.({ action: 'toolbar' }),
    );
    expect(screen.getByTestId('adapter-tb-expand')).toBeOnTheScreen();
    await act(async () =>
      lastKeyboardProps().onShortcut?.({ action: 'toolbar' }),
    );
    expect(screen.getByTestId('adapter-tb-collapse')).toBeOnTheScreen();
  });

  it('can fold away with no handle left, remembered on this device', async () => {
    const first = await mount();
    await stream(first.core);

    await fireEvent.press(screen.getByTestId('adapter-tb-more'));
    await fireEvent(
      screen.getByTestId('adapter-more-hideHandle'),
      'valueChange',
      true,
    );
    expect(first.kv.getJSON(TOOLBAR_HANDLE_HIDDEN_KEY)).toBe(true);
    await fireEvent.press(screen.getByTestId('adapter-more-close'));
    await fireEvent.press(screen.getByTestId('adapter-tb-collapse'));
    expect(screen.queryByTestId('adapter-toolbar')).toBeNull();
    expect(screen.getByText(adapters.toast.toolbarHidden)).toBeOnTheScreen();
    await act(async () =>
      lastVideoProps().onGesture?.({ kind: 'threeFingerTap' }),
    );
    expect(screen.getByTestId('adapter-tb-collapse')).toBeOnTheScreen();
    await cleanup();

    const second = await mount({}, { kv: first.kv });
    await stream(second.core);
    await act(async () =>
      lastKeyboardProps().onShortcut?.({ action: 'toolbar' }),
    );
    expect(screen.queryByTestId('adapter-toolbar')).toBeNull();
    // Said once.
    expect(screen.queryByText(adapters.toast.toolbarHidden)).toBeNull();
  });

  it('remembers the docked edge on this device (V04)', async () => {
    const first = await mount();
    await stream(first.core);

    await fireEvent(
      screen.getByTestId('adapter-tb-collapse'),
      'accessibilityAction',
      {
        nativeEvent: { actionName: 'moveLeft' },
      },
    );
    expect(first.kv.getJSON(TOOLBAR_DOCK_KEY)).toBe('left');
    expect(screen.getByTestId('adapter-toolbar')).toHaveStyle({ left: 12 });
    await cleanup();

    const core = createFakeCore();
    installFakeAdapters(core);
    const navigation = mockNavigation<'AdapterSession'>();
    const view = await renderWithProviders(
      <AdapterSessionScreen
        navigation={navigation}
        route={mockRoute('AdapterSession', defaults)}
      />,
      { core, kv: first.kv },
    );
    clients.push(view.queryClient);
    await advance();
    await stream(core);
    expect(screen.getByTestId('adapter-toolbar')).toHaveStyle({ left: 12 });
  });

  it('shows the gesture coach once on the first picture (V61)', async () => {
    const view = await mount({}, { settings: { 'session.coachShown': false } });
    await streamWithoutFrame(view.core);
    expect(screen.queryByTestId('gesture-coach')).toBeNull();
    await firstFrame();

    expect(screen.getByTestId('gesture-coach')).toBeOnTheScreen();
    await fireEvent.press(screen.getByText(coach.start));
    expect(screen.queryByTestId('gesture-coach')).toBeNull();
    expect(view.settingsStore.get('session.coachShown')).toBe(true);

    await fireEvent.press(screen.getByTestId('adapter-tb-more'));
    await fireEvent.press(screen.getByTestId('adapter-more-gestures'));
    expect(screen.getByTestId('gesture-coach')).toBeOnTheScreen();
  });

  it('starts in the default mode from Settings (V24)', async () => {
    const { core } = await mount(
      {},
      { settings: { 'session.mode.phone': 'touch' } },
    );
    await stream(core);
    expect(lastVideoProps().mode).toBe('touch');
    await cleanup();
    // The first page folded its session away; this one connects afresh.
    resetAdapterLive();

    const pad = new FakePlatform();
    pad.info = { ...pad.info, isPad: true };
    const second = await mount(
      {},
      { platform: pad, settings: { 'session.mode.tablet': 'trackpad' } },
    );
    await stream(second.core);
    expect(lastVideoProps().mode).toBe('trackpad');
  });
});

describe('toolbar hints and iPad keys', () => {
  it('names a toolbar button on long press (V39)', async () => {
    const { core } = await mount();
    await stream(core);

    await fireEvent(screen.getByTestId('adapter-tb-keyboard'), 'longPress');
    expect(screen.getByText(adapters.toolbar.keyboard)).toBeOnTheScreen();
    await fireEvent(screen.getByTestId('adapter-tb-disconnect'), 'longPress');
    expect(screen.getByText(adapters.toolbar.disconnect)).toBeOnTheScreen();
    // A long press only explains; it does not disconnect.
    expect(screen.queryByTestId('adapter-disconnect-dialog-card')).toBeNull();
  });

  it('sends the shortcuts iPadOS keeps for itself (V41)', async () => {
    const pad = new FakePlatform();
    pad.info = { ...pad.info, isPad: true };
    const { core } = await mount({}, { platform: pad });
    await stream(core);

    expect(screen.queryByTestId('adapter-key-globe')).toBeNull();
    await fireEvent.press(screen.getByTestId('adapter-key-cmdSpace'));
    expect(core.calls).toContainEqual({
      method: 'sessions.combo',
      args: [ID, 'cmdSpace'],
    });
    await fireEvent(screen.getByTestId('adapter-key-cmdTab'), 'longPress');
    expect(screen.getByText(ipadSession.keys.cmdTab.a11y)).toBeOnTheScreen();
  });

  it('uses the Windows keys over remote desktop on iPad (V41)', async () => {
    const pad = new FakePlatform();
    pad.info = { ...pad.info, isPad: true };
    const { core } = await mount({ target: rdp }, { platform: pad });
    await stream(core);

    expect(screen.queryByTestId('adapter-key-cmdTab')).toBeNull();
    await fireEvent.press(screen.getByTestId('adapter-key-altTab'));
    expect(core.calls).toContainEqual({
      method: 'sessions.combo',
      args: [ID, 'altTab'],
    });
  });

  it('keeps the phone toolbar without shortcut keys (V41)', async () => {
    const { core } = await mount();
    await stream(core);
    expect(screen.queryByTestId('adapter-key-cmdTab')).toBeNull();
  });

  it('says so when a shortcut does not go through (V41)', async () => {
    const pad = new FakePlatform();
    pad.info = { ...pad.info, isPad: true };
    const core = createFakeCore();
    jest.spyOn(core.sessions, 'combo').mockImplementation(() => {
      throw new Error('gone');
    });
    await mount({}, { core, platform: pad });
    await stream(core);

    await fireEvent.press(screen.getByTestId('adapter-key-cmdTab'));
    expect(screen.getByText(adapters.toast.comboFailed)).toBeOnTheScreen();
  });
});

describe('feedback', () => {
  it('keeps a small unencrypted badge after connecting (V25)', async () => {
    const { core } = await mount(
      { addrKind: 'lan' },
      { settings: { 'tailnet.enabled': true } },
    );
    await stream(core);
    expect(screen.getByTestId('adapter-unencrypted-badge')).toHaveProp(
      'accessibilityLabel',
      adapters.unencryptedBadgeA11y,
    );
    await cleanup();

    const off = await mount({ addrKind: 'lan' });
    await stream(off.core);
    expect(screen.getByTestId('adapter-unencrypted-badge')).toHaveProp(
      'accessibilityLabel',
      adapters.unencryptedBadgeA11yLan,
    );
    await cleanup();

    const second = await mount();
    await stream(second.core);
    expect(screen.queryByTestId('adapter-unencrypted-badge')).toBeNull();
  });

  it('keeps the RDP takeover note for a while after connecting (V26)', async () => {
    const { core } = await mount({ target: rdp });
    await stream(core);

    expect(screen.getByTestId('adapter-takeover-note')).toBeOnTheScreen();
    await advance(TAKEOVER_NOTE_MS);
    expect(screen.queryByTestId('adapter-takeover-note')).toBeNull();
  });

  it('reports failed Ctrl+Alt+Del and password typing (V20)', async () => {
    const { core } = await mount(
      { target: rdp },
      { setup: f => f.failNext('sendCtrlAltDel', new Error('gone')) },
    );
    await stream(core);
    await fireEvent.press(screen.getByTestId('adapter-tb-more'));
    await fireEvent.press(screen.getByTestId('adapter-more-ctrlAltDel'));
    expect(screen.getByText(adapters.toast.ctrlAltDelFailed)).toBeOnTheScreen();
    await cleanup();

    const second = await mount(
      {},
      {
        setup: f => {
          f.setSaved(vnc, { saved: true, user: 'alice' });
          f.failNext('typePassword', new Error('gone'));
        },
      },
    );
    await stream(second.core);
    await fireEvent.press(screen.getByTestId('adapter-tb-typePassword'));
    expect(
      screen.getByText(adapters.toast.passwordTypeFailed),
    ).toBeOnTheScreen();
  });

  it('says when switching screens fails and when a screen goes away (V20, V44)', async () => {
    const layout = [
      { x: 0, y: 0, width: 1920, height: 1080 },
      { x: 1920, y: 0, width: 1920, height: 1080 },
      { x: 3840, y: 0, width: 1920, height: 1080 },
    ];
    const { core, fake } = await mount({ target: rdp });
    await stream(core);
    await act(async () => fake.emitScreens(ID, layout));

    await fireEvent.press(screen.getByTestId('adapter-tb-screens'));
    await fireEvent.press(screen.getByTestId('adapter-screen-2'));
    expect(callsOf(fake, 'selectScreen').at(-1)).toEqual({
      method: 'selectScreen',
      args: [ID, 2],
    });

    await act(async () => fake.emitScreens(ID, layout.slice(0, 1)));
    expect(screen.getByText(adapters.toast.screenGone)).toBeOnTheScreen();

    await act(async () => fake.emitScreens(ID, layout));
    fake.failNext('selectScreen', new Error('gone'));
    await fireEvent.press(screen.getByTestId('adapter-tb-screens'));
    await fireEvent.press(screen.getByTestId('adapter-screen-1'));
    expect(
      screen.getByText(adapters.toast.screenSwitchFailed),
    ).toBeOnTheScreen();
  });

  it('keeps the remote clipboard for 更多 while auto sync is off (V40)', async () => {
    const platform = new FakePlatform();
    const { core } = await mount(
      {},
      { platform, settings: { 'clipboard.autoSync': false } },
    );
    await stream(core);
    await fireEvent.press(screen.getByTestId('adapter-tb-more'));
    expect(screen.queryByTestId('adapter-more-remoteClipboard')).toBeNull();
    await fireEvent.press(screen.getByTestId('adapter-more-close'));

    await act(async () => {
      core.emit({ type: 'adapter.clipboard', sessionId: ID, text: 'x' });
    });
    await fireEvent.press(screen.getByTestId('adapter-tb-more'));
    await fireEvent.press(screen.getByTestId('adapter-more-remoteClipboard'));
    await advance();
    expect(platform.copied).toEqual(['x']);
    expect(
      screen.getByText(adapters.toast.remoteClipboardCopied),
    ).toBeOnTheScreen();
  });

  it('announces received clipboard text only the first time (V40)', async () => {
    const platform = new FakePlatform();
    const { core } = await mount(
      {},
      { platform, settings: { 'clipboard.autoSync': true } },
    );
    await stream(core);
    await act(async () => {
      core.emit({ type: 'adapter.clipboard', sessionId: ID, text: 'a' });
    });
    expect(
      screen.getByText(adapters.toast.clipboardReceived),
    ).toBeOnTheScreen();
    await advance(10000);
    expect(screen.queryByText(adapters.toast.clipboardReceived)).toBeNull();

    await act(async () => {
      core.emit({ type: 'adapter.clipboard', sessionId: ID, text: 'b' });
    });
    await advance();
    expect(platform.copied).toEqual(['a', 'b']);
    expect(screen.queryByText(adapters.toast.clipboardReceived)).toBeNull();
  });

  it('switches the RDP sound from 更多 (V42)', async () => {
    const view = await mount({ target: rdp });
    await stream(view.core);
    await fireEvent.press(screen.getByTestId('adapter-tb-more'));

    await fireEvent.press(screen.getByTestId('adapter-more-sound'));
    expect(view.settingsStore.get('session.sound')).toBe(true);
  });

  it('the host-ready banner can be dismissed (V22)', async () => {
    const core = createFakeCore();
    mockDirectory = hostH1(false);
    await mount({ context: 'loginWindow' }, { core });
    await stream(core);
    await advance();

    await fireEvent.press(screen.getByTestId('adapter-host-ready-dismiss'));
    expect(screen.queryByTestId('adapter-host-ready')).toBeNull();
  });

  it('the host-ready banner needs a known console state (V37)', async () => {
    const unknown = hostH1(false);
    const core = createFakeCore();
    mockDirectory = {
      ...unknown,
      computers: unknown.computers.map(computer => ({
        ...computer,
        console: null,
      })),
    };
    await mount({ context: 'loginWindow' }, { core });
    await stream(core);
    await advance();

    expect(screen.queryByTestId('adapter-host-ready')).toBeNull();
  });
});

describe('error page', () => {
  it('a timed-out certificate question says so (V18)', async () => {
    const { core } = await mount({ target: rdp });
    await act(async () => {
      core.emit({ type: 'adapter.certPrompt', sessionId: ID, fp: 'aa:bb' });
    });
    await endWith(core, {
      code: 'certRejected',
      rsCode: ADAPTER_RS_E.certRejected,
      message: 'no answer',
    });

    expect(
      within(screen.getByTestId('adapter-error')).getByText(
        adapters.errors.certTimeoutTitle,
      ),
    ).toBeOnTheScreen();
  });

  it('a certificate the user rejected keeps the plain title', async () => {
    const { core } = await mount({ target: rdp });
    await act(async () => {
      core.emit({ type: 'adapter.certPrompt', sessionId: ID, fp: 'aa:bb' });
    });
    await fireEvent.press(screen.getByText(adapters.cert.reject));
    await endWith(core, {
      code: 'certRejected',
      rsCode: ADAPTER_RS_E.certRejected,
      message: 'rejected',
    });

    expect(
      within(screen.getByTestId('adapter-error')).getByText(
        adapters.errors.title('certRejected', 'Mac mini', 'rdp'),
      ),
    ).toBeOnTheScreen();
  });

  it('nlaRequired offers to retry with an account and password (V19)', async () => {
    const { core, fake } = await mount({ target: rdp });
    await endWith(core, {
      code: 'nlaRequired',
      rsCode: ADAPTER_RS_E.unsupported,
      message: 'nla',
    });

    await fireEvent.press(screen.getByText(adapters.errors.retryWithPassword));
    await advance();
    expect(callsOf(fake, 'open').at(-1)?.args[1]).toMatchObject({
      useSaved: false,
    });
  });
});

describe('credentials', () => {
  it('a failed login brings back the user name and domain (V14)', async () => {
    const { core, fake } = await mount({ target: rdp });
    const fields = ['user', 'password', 'domain'] as const;
    await emitState(core, 'needsCredentials', { credFields: [...fields] });
    await fireEvent.changeText(screen.getByTestId('adapter-creds-user'), 'bob');
    await fireEvent.changeText(
      screen.getByTestId('adapter-creds-password'),
      'pw',
    );
    await fireEvent.changeText(
      screen.getByTestId('adapter-creds-domain'),
      'CORP',
    );
    await fireEvent.press(screen.getByTestId('adapter-creds-submit'));
    expect(callsOf(fake, 'submitCredentials')).toHaveLength(1);

    await emitState(core, 'authenticating');
    await emitState(core, 'needsCredentials', {
      credFields: [...fields],
      credError: true,
    });
    expect(screen.getByTestId('adapter-creds-user')).toHaveProp('value', 'bob');
    expect(screen.getByTestId('adapter-creds-domain')).toHaveProp(
      'value',
      'CORP',
    );
    expect(screen.getByTestId('adapter-creds-password')).toHaveProp(
      'value',
      '',
    );
    expect(screen.getByTestId('adapter-creds-failed')).toBeOnTheScreen();
  });

  it('a submit that does not reach the computer is shown', async () => {
    const { core } = await mount(
      {},
      { setup: f => f.failNext('submitCredentials', new Error('gone')) },
    );
    await emitState(core, 'needsCredentials', { credFields: ['password'] });
    await fireEvent.changeText(
      screen.getByTestId('adapter-creds-password'),
      'pw',
    );
    await fireEvent.press(screen.getByTestId('adapter-creds-submit'));

    expect(screen.getByTestId('adapter-creds-submit-failed')).toBeOnTheScreen();
    expect(core.calls).toContainEqual({
      method: 'logs.write',
      args: ['warn', 'adapters', 'submit credentials failed'],
    });
  });
});

describe('app lock (S03)', () => {
  it('asks before connecting when the lock covers every session', async () => {
    const platform = new FakePlatform();
    const { fake } = await mount(
      {},
      { platform, settings: { 'lock.beforeSession': true } },
    );

    expect(platform.calls).toContainEqual({
      method: 'authenticate',
      args: [adapters.authenticate('Mac mini')],
    });
    expect(callsOf(fake, 'open')).toHaveLength(1);
  });

  it('leaves without connecting when verification is cancelled', async () => {
    const platform = new FakePlatform();
    platform.authResult = 'cancelled';
    const { fake, navigation } = await mount(
      {},
      { platform, settings: { 'lock.sensitiveHosts': ['h1'] } },
    );

    expect(callsOf(fake, 'open')).toEqual([]);
    expect(navigation.goBack).toHaveBeenCalledTimes(1);
  });

  it('does not ask when the lock is off', async () => {
    const platform = new FakePlatform();
    await mount({}, { platform });

    expect(
      platform.calls.filter(call => call.method === 'authenticate'),
    ).toEqual([]);
  });
});

describe('history (H05, H06)', () => {
  it('writes one 最近 entry named from the computer list', async () => {
    const core = createFakeCore();
    // h1 is paired with this phone, so the local directory names it.
    const view = await mount(
      {},
      { core, localDirectory: { computers: hostH1(false).computers } },
    );
    await advance();
    await stream(core);

    expect(view.kv.getJSON(ADAPTER_LIVE_KEY)).toEqual([
      expect.objectContaining({ kind: 'adapter', targetId: 'host:h1' }),
    ]);
    await advance(5000);
    await fireEvent.press(screen.getByTestId('adapter-tb-disconnect'));
    await fireEvent.press(screen.getByTestId('adapter-disconnect-confirm'));
    await cleanup();

    expect(readHistory(view.kv)).toEqual([
      expect.objectContaining({
        kind: 'adapter',
        protocol: 'vnc',
        targetId: 'host:h1',
        name: computers[0]?.name,
        durationS: 5,
      }),
    ]);
    expect(view.kv.getJSON(ADAPTER_LIVE_KEY)).toBeUndefined();
  });

  it('keeps one entry across a reconnect and records the last error', async () => {
    const typed: AdapterTarget = { protocol: 'vnc', addr: '192.168.9.9' };
    const view = await mount({ addrKind: 'manual', target: typed });
    await stream(view.core);
    await endWith(view.core, DROP);
    await advance(1000);
    await stream(view.core, SECOND_ID);
    await endWith(
      view.core,
      {
        code: 'authFailed',
        rsCode: ADAPTER_RS_E.authFailed,
        message: 'locked out',
      },
      SECOND_ID,
    );
    await cleanup();

    const history = readHistory(view.kv);
    expect(history).toHaveLength(1);
    expect(history[0]).toMatchObject({
      kind: 'adapter',
      targetId: 'addr:vnc:192.168.9.9',
      name: 'Mac mini',
      endReason: 'authFailed',
      code: ADAPTER_RS_E.authFailed,
    });
  });

  it('writes nothing when the session never showed a picture', async () => {
    const view = await mount();
    await endWith(view.core, DROP);
    await cleanup();

    expect(readHistory(view.kv)).toEqual([]);
  });
});

describe('collapse (V33)', () => {
  /** Streams, folds the page away and unmounts it, as leaving would. */
  async function collapsed(opts: MountOptions = {}) {
    const first = await mount({}, opts);
    await stream(first.core);
    await fireEvent.press(screen.getByTestId('adapter-tb-minimize'));
    await cleanup();
    return first;
  }

  function record(): CollapsedAdapter {
    const found = adapterLive.peek(ID);
    if (found === undefined) {
      throw new Error('no collapsed session');
    }
    return found;
  }

  it('收起 keeps the connection and hands the session over', async () => {
    const first = await mount();
    await stream(first.core);

    await fireEvent.press(screen.getByTestId('adapter-tb-minimize'));
    expect(first.navigation.popTo).toHaveBeenCalledWith('MainTabs', {
      screen: 'Home',
    });
    await cleanup();

    expect(callsOf(first.fake, 'close')).toEqual([]);
    expect(record()).toMatchObject({
      sessionId: ID,
      phase: 'streaming',
      params: defaults,
      width: 1920,
      height: 1080,
      mode: 'trackpad',
    });
    // The 最近 entry is written when the session really ends.
    expect(readHistory(first.kv)).toEqual([]);
    expect(first.kv.getJSON(ADAPTER_LIVE_KEY)).toHaveLength(1);
  });

  it('closes a session that never showed a picture when the page goes', async () => {
    const { fake } = await mount();
    await cleanup();

    expect(callsOf(fake, 'close')).toEqual([{ method: 'close', args: [ID] }]);
    expect(adapterLive.peek(ID)).toBeUndefined();
  });

  it('returns by session id without connecting again', async () => {
    const first = await collapsed();
    const second = await mount({ sessionId: ID }, { core: first.core });

    expect(callsOf(second.fake, 'open')).toEqual([]);
    expect(adapterLive.peek(ID)).toBeUndefined();
    expect(lastVideoProps().sessionId).toBe(ID);
    expect(screen.queryByTestId('adapter-waiting')).toBeNull();
    expect(screen.getByTestId('adapter-toolbar')).toBeOnTheScreen();

    // Still the same session: disconnecting closes it.
    await fireEvent.press(screen.getByTestId('adapter-tb-disconnect'));
    await fireEvent.press(screen.getByTestId('adapter-disconnect-confirm'));
    expect(callsOf(second.fake, 'close')).toEqual([
      { method: 'close', args: [ID] },
    ]);
  });

  it('tapping the computer again picks up its collapsed session', async () => {
    const first = await collapsed();
    const second = await mount({}, { core: first.core });

    expect(callsOf(second.fake, 'open')).toEqual([]);
    expect(lastVideoProps().sessionId).toBe(ID);
  });

  it('keeps the input mode it was folded away with', async () => {
    const first = await mount();
    await stream(first.core);
    await fireEvent.press(screen.getByTestId('adapter-tb-mode'));
    await fireEvent.press(screen.getByTestId('adapter-tb-minimize'));
    await cleanup();

    await mount({ sessionId: ID }, { core: first.core });
    expect(lastVideoProps().mode).toBe('touch');
  });

  it('asks the App lock before picking a session up', async () => {
    const first = await collapsed();
    const platform = new FakePlatform();
    platform.authResult = 'cancelled';
    const second = await mount(
      { sessionId: ID },
      {
        core: first.core,
        platform,
        settings: { 'lock.beforeSession': true },
      },
    );

    expect(platform.calls).toContainEqual({
      method: 'authenticate',
      args: [adapters.authenticate('Mac mini')],
    });
    expect(second.navigation.goBack).toHaveBeenCalledTimes(1);
    // Not verified: it stays folded away, still connected.
    expect(callsOf(second.fake, 'close')).toEqual([]);
    expect(callsOf(second.fake, 'open')).toEqual([]);
    expect(record().phase).toBe('streaming');
  });

  it('picks the session up once the App lock lets it', async () => {
    const first = await collapsed();
    const platform = new FakePlatform();
    const second = await mount(
      { sessionId: ID },
      {
        core: first.core,
        platform,
        settings: { 'lock.sensitiveHosts': ['h1'] },
      },
    );

    expect(
      platform.calls.filter(call => call.method === 'authenticate'),
    ).toHaveLength(1);
    expect(callsOf(second.fake, 'open')).toEqual([]);
    expect(lastVideoProps().sessionId).toBe(ID);
  });

  it('connects again by itself when the session dropped while away', async () => {
    const first = await collapsed();
    adapterLive.end(ID, DROP);
    const second = await mount({ sessionId: ID }, { core: first.core });

    expect(callsOf(second.fake, 'open')).toHaveLength(1);
    expect(screen.getByTestId('adapter-waiting')).toBeOnTheScreen();
    expect(adapterLive.peek(ID)).toBeUndefined();
  });

  it('shows why a session ended while away, and retries from there', async () => {
    const first = await collapsed();
    adapterLive.end(ID, {
      code: 'serverClosed',
      rsCode: ADAPTER_RS_E.broken,
      message: 'taken over',
    });
    const second = await mount({ sessionId: ID }, { core: first.core });

    expect(callsOf(second.fake, 'open')).toEqual([]);
    expect(
      screen.getByText(adapters.errors.title('serverClosed', 'Mac mini')),
    ).toBeOnTheScreen();
    await fireEvent.press(screen.getByText(adapters.errors.retry));
    expect(callsOf(second.fake, 'open')).toHaveLength(1);
  });

  it('writes one 最近 entry across folding away and returning', async () => {
    const first = await mount();
    await stream(first.core);
    await advance(5000);
    await fireEvent.press(screen.getByTestId('adapter-tb-minimize'));
    await cleanup();
    await advance(2000);

    await mount({ sessionId: ID }, { core: first.core, kv: first.kv });
    await advance(3000);
    await fireEvent.press(screen.getByTestId('adapter-tb-disconnect'));
    await fireEvent.press(screen.getByTestId('adapter-disconnect-confirm'));
    await cleanup();

    const history = readHistory(first.kv);
    expect(history).toHaveLength(1);
    expect(history[0]).toMatchObject({ kind: 'adapter', durationS: 10 });
    expect(first.kv.getJSON(ADAPTER_LIVE_KEY)).toBeUndefined();
  });
});

describe('iPad keyboard work (RS-IPAD1)', () => {
  it('counts the open page as a live session until it ends', async () => {
    resetAdapterPages();
    // No Remote Host session in the fixtures, so the count is the page alone.
    const view = await mount({}, { core: createFakeCore({ sessions: [] }) });
    const live = await renderHook(() => useLiveSessionCount(), {
      wrapper: view.wrapper,
    });
    // Connecting already keeps the screen awake.
    expect(live.result.current).toBe(1);
    await stream(view.core);
    expect(live.result.current).toBe(1);

    await endWith(view.core, {
      code: 'authFailed',
      rsCode: ADAPTER_RS_E.authFailed,
      message: 'three failures',
    });
    // `screen` (and toBeOnTheScreen) follow the hook's later render, so the
    // error page is looked up in the page's own render.
    expect(view.queryByTestId('adapter-error')).not.toBeNull();
    expect(live.result.current).toBe(0);
  });

  it('shows the pointer-lock chip on iPad when the pointer escaped', async () => {
    const pad = new FakePlatform();
    pad.info = { ...pad.info, isPad: true };
    const { core } = await mount({}, { platform: pad });
    await stream(core);
    expect(screen.queryByTestId('pointer-lock-chip')).toBeNull();

    await act(async () =>
      setPointerLock(ID, { mouse: true, locked: false, reason: 'escaped' }),
    );
    expect(screen.getByTestId('pointer-lock-chip')).toBeOnTheScreen();
    expect(screen.getByText(ipadSession.pointer.escaped)).toBeOnTheScreen();

    await act(async () =>
      setPointerLock(ID, { mouse: true, locked: true, reason: 'locked' }),
    );
    expect(screen.queryByTestId('pointer-lock-chip')).toBeNull();
    await act(async () => clearPointerLock(ID));
  });

  it('pushes the clipboard before a paste, warning about Chinese over VNC', async () => {
    const platform = new FakePlatform();
    const { core } = await mount({}, { platform });
    await stream(core);

    const beforePaste = lastKeyboardProps().onBeforePaste;
    expect(beforePaste).toBeDefined();
    platform.clipboardText = 'hi';
    await act(async () => beforePaste?.());
    expect(core.calls).toContainEqual({
      method: 'sessions.clipboardSend',
      args: [ID, 'hi'],
    });
    // A paste is not worth a toast of its own.
    expect(screen.queryByText(adapters.toast.clipboardSent)).toBeNull();

    platform.clipboardText = '你好';
    await act(async () => lastKeyboardProps().onBeforePaste?.());
    expect(
      screen.getByText('屏幕共享只能粘贴英文和符号，中文请在电脑上输入'),
    ).toBeOnTheScreen();
    expect(
      core.calls.filter(call => call.method === 'sessions.clipboardSend'),
    ).toHaveLength(1);

    platform.clipboardText = null;
    await act(async () => lastKeyboardProps().onBeforePaste?.());
    expect(
      core.calls.filter(call => call.method === 'sessions.clipboardSend'),
    ).toHaveLength(1);
  });

  it('pushes Chinese text before a paste over remote desktop', async () => {
    const platform = new FakePlatform();
    const { core } = await mount({ target: rdp }, { platform });
    await stream(core);

    platform.clipboardText = '你好';
    await act(async () => lastKeyboardProps().onBeforePaste?.());
    expect(core.calls).toContainEqual({
      method: 'sessions.clipboardSend',
      args: [ID, '你好'],
    });

    platform.clipboardText = '再见';
    core.failNext('sessions.clipboardSend', new Error('gone'));
    await act(async () => lastKeyboardProps().onBeforePaste?.());
    expect(core.calls).toContainEqual({
      method: 'logs.write',
      args: ['warn', 'adapters', 'clipboard sync before paste failed'],
    });
  });

  it('pastes over this device clipboard only when it is the newer one', async () => {
    const platform = new FakePlatform();
    const { core } = await mount({ target: rdp }, { platform });
    await stream(core);
    const sends = () =>
      core.calls
        .filter(call => call.method === 'sessions.clipboardSend')
        .map(call => call.args[1]);

    // A screenshot on the computer after the copy here.
    platform.clipboardText = 'old';
    await act(async () => {
      core.emit({ type: 'adapter.clipboardOther', sessionId: ID });
    });
    await act(async () => lastKeyboardProps().onBeforePaste?.());
    expect(sends()).toEqual([]);

    platform.clipboardText = 'new';
    await act(async () => lastKeyboardProps().onBeforePaste?.());
    await act(async () => lastKeyboardProps().onBeforePaste?.());
    expect(sends()).toEqual(['new']);

    // Text copied on the computer comes here already; nothing to send back.
    await act(async () => {
      core.emit({ type: 'adapter.clipboard', sessionId: ID, text: 'remote' });
    });
    await act(async () => lastKeyboardProps().onBeforePaste?.());
    expect(sends()).toEqual(['new']);

    // Without a change count (Android) every paste brings it over.
    platform.clipboardCount = null;
    await act(async () => lastKeyboardProps().onBeforePaste?.());
    expect(sends()).toEqual(['new', 'new']);
  });

  it('offers 适应屏幕 in 更多 only while zoomed in', async () => {
    const { core } = await mount();
    await stream(core);

    await fireEvent.press(screen.getByTestId('adapter-tb-more'));
    expect(screen.queryByTestId('adapter-more-fitScreen')).toBeNull();
    await fireEvent.press(screen.getByTestId('adapter-more-close'));

    await act(async () =>
      lastVideoProps().onViewport?.({ zoom: 2, x: 0, y: 0 }),
    );
    await fireEvent.press(screen.getByTestId('adapter-tb-more'));
    await fireEvent.press(screen.getByTestId('adapter-more-fitScreen'));
    expect(screen.queryByTestId('adapter-more')).toBeNull();
    const handle = lastVideoProps().ref as RefObject<RemoteSessionHandle>;
    expect(handle.current?.zoomReset).toHaveBeenCalledTimes(1);

    await act(async () =>
      lastVideoProps().onViewport?.({ zoom: 1, x: 0, y: 0 }),
    );
    await fireEvent.press(screen.getByTestId('adapter-tb-more'));
    expect(screen.queryByTestId('adapter-more-fitScreen')).toBeNull();
  });

  it('offers the new iPad shortcuts on the toolbar', async () => {
    const pad = new FakePlatform();
    pad.info = { ...pad.info, isPad: true };
    const { core } = await mount({}, { platform: pad });
    await stream(core);

    for (const combo of ['ctrlSpace', 'cmdH', 'cmdShift4'] as const) {
      await fireEvent.press(screen.getByTestId(`adapter-key-${combo}`));
      expect(core.calls).toContainEqual({
        method: 'sessions.combo',
        args: [ID, combo],
      });
    }
    await cleanup();
    resetAdapterLive();

    const rdpPad = new FakePlatform();
    rdpPad.info = { ...rdpPad.info, isPad: true };
    const second = await mount({ target: rdp }, { platform: rdpPad });
    await stream(second.core);
    await fireEvent.press(screen.getByTestId('adapter-key-winSpace'));
    expect(second.core.calls).toContainEqual({
      method: 'sessions.combo',
      args: [ID, 'winSpace'],
    });
  });
});

describe('session switcher (RS-MULTI1)', () => {
  const windows: RootStackParamList['AdapterSession'] = {
    target: { protocol: 'rdp', hostId: 'h2', addr: '100.64.0.9' },
    name: 'Win PC',
    addrKind: 'tailnet',
  };

  /** A Remote Desktop session folded away before this page opened. */
  function foldedWindows(sessionId = SECOND_ID): void {
    const startedAtUs = FIXTURE_NOW - 1000e6;
    adapterLive.put({
      sessionId,
      params: windows,
      name: 'Win PC',
      phase: 'streaming',
      error: null,
      screens: [],
      screenIndex: 0,
      mode: 'trackpad',
      startedAtUs,
      collapsedAtUs: startedAtUs,
      history: {
        live: null,
        stoppedAtUs: null,
        error: null,
        name: 'Win PC',
        targetId: 'host:h2',
      },
    });
  }

  function backgroundCalls(core: FakeCore) {
    return core.calls.filter(call => call.method === 'sessions.background');
  }

  /** The Host session in the fixtures (oldest), Win PC, then this page. */
  async function threeSessions() {
    foldedWindows();
    const view = await mount();
    await stream(view.core);
    return view;
  }

  beforeEach(() => {
    // The fixture Host session started before this page did.
    jest.setSystemTime(FIXTURE_NOW / 1000);
  });

  afterEach(() => {
    resetArrival();
    mockAnimateSpace.mockClear();
  });

  it('has the button with this session alone, marked current', async () => {
    const { core, navigation } = await mount(
      {},
      { core: createFakeCore({ sessions: [] }) },
    );
    await stream(core);

    await fireEvent.press(screen.getByTestId('adapter-tb-switch'));
    expect(screen.getByTestId('session-switcher')).toBeOnTheScreen();
    expect(screen.getByTestId(`switcher-item-${ID}`)).toBeSelected();
    expect(screen.getAllByTestId(/^switcher-item-/)).toHaveLength(1);
    expect(screen.getByTestId('switcher-other')).toBeOnTheScreen();

    // Tapping the current one only closes the list.
    await fireEvent.press(screen.getByTestId(`switcher-item-${ID}`));
    expect(screen.queryByTestId('session-switcher')).toBeNull();
    expect(navigation.replace).not.toHaveBeenCalled();
    expect(navigation.navigate).not.toHaveBeenCalled();
  });

  it('tapping the current session closes the switcher and leaves the page working (RS-SPACES1)', async () => {
    const { core, navigation } = await threeSessions();
    await fireEvent.press(screen.getByTestId('adapter-tb-switch'));
    const sheet = screen.getByTestId('session-switcher');
    // Drawn in the page, not in a native Modal over the session.
    expect(
      within(screen.getByTestId('adapter-session-body')).getByTestId(
        'session-switcher',
      ),
    ).toBe(sheet);
    const modals = hostTypesAbove(sheet).filter(type => /modal/i.test(type));
    expect(modals).toEqual([]);

    await fireEvent.press(screen.getByTestId(`switcher-item-${ID}`));
    expect(screen.queryByTestId('session-switcher')).toBeNull();
    expect(navigation.replace).not.toHaveBeenCalled();
    expect(navigation.navigate).not.toHaveBeenCalled();
    expect(backgroundCalls(core)).toEqual([]);

    // The toolbar still answers; X and the backdrop close the list again.
    await fireEvent.press(screen.getByTestId('adapter-tb-switch'));
    await fireEvent.press(screen.getByTestId('session-switcher-close'));
    expect(screen.queryByTestId('session-switcher')).toBeNull();
    await fireEvent.press(screen.getByTestId('adapter-tb-switch'));
    await fireEvent.press(screen.getByTestId(`switcher-item-${ID}`));
    await fireEvent.press(screen.getByTestId('adapter-tb-switch'));
    await fireEvent.press(screen.getByTestId('session-switcher-backdrop'));
    expect(screen.queryByTestId('session-switcher')).toBeNull();
    expect(screen.getByTestId('adapter-tb-switch')).toBeOnTheScreen();
    expect(navigation.replace).not.toHaveBeenCalled();
  });

  it('folding away pops back to the tabs instead of stacking another copy (RS-SPACES1)', async () => {
    const { core, navigation } = await mount();
    await stream(core);

    await fireEvent.press(screen.getByTestId('adapter-tb-switch'));
    await fireEvent.press(screen.getByTestId('switcher-other'));
    // React Navigation 7's navigate would push a second MainTabs and leave
    // this page mounted, live and never folded away underneath.
    expect(navigation.popTo).toHaveBeenCalledWith('MainTabs', {
      screen: 'Home',
    });
    expect(navigation.navigate).not.toHaveBeenCalled();
  });

  it('a page leaving after another page picked its session up leaves it running (RS-SPACES1)', async () => {
    const { core, fake } = await mount();
    await stream(core);
    // The newer page is in front with this session and has resumed it.
    const release = showAdapterSession(ID, Symbol('newer page'));

    await cleanup();
    // Not paused, not folded away again, not closed.
    expect(backgroundCalls(core)).toEqual([]);
    expect(adapterLive.peek(ID)).toBeUndefined();
    expect(callsOf(fake, 'close')).toEqual([]);
    release();
  });

  it('a page leaving on its own still folds its session away and pauses it', async () => {
    const { core } = await mount();
    await stream(core);
    const release = showAdapterSession(SECOND_ID, Symbol('other session'));

    await cleanup();
    expect(backgroundCalls(core)).toEqual([
      { method: 'sessions.background', args: [ID, true] },
    ]);
    expect(adapterLive.peek(ID)).toMatchObject({ sessionId: ID });
    release();
  });

  it('lists Host, folded-away and terminal sessions by start time', async () => {
    await threeSessions();
    await fireEvent.press(screen.getByTestId('adapter-tb-switch'));

    expect(
      screen.getAllByTestId(/^switcher-item-/).map(item => item.props.testID),
    ).toEqual([
      `switcher-item-${liveSession.id}`,
      `switcher-item-${SECOND_ID}`,
      `switcher-item-${ID}`,
    ]);
    expect(screen.getByTestId(`switcher-item-${ID}`)).toBeSelected();
    expect(screen.queryByTestId('switcher-many')).toBeNull();
  });

  it('folds this session away and replaces the page with the tapped one', async () => {
    const { core, fake, navigation } = await threeSessions();
    await fireEvent.press(screen.getByTestId('adapter-tb-switch'));

    await fireEvent.press(screen.getByTestId(`switcher-item-${SECOND_ID}`));
    expect(navigation.replace).toHaveBeenCalledWith('AdapterSession', {
      ...windows,
      sessionId: SECOND_ID,
    });
    expect(navigation.navigate).not.toHaveBeenCalled();
    // The page goes once replaced: the session stays connected, paused.
    await cleanup();
    expect(callsOf(fake, 'close')).toEqual([]);
    expect(adapterLive.peek(ID)).toMatchObject({ phase: 'streaming' });
    expect(backgroundCalls(core)).toEqual([
      { method: 'sessions.background', args: [ID, true] },
    ]);
  });

  it('goes to a Host session by its id', async () => {
    const { navigation } = await threeSessions();
    await fireEvent.press(screen.getByTestId('adapter-tb-switch'));

    await fireEvent.press(
      screen.getByTestId(`switcher-item-${liveSession.id}`),
    );
    expect(navigation.replace).toHaveBeenCalledWith('Session', {
      hostId: liveSession.hostId,
      sessionId: liveSession.id,
    });
  });

  it('连接其他电脑 folds this session away and opens the computer list', async () => {
    const { core, fake, navigation } = await threeSessions();
    await fireEvent.press(screen.getByTestId('adapter-tb-switch'));

    await fireEvent.press(screen.getByTestId('switcher-other'));
    expect(navigation.popTo).toHaveBeenCalledWith('MainTabs', {
      screen: 'Home',
    });
    expect(navigation.replace).not.toHaveBeenCalled();
    await cleanup();
    expect(callsOf(fake, 'close')).toEqual([]);
    expect(backgroundCalls(core)).toEqual([
      { method: 'sessions.background', args: [ID, true] },
    ]);
  });

  const toHost = {
    route: 'Session',
    params: { hostId: liveSession.hostId, sessionId: liveSession.id },
  };
  const toWindows = {
    route: 'AdapterSession',
    params: { ...windows, sessionId: SECOND_ID },
  };
  const shortcutCases: {
    name: string;
    shortcut: SessionShortcutEvent;
    to: { route: string; params: object };
  }[] = [
    { name: 'next', shortcut: { action: 'next' }, to: toHost },
    { name: 'previous', shortcut: { action: 'previous' }, to: toWindows },
    { name: 'the first', shortcut: { action: 'index', index: 1 }, to: toHost },
    {
      name: 'the second',
      shortcut: { action: 'index', index: 2 },
      to: toWindows,
    },
  ];
  it.each(shortcutCases)('⌃⌥⌘ $name switches', async ({ shortcut, to }) => {
    const { navigation } = await threeSessions();

    await act(async () => lastKeyboardProps().onShortcut?.(shortcut));
    expect(navigation.replace).toHaveBeenCalledTimes(1);
    expect(navigation.replace).toHaveBeenCalledWith(to.route, to.params);
  });

  it('⌃⌥⌘ numbers past the list or of this session do nothing', async () => {
    const { navigation } = await threeSessions();

    for (const index of [3, 4, 9]) {
      await act(async () =>
        lastKeyboardProps().onShortcut?.({ action: 'index', index }),
      );
    }
    expect(navigation.replace).not.toHaveBeenCalled();
    expect(navigation.navigate).not.toHaveBeenCalled();
  });

  it('⌃⌥⌘S opens the switcher', async () => {
    await threeSessions();

    await act(async () =>
      lastKeyboardProps().onShortcut?.({ action: 'switcher' }),
    );
    expect(screen.getByTestId('session-switcher')).toBeOnTheScreen();
  });

  it('only switches once while the page is going', async () => {
    const { navigation } = await threeSessions();

    await act(async () =>
      lastKeyboardProps().onShortcut?.({ action: 'next' }),
    );
    await act(async () =>
      lastKeyboardProps().onShortcut?.({ action: 'previous' }),
    );
    expect(navigation.replace).toHaveBeenCalledTimes(1);
  });

  it('⌃⌥⌘ plays the swipe slide the way the target lies, then switches (RS-SPACES1)', async () => {
    const { navigation } = await threeSessions();

    await act(async () =>
      lastKeyboardProps().onShortcut?.({ action: 'index', index: 1 }),
    );
    expect(mockAnimateSpace).toHaveBeenCalledWith(liveSession.id, 'previous');
    expect(navigation.replace).toHaveBeenCalledTimes(1);
    expect(mockAnimateSpace.mock.invocationCallOrder[0]).toBeLessThan(
      jest.mocked(navigation.replace).mock.invocationCallOrder[0],
    );
  });

  it('three fingers to a neighbour fold this session away, then replace the page (RS-SPACES1)', async () => {
    const { core, fake, navigation } = await threeSessions();
    expect(lastVideoProps().spaces?.map(space => space.id)).toEqual([
      liveSession.id,
      SECOND_ID,
      ID,
    ]);

    await act(async () => lastVideoProps().onSpaceSwipe?.({ phase: 'began' }));
    expect(screen.getByTestId('space-indicator')).toBeOnTheScreen();
    expect(screen.getByTestId('space-dot-2')).toBeSelected();
    await act(async () => lastVideoProps().onSpaceSwipe?.({ phase: 'ended' }));
    await act(async () =>
      lastVideoProps().onSpaceSwitch?.({ sessionId: SECOND_ID }),
    );
    // A second report of the same slide does not switch again.
    await act(async () =>
      lastVideoProps().onSpaceSwitch?.({ sessionId: SECOND_ID }),
    );
    expect(navigation.replace).toHaveBeenCalledTimes(1);
    expect(navigation.replace).toHaveBeenCalledWith('AdapterSession', {
      ...windows,
      sessionId: SECOND_ID,
    });
    expect(peekArrival(SECOND_ID)).toBe(true);

    // Replaced: the session stays connected, folded away and paused.
    await cleanup();
    expect(callsOf(fake, 'close')).toEqual([]);
    expect(adapterLive.peek(ID)).toMatchObject({ phase: 'streaming' });
    expect(backgroundCalls(core)).toEqual([
      { method: 'sessions.background', args: [ID, true] },
    ]);
  });

  it('a page switched to shows the last picture until its own first frame (RS-SPACES1)', async () => {
    foldedWindows();
    markArrival(SECOND_ID);
    await mount({ ...windows, sessionId: SECOND_ID });

    expect(lastVideoProps().sessionId).toBe(SECOND_ID);
    expect(lastVideoProps().showSnapshot).toBe(true);
    expect(screen.queryByTestId('adapter-waiting')).toBeNull();
    expect(screen.getByTestId('space-indicator')).toBeOnTheScreen();
    expect(screen.getByTestId('space-indicator-name')).toHaveTextContent(
      'Win PC',
    );

    await firstFrame();
    expect(lastVideoProps().showSnapshot).toBe(false);
    await advance(2000);
    expect(screen.queryByTestId('space-indicator')).toBeNull();
  });

  it('a page picked up from the list shows no snapshot or dots (RS-SPACES1)', async () => {
    foldedWindows();
    await mount({ ...windows, sessionId: SECOND_ID });
    expect(lastVideoProps().showSnapshot).toBe(false);
    expect(screen.queryByTestId('space-indicator')).toBeNull();
  });

  it('pauses the picture when folded away and resumes it on return', async () => {
    const first = await mount();
    await stream(first.core);
    await fireEvent.press(screen.getByTestId('adapter-tb-switch'));
    await fireEvent.press(
      screen.getByTestId(`switcher-item-${liveSession.id}`),
    );
    await cleanup();
    expect(backgroundCalls(first.core)).toEqual([
      { method: 'sessions.background', args: [ID, true] },
    ]);

    const second = await mount({ sessionId: ID }, { core: first.core });
    expect(callsOf(second.fake, 'open')).toEqual([]);
    expect(backgroundCalls(first.core)).toEqual([
      { method: 'sessions.background', args: [ID, true] },
      { method: 'sessions.background', args: [ID, false] },
    ]);
  });

  it('does not resume a session that ended while away', async () => {
    const first = await mount();
    await stream(first.core);
    await fireEvent.press(screen.getByTestId('adapter-tb-minimize'));
    await cleanup();
    adapterLive.end(ID, DROP);

    await mount({ sessionId: ID }, { core: first.core });
    expect(backgroundCalls(first.core)).toEqual([
      { method: 'sessions.background', args: [ID, true] },
    ]);
  });

  it('says it costs battery with more than four sessions open', async () => {
    for (const id of [SECOND_ID, THIRD_ID, FOURTH_ID]) {
      foldedWindows(id);
    }
    const { core } = await mount();
    await stream(core);
    await fireEvent.press(screen.getByTestId('adapter-tb-switch'));

    expect(screen.getByTestId('switcher-many')).toHaveTextContent(
      '同时连着 5 台，会更耗电',
    );
  });

  it('back closes the switcher first', async () => {
    const { core, navigation } = await mount();
    await stream(core);
    await fireEvent.press(screen.getByTestId('adapter-tb-switch'));

    const body = screen.getByTestId('adapter-session-body');
    await act(async () => body.props.onAccessibilityEscape());
    expect(screen.queryByTestId('session-switcher')).toBeNull();
    expect(navigation.navigate).not.toHaveBeenCalled();
  });
});
