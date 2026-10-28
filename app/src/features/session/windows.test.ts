import { Platform } from 'react-native';
import type { Spec } from '../../native/specs/NativeRsWindow';
import type { SwitcherItem } from './switcher';
import {
  PICKER_PAYLOAD,
  decodeWindowPayload,
  decodeWindowTarget,
  encodeWindowTarget,
  windowRoute,
  windowTargetOf,
} from './windowTarget';
import {
  MAIN_WINDOW,
  focusOtherWindow,
  hiddenByWindow,
  leaveWindow,
  openInNewWindow,
  otherWindowShowing,
  planWindowPauses,
  registerWindowPage,
  releaseWindowPage,
  resetWindows,
  setWindowPhase,
  shownInOtherWindows,
  windows,
} from './windows';

const mockOpen = jest.fn<void, [string, string, string]>();
const mockActivate = jest.fn<void, [string]>();
const mockClose = jest.fn<void, [string]>();
let mockSupported = true;
let mockNativeModule: Spec | null = null;

jest.mock('../../native/specs/NativeRsWindow', () => ({
  __esModule: true,
  get default() {
    return mockNativeModule;
  },
}));

const mac: SwitcherItem = {
  kind: 'host',
  id: 's1',
  hostId: 'h1',
  name: 'Studio Mac mini',
  status: 'connected',
  startedAtUs: 100,
};
const pc: SwitcherItem = {
  kind: 'rdp',
  id: 'a2',
  params: {
    target: { protocol: 'rdp', addr: '100.64.0.9' },
    name: 'Win PC',
    switchSeq: 3,
  },
  name: 'Win PC',
  status: 'connected',
  startedAtUs: 200,
};
const nas: SwitcherItem = {
  kind: 'ssh',
  id: 'x3',
  serverId: 'srv',
  name: 'NAS',
  status: 'connected',
  startedAtUs: 50,
};

beforeEach(() => {
  jest.replaceProperty(Platform, 'OS', 'ios');
  mockSupported = true;
  mockOpen.mockReset();
  mockActivate.mockReset();
  mockClose.mockReset();
  mockNativeModule = {
    getConstants: () => ({ supported: mockSupported }),
    open: mockOpen,
    activate: mockActivate,
    close: mockClose,
    refresh: jest.fn(),
    onWindowState: jest.fn(() => ({ remove: jest.fn() })),
  };
  resetWindows();
});

afterEach(() => {
  mockNativeModule = null;
  jest.useRealTimers();
  jest.restoreAllMocks();
});

describe('window targets', () => {
  it('round-trips host and adapter sessions, without the switch sequence', () => {
    const host = windowTargetOf(mac);
    expect(host).toEqual({
      kind: 'host',
      sessionId: 's1',
      hostId: 'h1',
      name: 'Studio Mac mini',
    });
    expect(decodeWindowTarget(encodeWindowTarget(host!))).toEqual(host);
    const adapter = windowTargetOf(pc);
    expect(adapter).toEqual({
      kind: 'rdp',
      sessionId: 'a2',
      name: 'Win PC',
      params: { target: { protocol: 'rdp', addr: '100.64.0.9' }, name: 'Win PC' },
    });
    expect(decodeWindowTarget(encodeWindowTarget(adapter!))).toEqual(adapter);
  });

  it('keeps terminals in the main window and rejects bad payloads', () => {
    expect(windowTargetOf(nas)).toBeNull();
    expect(decodeWindowTarget(undefined)).toBeNull();
    expect(decodeWindowTarget('not json')).toBeNull();
    expect(decodeWindowTarget('{"kind":"host","sessionId":"s1"}')).toBeNull();
    expect(
      decodeWindowTarget(
        '{"kind":"vnc","sessionId":"v1","params":{"target":{"protocol":"rdp","addr":"x"}}}',
      ),
    ).toBeNull();
  });

  it('reads the 选电脑 payload the native side writes', () => {
    expect(decodeWindowPayload(PICKER_PAYLOAD)).toEqual({ kind: 'picker' });
    expect(decodeWindowPayload('{ "kind": "picker" }')).toEqual({
      kind: 'picker',
    });
    const host = windowTargetOf(mac)!;
    expect(decodeWindowPayload(encodeWindowTarget(host))).toEqual(host);
    expect(decodeWindowPayload('{broken')).toBeNull();
    expect(decodeWindowPayload(7)).toBeNull();
    // A session payload never reads as the list.
    expect(decodeWindowTarget(PICKER_PAYLOAD)).toBeNull();
  });

  it('opens the session page the target names', () => {
    expect(windowRoute(windowTargetOf(mac)!)).toEqual({
      name: 'Session',
      params: { hostId: 'h1', sessionId: 's1' },
    });
    expect(windowRoute(windowTargetOf(pc)!)).toEqual({
      name: 'AdapterSession',
      params: {
        target: { protocol: 'rdp', addr: '100.64.0.9' },
        name: 'Win PC',
        sessionId: 'a2',
      },
    });
  });
});

