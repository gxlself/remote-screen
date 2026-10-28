import { act, cleanup, fireEvent, screen } from '@testing-library/react-native';
import { Platform, Pressable, Text } from 'react-native';
import { adapterLive, resetAdapterLive } from '../features/adapters/liveStore';
import type { CollapsedAdapter } from '../features/adapters/liveStore';
import { useLockState } from '../features/lock/lockState';
import {
  PICKER_PAYLOAD,
  encodeWindowTarget,
} from '../features/session/windowTarget';
import {
  MAIN_WINDOW,
  registerWindowPage,
  resetWindows,
  windows,
} from '../features/session/windows';
import { createFakeCore } from '../native/rscore/fakeCore';
import { liveSession } from '../native/rscore/fixtures';
import type { Spec } from '../native/specs/NativeRsWindow';
import type { LocalDirectorySeed } from '../test/localDirectory';
import { renderWithProviders } from '../test/render';
import { PublishSharedTree, sharedTree } from './window/sharedTree';
import WindowRoot from './WindowRoot';

const mockOpen = jest.fn<void, [string, string, string]>();
const mockActivate = jest.fn<void, [string]>();
const mockClose = jest.fn<void, [string]>();
let mockNativeModule: Spec | null = null;

jest.mock('../native/specs/NativeRsWindow', () => ({
  __esModule: true,
  get default() {
    return mockNativeModule;
  },
}));

type FakeScreenProps = {
  route: { name: string; params: Record<string, unknown> };
  navigation: { goBack(): void };
};

function MockScreen({ route, navigation }: FakeScreenProps) {
  return (
    <Pressable onPress={() => navigation.goBack()} testID="fake-back">
      <Text>{`${route.name}:${String(
        route.params.sessionId ?? route.params.hostId,
      )}`}</Text>
    </Pressable>
  );
}

// The window's job is to open the page and close after it; the pages have
// their own tests.
jest.mock('../screens/Session', () => ({
  __esModule: true,
  default: MockScreen,
}));
jest.mock('../features/adapters/AdapterSessionScreen', () => ({
  __esModule: true,
  default: MockScreen,
}));
jest.mock('../screens/SessionEnded', () => ({
  __esModule: true,
  default: MockScreen,
}));

const hostPayload = encodeWindowTarget({
  kind: 'host',
  sessionId: liveSession.id,
  hostId: liveSession.hostId,
  name: 'Studio Mac mini',
});

beforeEach(() => {
  jest.replaceProperty(Platform, 'OS', 'ios');
  mockOpen.mockReset();
  mockActivate.mockReset();
  mockClose.mockReset();
  mockNativeModule = {
    getConstants: () => ({ supported: true }),
    open: mockOpen,
    activate: mockActivate,
    close: mockClose,
    refresh: jest.fn(),
    onWindowState: jest.fn(() => ({ remove: jest.fn() })),
  };
  resetWindows();
  resetAdapterLive();
  sharedTree.setState({ tree: null });
  useLockState.setState({ locked: false });
});

afterEach(async () => {
  await cleanup();
  mockNativeModule = null;
  jest.restoreAllMocks();
});

async function mountWindow(
  payload: unknown,
  windowId: unknown = 'w1',
  localDirectory?: LocalDirectorySeed,
) {
  const core = createFakeCore({ sessions: [liveSession] });
  await renderWithProviders(
    <>
      <PublishSharedTree />
      <WindowRoot payload={payload} windowId={windowId} />
    </>,
    { core, ...(localDirectory === undefined ? {} : { localDirectory }) },
  );
  await act(async () => {
    await Promise.resolve();
  });
  const root = screen.queryByTestId('window-root-w1');
  if (root !== null) {
    await act(async () => {
      fireEvent(root, 'layout', {
        nativeEvent: { layout: { x: 0, y: 0, width: 600, height: 800 } },
      });
    });
  }
  return core;
}

it('opens its session page full screen', async () => {
  await mountWindow(hostPayload);
  expect(screen.getByText(`Session:${liveSession.id}`)).toBeOnTheScreen();
  expect(mockClose).not.toHaveBeenCalled();
});

it('waits for its own size before the page opens its connection', async () => {
  const core = createFakeCore({ sessions: [liveSession] });
  await renderWithProviders(
    <>
      <PublishSharedTree />
      <WindowRoot payload={hostPayload} windowId="w1" />
    </>,
    { core },
  );
  await act(async () => {
    await Promise.resolve();
  });
  expect(screen.queryByText(`Session:${liveSession.id}`)).toBeNull();
  expect(screen.getByTestId('window-waiting')).toBeOnTheScreen();
});

it('closes and shows the main window when the page is left', async () => {
  await mountWindow(hostPayload);
  await act(async () => {
    await fireEvent.press(screen.getByTestId('fake-back'));
  });
  expect(mockActivate).toHaveBeenCalledWith(MAIN_WINDOW);
  expect(mockClose).toHaveBeenCalledWith('w1');
});

it('opens a folded-away adapter session on its page', async () => {
  adapterLive.put({
    sessionId: 'v-live',
    params: { target: { protocol: 'vnc', addr: '100.64.0.2' } },
    name: 'Mac',
    phase: 'streaming',
  } as CollapsedAdapter);
  await mountWindow(
    encodeWindowTarget({
      kind: 'vnc',
      sessionId: 'v-live',
      name: 'Mac',
      params: { target: { protocol: 'vnc', addr: '100.64.0.2' } },
    }),
  );
  expect(screen.getByText('AdapterSession:v-live')).toBeOnTheScreen();
  expect(mockClose).not.toHaveBeenCalled();
});

