import { act, renderHook } from '@testing-library/react-native';
import { Platform } from 'react-native';
import type { RemoteSessionHandle } from '../../native/session/RemoteSessionView';
import {
  SLIDE_FALLBACK_MS,
  SNAPSHOT_WAIT_MS,
  SPACE_FADE_MS,
  SPACE_HINT_MS,
  markArrival,
  resetArrival,
} from './spaces';
import { buildSwitcherList } from './switcher';
import type { SwitcherItem } from './switcher';
import { useSpaces } from './useSpaces';
import type { SpacesOptions } from './useSpaces';

function screen(id: string, startedAtUs: number): SwitcherItem {
  const base = {
    id,
    name: `PC ${id}`,
    status: 'connected' as const,
    startedAtUs,
  };
  if (id === 'h') {
    return { ...base, kind: 'host', hostId: 'host-h' };
  }
  return {
    ...base,
    kind: 'rdp',
    params: { target: { protocol: 'rdp', addr: '10.0.0.9' }, name: base.name },
  };
}

const left = screen('h', 10);
const middle = screen('m', 20);
const right = screen('r', 30);
const list = buildSwitcherList([right, left, middle]);

function handle(): RemoteSessionHandle & { animateSpace: jest.Mock } {
  return {
    zoomReset: jest.fn(),
    centerCursor: jest.fn(),
    showMagnifier: jest.fn(),
    animateSpace: jest.fn(),
  };
}

async function mount(overrides: Partial<SpacesOptions> = {}) {
  const switchTo = jest.fn();
  const video = handle();
  const options: SpacesOptions = {
    list,
    currentId: 'm',
    routeSessionId: undefined,
    framed: true,
    videoRef: { current: video },
    switchTo,
    ...overrides,
  };
  const view = await renderHook((props: SpacesOptions) => useSpaces(props), {
    initialProps: options,
  });
  return { ...view, switchTo, video, options };
}

async function advance(ms: number) {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
}

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(() => {
  resetArrival();
  jest.useRealTimers();
});

it('lines up the screens for the native view', async () => {
  const { result } = await mount();
  expect(result.current.spaces).toEqual([
    { id: 'h', name: 'PC h' },
    { id: 'm', name: 'PC m' },
    { id: 'r', name: 'PC r' },
  ]);
  expect(result.current.indicator).toBeNull();
  expect(result.current.showSnapshot).toBe(false);
});

it('shows the dots while swiping and fades them 1.5 s after', async () => {
  const { result } = await mount();

  await act(async () => result.current.onSpaceSwipe({ phase: 'began' }));
  expect(result.current.indicator).toEqual({ highlightId: 'm', fading: false });
  // Held for as long as the fingers are down.
  await advance(SPACE_HINT_MS * 2);
  expect(result.current.indicator?.fading).toBe(false);

  await act(async () => result.current.onSpaceSwipe({ phase: 'ended' }));
  await advance(SPACE_HINT_MS - 1);
  expect(result.current.indicator?.fading).toBe(false);
  await advance(1);
  expect(result.current.indicator?.fading).toBe(true);
  await advance(SPACE_FADE_MS);
  expect(result.current.indicator).toBeNull();
});

it('switches once the native slide asks, and only once', async () => {
  const { result, switchTo } = await mount();

  await act(async () => result.current.onSpaceSwitch({ sessionId: 'r' }));
  await act(async () => result.current.onSpaceSwitch({ sessionId: 'h' }));
  expect(switchTo).toHaveBeenCalledTimes(1);
  expect(switchTo).toHaveBeenCalledWith(right);
});

it('ignores a switch to a session that ended or is already shown', async () => {
  const { result, switchTo } = await mount();

  await act(async () => result.current.onSpaceSwitch({ sessionId: 'gone' }));
  await act(async () => result.current.onSpaceSwitch({ sessionId: 'm' }));
  expect(switchTo).not.toHaveBeenCalled();
});

it('⌃⌥⌘ keys play the same slide toward the target, then switch', async () => {
  const { result, switchTo, video } = await mount();

  await act(async () => result.current.slideTo(left));
  expect(video.animateSpace).toHaveBeenCalledWith('h', 'previous');
  expect(result.current.indicator).toEqual({ highlightId: 'm', fading: false });
  expect(switchTo).not.toHaveBeenCalled();
  // The native view finishes the slide.
  await act(async () => result.current.onSpaceSwitch({ sessionId: 'h' }));
  expect(switchTo).toHaveBeenCalledWith(left);
  // The fallback does not switch a second time.
  await advance(SLIDE_FALLBACK_MS);
  expect(switchTo).toHaveBeenCalledTimes(1);
});

it('a numbered jump slides the way the target lies', async () => {
  const { result, video } = await mount({ currentId: 'h' });
  await act(async () => result.current.slideTo(right));
  expect(video.animateSpace).toHaveBeenCalledWith('r', 'next');
});

it('switches anyway when the slide never reports back', async () => {
  const { result, switchTo } = await mount();

  await act(async () => result.current.slideTo(right));
  await advance(SLIDE_FALLBACK_MS - 1);
  expect(switchTo).not.toHaveBeenCalled();
  await advance(1);
  expect(switchTo).toHaveBeenCalledWith(right);
});

it('switches straight away without a picture to slide', async () => {
  const { result, switchTo } = await mount({ videoRef: { current: null } });
  await act(async () => result.current.slideTo(right));
  expect(switchTo).toHaveBeenCalledWith(right);
});

it('a switched-to page opens on the last picture until its first frame', async () => {
  markArrival('m');
  const view = await mount({ routeSessionId: 'm', framed: false });
  expect(view.result.current.showSnapshot).toBe(true);
  expect(view.result.current.indicator).toEqual({
    highlightId: 'm',
    fading: false,
  });

  await view.rerender({ ...view.options, framed: true });
  expect(view.result.current.showSnapshot).toBe(false);
  // The dots still fade on time.
  await advance(SPACE_HINT_MS + SPACE_FADE_MS);
  expect(view.result.current.indicator).toBeNull();
});

it('stops waiting on the last picture after 3 s', async () => {
  markArrival('m');
  const { result } = await mount({ routeSessionId: 'm', framed: false });
  await advance(SNAPSHOT_WAIT_MS - 1);
  expect(result.current.showSnapshot).toBe(true);
  await advance(1);
  expect(result.current.showSnapshot).toBe(false);
});

it('a page opened any other way shows no snapshot and no dots', async () => {
  markArrival('other');
  const { result } = await mount({ routeSessionId: 'm', framed: false });
  expect(result.current.showSnapshot).toBe(false);
  expect(result.current.indicator).toBeNull();
});

it('uses each switch once', async () => {
  markArrival('m');
  const first = await mount({ routeSessionId: 'm', framed: false });
  expect(first.result.current.showSnapshot).toBe(true);
  await first.unmount();

  const second = await mount({ routeSessionId: 'm', framed: false });
  expect(second.result.current.showSnapshot).toBe(false);
});

it('Android keeps its usual wait but still shows the dots', async () => {
  const os = jest.replaceProperty(Platform, 'OS', 'android');
  markArrival('m');
  const { result } = await mount({ routeSessionId: 'm', framed: false });
  expect(result.current.showSnapshot).toBe(false);
  expect(result.current.indicator).toEqual({ highlightId: 'm', fading: false });
  os.restore();
});