describe('在新窗口打开', () => {
  it('asks the native side for a window with the session as payload', () => {
    expect(openInNewWindow(mac, MAIN_WINDOW)).toBe('opened');
    expect(mockOpen).toHaveBeenCalledTimes(1);
    const [windowId, payload, title] = mockOpen.mock.calls[0]!;
    expect(windowId).toMatch(/^[A-Za-z0-9_-]+$/);
    expect(windowId).not.toBe(MAIN_WINDOW);
    expect(decodeWindowTarget(payload)).toEqual(windowTargetOf(mac));
    expect(title).toBe('Studio Mac mini');
    // Until its page is up, the session counts as shown there.
    expect(otherWindowShowing(windows.getState(), MAIN_WINDOW, 's1')).toBe(
      windowId,
    );
  });

  it('brings the window forward when the session is shown elsewhere', () => {
    registerWindowPage('p1', 'w-other', mac);
    expect(openInNewWindow(mac, MAIN_WINDOW)).toBe('activated');
    expect(mockActivate).toHaveBeenCalledWith('w-other');
    expect(mockOpen).not.toHaveBeenCalled();
  });

  it('moves the current session: the window opens once its page let it go', () => {
    jest.useFakeTimers();
    registerWindowPage('p1', MAIN_WINDOW, mac);
    expect(openInNewWindow(mac, MAIN_WINDOW)).toBe('moving');
    jest.runAllTimers();
    expect(mockOpen).not.toHaveBeenCalled();

    releaseWindowPage('p1');
    expect(mockOpen).not.toHaveBeenCalled();
    jest.runAllTimers();
    expect(mockOpen).toHaveBeenCalledTimes(1);
    expect(decodeWindowTarget(mockOpen.mock.calls[0]![1])).toEqual(
      windowTargetOf(mac),
    );
  });

  it('does nothing without system windows or for terminals', () => {
    expect(openInNewWindow(nas, MAIN_WINDOW)).toBe('unsupported');
    mockSupported = false;
    expect(openInNewWindow(mac, MAIN_WINDOW)).toBe('unsupported');
    expect(mockOpen).not.toHaveBeenCalled();
  });

  it('forgets a window the native side could not open', () => {
    openInNewWindow(mac, MAIN_WINDOW);
    const windowId = mockOpen.mock.calls[0]![0];
    setWindowPhase(windowId, 'closed');
    expect(otherWindowShowing(windows.getState(), MAIN_WINDOW, 's1')).toBeNull();
  });
});

describe('one window per session', () => {
  it('finds the session in another window, by id or by computer', () => {
    registerWindowPage('p1', 'w1', mac);
    registerWindowPage('p2', 'w2', pc);
    const state = windows.getState();
    expect(otherWindowShowing(state, MAIN_WINDOW, 's1')).toBe('w1');
    expect(otherWindowShowing(state, 'w1', 's1')).toBeNull();
    // An adapter page asked for the same computer before it has a session.
    expect(
      otherWindowShowing(state, MAIN_WINDOW, undefined, {
        protocol: 'rdp',
        addr: '100.64.0.9',
      }),
    ).toBe('w2');
    expect(shownInOtherWindows(state, 'w1').map(item => item.id)).toEqual([
      'a2',
    ]);
  });

  it('switching to a session shown elsewhere brings that window forward', () => {
    registerWindowPage('p1', 'w1', mac);
    expect(focusOtherWindow('s1', MAIN_WINDOW)).toBe(true);
    expect(mockActivate).toHaveBeenCalledWith('w1');
    expect(focusOtherWindow('s1', 'w1')).toBe(false);
    expect(focusOtherWindow('a2', MAIN_WINDOW)).toBe(false);
  });

  it('leaving a window closes it and shows the main window', () => {
    leaveWindow('w1');
    expect(mockActivate).toHaveBeenCalledWith(MAIN_WINDOW);
    expect(mockClose).toHaveBeenCalledWith('w1');
    mockClose.mockReset();
    leaveWindow(MAIN_WINDOW);
    expect(mockClose).not.toHaveBeenCalled();
  });
});

describe('window visibility', () => {
  it('pauses only the sessions whose window is off screen', () => {
    registerWindowPage('p1', MAIN_WINDOW, mac);
    registerWindowPage('p2', 'w2', pc);
    setWindowPhase(MAIN_WINDOW, 'active');
    setWindowPhase('w2', 'inactive');
    // Split view: both on screen.
    expect(planWindowPauses(windows.getState(), new Map(), null)).toEqual({
      pause: [],
      resume: [],
      drop: [],
    });
    setWindowPhase('w2', 'background');
    expect(planWindowPauses(windows.getState(), new Map(), null).pause).toEqual(
      [pc],
    );
    expect(hiddenByWindow(windows.getState(), 'a2')).toBe(true);
    expect(hiddenByWindow(windows.getState(), 's1')).toBe(false);
    // Picture in picture keeps it playing.
    expect(planWindowPauses(windows.getState(), new Map(), 'a2').pause).toEqual(
      [],
    );
    setWindowPhase('w2', 'active');
    expect(
      planWindowPauses(windows.getState(), new Map([['a2', 'rdp']]), null)
        .resume,
    ).toEqual(['a2']);
  });

  it('leaves the whole app in the background to the app-wide rule', () => {
    registerWindowPage('p1', MAIN_WINDOW, mac);
    setWindowPhase(MAIN_WINDOW, 'background');
    expect(planWindowPauses(windows.getState(), new Map(), null).pause).toEqual(
      [],
    );
    expect(hiddenByWindow(windows.getState(), 's1')).toBe(false);
  });

  it('resumes a Host session whose page went away and drops an adapter one', () => {
    setWindowPhase(MAIN_WINDOW, 'active');
    const paused = new Map<string, SwitcherItem['kind']>([
      ['s1', 'host'],
      ['a2', 'rdp'],
    ]);
    expect(planWindowPauses(windows.getState(), paused, null)).toEqual({
      pause: [],
      resume: ['s1'],
      drop: ['a2'],
    });
  });

  it('closing the main window only hides it', () => {
    setWindowPhase(MAIN_WINDOW, 'closed');
    setWindowPhase('w1', 'active');
    setWindowPhase('w1', 'closed');
    expect(windows.getState().phases).toEqual({ [MAIN_WINDOW]: 'background' });
    setWindowPhase('w1', 'nonsense');
    expect(windows.getState().phases).toEqual({ [MAIN_WINDOW]: 'background' });
  });
});
