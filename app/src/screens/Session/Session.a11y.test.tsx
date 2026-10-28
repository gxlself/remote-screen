import { act, cleanup, fireEvent } from '@testing-library/react-native';
import { AccessibilityInfo, Platform } from 'react-native';
import type { QueryClient } from '@tanstack/react-query';
import type { RootStackParamList } from '../../app/navigation/routes';
import type { SessionKeyboardProps } from '../../native/keyboard/SessionKeyboard';
import { createFakeCore } from '../../native/rscore/fakeCore';
import type { FakeCore } from '../../native/rscore/fakeCore';
import { liveSession } from '../../native/rscore/fixtures';
import { renderWithProviders } from '../../test/render';
import { mockNavigation, mockRoute } from '../../test/navigation';
import SessionScreen from './index';

jest.mock('../../native/session/RemoteSessionView', () =>
  require('../../test/mocks/RemoteSessionViewMock'),
);

const mockKeyboardProps = jest.fn<void, [SessionKeyboardProps]>();

jest.mock('../../native/keyboard/SessionKeyboard', () => {
  const React = jest.requireActual<typeof import('react')>('react');
  const { View } =
    jest.requireActual<typeof import('react-native')>('react-native');
  function SessionKeyboard(props: SessionKeyboardProps) {
    mockKeyboardProps(props);
    return React.createElement(View, { testID: 'session-keyboard-mock' });
  }
  return { __esModule: true, default: SessionKeyboard, SessionKeyboard };
});

const hostId = 'h7v2m9q3k8x4tb1a';
const clients: QueryClient[] = [];
type Options = NonNullable<Parameters<typeof renderWithProviders>[1]>;

async function advance(ms = 1) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

async function mount(
  params: Partial<RootStackParamList['Session']> = {},
  opts: Options = {},
) {
  const core = opts.core ?? createFakeCore({ sessions: [] });
  const navigation = mockNavigation<'Session'>();
  const route = mockRoute('Session', { hostId, ...params });
  const view = await renderWithProviders(
    <SessionScreen navigation={navigation} route={route} />,
    { ...opts, core },
  );
  clients.push(view.queryClient);
  await advance();
  return { ...view, navigation };
}

async function stream(core: FakeCore, view: Awaited<ReturnType<typeof mount>>) {
  await act(() =>
    core.emit({
      type: 'session.state',
      sessionId: 's1',
      hostId,
      phase: 'streaming',
    }),
  );
  await fireEvent(
    view.getByTestId('remote-session-view-placeholder', {
      includeHiddenElements: true,
    }),
    'onFirstFrame',
  );
}

beforeEach(() => {
  jest.useFakeTimers();
  mockKeyboardProps.mockClear();
  jest
    .spyOn(AccessibilityInfo, 'isScreenReaderEnabled')
    .mockResolvedValue(false);
});

afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('keeps the toolbar visible for a screen reader after a three-finger gesture', async () => {
  jest.mocked(AccessibilityInfo.isScreenReaderEnabled).mockResolvedValue(true);
  const view = await mount({}, { settings: { 'session.coachShown': true } });
  await stream(view.core, view);

  await fireEvent(
    view.getByTestId('remote-session-view-placeholder'),
    'gesture',
    { kind: 'threeFingerTap' },
  );
  expect(view.getByTestId('layer-toolbar')).toBeOnTheScreen();
});

it('still hides the toolbar after the gesture when the screen reader is off', async () => {
  const view = await mount({}, { settings: { 'session.coachShown': true } });
  await stream(view.core, view);

  await fireEvent(
    view.getByTestId('remote-session-view-placeholder'),
    'gesture',
    { kind: 'threeFingerTap' },
  );
  expect(view.queryByTestId('layer-toolbar')).toBeNull();
});

it('uses Magic Tap for mode switching and ignores it while a sheet is open', async () => {
  const view = await mount({}, { settings: { 'session.coachShown': true } });
  await stream(view.core, view);
  const root = view.getByTestId('session-root');

  await fireEvent(root, 'magicTap');
  expect(
    view.getByLabelText('Studio Mac mini 远端画面，双指双击切换触控板'),
  ).toBeOnTheScreen();

  await fireEvent.press(view.getByLabelText('更多'));
  await fireEvent(root, 'magicTap');
  expect(
    view.queryByLabelText('Studio Mac mini 远端画面，双指双击切换直接触控'),
  ).toBeNull();
});

it('closes the shell layers in accessibility escape order', async () => {
  const view = await mount({}, { settings: { 'session.coachShown': true } });
  await stream(view.core, view);
  const root = view.getByTestId('session-root');

  await fireEvent.press(view.getByLabelText('更多'));
  await fireEvent(root, 'accessibilityEscape');
  expect(view.queryByTestId('layer-sheet')).toBeNull();

  await fireEvent.press(view.getByLabelText('打开键盘'));
  await fireEvent(root, 'accessibilityEscape');
  expect(mockKeyboardProps.mock.lastCall?.[0].open).toBe(false);

  await fireEvent(root, 'accessibilityEscape');
  expect(view.navigation.popTo).toHaveBeenCalledWith('MainTabs', {
    screen: 'Sessions',
  });
});

it('does not use the escape gesture for reconnecting or idle warnings', async () => {
  const reconnecting = await mount(
    {},
    {
      core: createFakeCore({
        sessions: [{ ...liveSession, phase: 'reconnecting' }],
      }),
    },
  );
  await fireEvent(
    reconnecting.getByTestId('session-root'),
    'accessibilityEscape',
  );
  expect(reconnecting.navigation.navigate).not.toHaveBeenCalled();
});

it('hides the layers behind the active sheet for screen readers', async () => {
  jest.mocked(AccessibilityInfo.isScreenReaderEnabled).mockResolvedValue(true);
  const view = await mount({}, { settings: { 'session.coachShown': true } });
  await stream(view.core, view);
  await fireEvent.press(view.getByLabelText('更多'));

  for (const testID of [
    'layer-video',
    'layer-hud',
    'layer-toolbar',
    'layer-keyboard',
  ]) {
    expect(
      view.getByTestId(testID, { includeHiddenElements: true }),
    ).toHaveProp('accessibilityElementsHidden', true);
    expect(
      view.getByTestId(testID, { includeHiddenElements: true }),
    ).toHaveProp('importantForAccessibility', 'no-hide-descendants');
  }
  expect(view.getByTestId('layer-sheet')).not.toHaveProp(
    'accessibilityElementsHidden',
  );
});

it('announces the connection once and uses Android-specific video labels', async () => {
  const announce = jest
    .spyOn(AccessibilityInfo, 'announceForAccessibility')
    .mockClear();
  const view = await mount({}, { settings: { 'session.coachShown': true } });
  await stream(view.core, view);
  expect(announce).toHaveBeenCalledWith('已连接 Studio Mac mini');
  const calls = announce.mock.calls.length;
  await act(() =>
    view.core.emit({
      type: 'session.stats',
      sessionId: 's1',
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
    }),
  );
  expect(announce).toHaveBeenCalledTimes(calls);

  jest.replaceProperty(Platform, 'OS', 'android');
  const android = await mount({}, { settings: { 'session.coachShown': true } });
  await stream(android.core, android);
  expect(
    android.getByLabelText('Studio Mac mini 远端画面，触控板模式'),
  ).toBeOnTheScreen();
});