it('closes an adapter window with nothing folded away', async () => {
  await mountWindow(
    encodeWindowTarget({
      kind: 'vnc',
      sessionId: 'v-gone',
      name: 'Mac',
      params: { target: { protocol: 'vnc', addr: '100.64.0.2' } },
    }),
  );
  // Nothing folded away under that id (a window brought back after a
  // relaunch): the window closes instead of opening an empty page.
  expect(screen.queryByText('AdapterSession:v-gone')).toBeNull();
  expect(mockClose).toHaveBeenCalledWith('w1');
});

it('closes a restored window whose session is gone', async () => {
  await mountWindow(
    encodeWindowTarget({
      kind: 'host',
      sessionId: 's-gone',
      hostId: liveSession.hostId,
      name: 'Studio Mac mini',
    }),
  );
  expect(screen.queryByText('Session:s-gone')).toBeNull();
  expect(mockActivate).toHaveBeenCalledWith(MAIN_WINDOW);
  expect(mockClose).toHaveBeenCalledWith('w1');
});

it('closes a window with a payload it cannot read', async () => {
  await mountWindow('{broken');
  expect(mockClose).toHaveBeenCalledWith('w1');
});

it('covers the window while the app is locked', async () => {
  await mountWindow(hostPayload);
  await act(async () => useLockState.setState({ locked: true }));
  expect(screen.getByTestId('window-locked')).toBeOnTheScreen();
  await act(async () => {
    await fireEvent.press(screen.getByTestId('window-locked-main'));
  });
  expect(mockActivate).toHaveBeenCalledWith(MAIN_WINDOW);
  await act(async () => useLockState.setState({ locked: false }));
  expect(screen.queryByTestId('window-locked')).toBeNull();
});

describe('选电脑 window (a window iPadOS opened by itself)', () => {
  const STUDIO_ID = liveSession.hostId;
  const OFFICE_ID = 'w3k8p2x7m4q9tb6c';

  async function settle() {
    for (let turn = 0; turn < 5; turn += 1) {
      await act(async () => {
        await new Promise(resolve => setTimeout(resolve, 0));
      });
    }
  }

  async function mountPicker() {
    const core = await mountWindow(PICKER_PAYLOAD, 'w1', {});
    await settle();
    return core;
  }

  it('shows the computer list and stays open', async () => {
    await mountPicker();
    expect(screen.getByTestId(`card-${STUDIO_ID}`)).toBeOnTheScreen();
    expect(screen.getByTestId(`card-${OFFICE_ID}`)).toBeOnTheScreen();
    expect(screen.queryByTestId('window-exit')).toBeNull();
    expect(mockClose).not.toHaveBeenCalled();
  });

  it('opens a computer here, full screen, and comes back to the list', async () => {
    await mountPicker();
    await fireEvent.press(screen.getByTestId(`card-${OFFICE_ID}`));
    await settle();
    expect(screen.getByText(`Session:${OFFICE_ID}`)).toBeOnTheScreen();

    await fireEvent.press(screen.getByTestId('fake-back'));
    await settle();
    expect(screen.queryByText(`Session:${OFFICE_ID}`)).toBeNull();
    expect(screen.getByTestId(`card-${OFFICE_ID}`)).toBeOnTheScreen();
    expect(mockClose).not.toHaveBeenCalled();
    expect(mockActivate).not.toHaveBeenCalled();
  });

  it('opens a live session here and comes back to the list when it ends', async () => {
    await mountPicker();
    await fireEvent.press(screen.getByTestId(`card-${STUDIO_ID}`));
    await settle();
    expect(screen.getByText(`Session:${liveSession.id}`)).toBeOnTheScreen();
    await fireEvent.press(screen.getByTestId('fake-back'));
    await settle();
    expect(screen.getByTestId(`card-${STUDIO_ID}`)).toBeOnTheScreen();
    expect(mockClose).not.toHaveBeenCalled();
  });

  it('brings forward the window that already shows the computer', async () => {
    await mountPicker();
    await act(async () =>
      registerWindowPage('elsewhere', 'w9', {
        kind: 'host',
        id: liveSession.id,
        hostId: STUDIO_ID,
        name: 'Studio Mac mini',
        status: 'connected',
        startedAtUs: 1,
      }),
    );
    await fireEvent.press(screen.getByTestId(`card-${STUDIO_ID}`));
    await settle();
    expect(mockActivate).toHaveBeenCalledWith('w9');
    expect(screen.queryByText(`Session:${liveSession.id}`)).toBeNull();
    expect(
      Object.values(windows.getState().pages).map(page => page.windowId),
    ).toEqual(['w9']);
  });

  it('hands pages it does not have to the main window', async () => {
    await mountPicker();
    await fireEvent.press(screen.getByLabelText('账号与设置'));
    expect(mockActivate).toHaveBeenCalledWith(MAIN_WINDOW);
    expect(screen.getByTestId(`card-${STUDIO_ID}`)).toBeOnTheScreen();
    expect(mockClose).not.toHaveBeenCalled();
  });
});
