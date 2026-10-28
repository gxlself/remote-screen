import { createRef } from 'react';
import type { ReactNode } from 'react';
import { act, cleanup, render } from '@testing-library/react-native';
import { Platform } from 'react-native';
import { videoStore } from '../../features/session/videoStore';
import {
  SettingsProvider,
  createSettingsStore,
} from '../../features/settings/store';
import type { SettingsStore } from '../../features/settings/store';
import { MemoryKv } from '../../features/storage/kv';
import { Commands } from '../../native/specs/RemoteSessionViewNativeComponent';
import type { NativeProps } from '../../native/specs/RemoteSessionViewNativeComponent';
import type { RemoteSessionHandle } from '../../native/session/RemoteSessionView';
import { VideoSlot } from './VideoSlot';
import type { VideoSlotProps } from './slots';

let mockProps: NativeProps;
const mockNativeHandle = {};
let settingsStore: SettingsStore;

function SettingsWrapper({ children }: { children: ReactNode }) {
  return <SettingsProvider store={settingsStore}>{children}</SettingsProvider>;
}

jest.mock('../../native/specs/RemoteSessionViewNativeComponent', () => {
  const React = require('react');
  const { View } = require('react-native');
  return {
    __esModule: true,
    default: function MockNativeSession(
      props: NativeProps & { ref?: React.Ref<unknown> },
    ) {
      mockProps = props;
      React.useImperativeHandle(props.ref, () => mockNativeHandle);
      return <View testID="native-session" />;
    },
    Commands: {
      zoomReset: jest.fn(),
      centerCursor: jest.fn(),
      showMagnifier: jest.fn(),
    },
  };
});

const props: VideoSlotProps = {
  sessionId: null,
  mode: 'touch',
  reduceMotion: true,
  toolbarCollapsed: true,
  keyboardInset: 300,
  accessibilityLabel: 'Studio Mac mini 远端画面，双指双击切换触控板',
};
const stats = {
  fps: 60,
  decodeP50Ms: 1,
  decodeP95Ms: 2,
  latencyP50Ms: -1,
  queue: 0,
  dropped: 0,
  keyframeRequests: 0,
  width: 1920,
  height: 1080,
  codec: 'hevc',
};
const viewport = { zoom: 2, x: 0.25, y: 0.5 };

beforeEach(() => {
  jest.clearAllMocks();
  jest.replaceProperty(Platform, 'OS', 'ios');
  videoStore.setState({ stats: {}, viewport: {} });
  settingsStore = createSettingsStore(new MemoryKv());
});

afterEach(async () => {
  await cleanup();
  jest.restoreAllMocks();
});

it('passes display and accessibility props through to the iOS native view', async () => {
  const view = await render(<VideoSlot {...props} />, {
    wrapper: SettingsWrapper,
  });
  expect(view.getByTestId('native-session')).toBeOnTheScreen();
  expect(mockProps.sessionId).toBe('');
  expect(mockProps.mode).toBe(props.mode);
  expect(mockProps.reduceMotion).toBe(true);
  expect(mockProps.toolbarCollapsed).toBe(true);
  expect(mockProps.keyboardInset).toBe(300);
  expect(mockProps.cursorSpeed).toBe(1.4);
  expect(mockProps.naturalScroll).toBe(true);
  expect(mockProps.haptics).toBe(true);
  expect(mockProps.accessible).toBe(true);
  expect(mockProps.accessibilityRole).toBe('image');
  expect(mockProps.accessibilityLabel).toBe(props.accessibilityLabel);
});

it('passes persisted gesture preferences to the native view', async () => {
  const kv = new MemoryKv();
  kv.setJSON('settings.session.cursorSpeed', 2.0);
  kv.setJSON('settings.session.naturalScroll', false);
  kv.setJSON('settings.session.haptics', false);
  settingsStore = createSettingsStore(kv);
  await render(<VideoSlot {...props} />, { wrapper: SettingsWrapper });

  expect(mockProps.cursorSpeed).toBe(2.0);
  expect(mockProps.naturalScroll).toBe(false);
  expect(mockProps.haptics).toBe(false);
});

it('updates native gesture preferences while the same session is attached', async () => {
  await render(<VideoSlot {...props} sessionId="123" />, {
    wrapper: SettingsWrapper,
  });
  // Match SessionDefaultsSection's numeric setter for the schema's literal default type.
  const setCursorSpeed = settingsStore.set as (
    key: 'session.cursorSpeed',
    value: number,
  ) => void;

  await act(() => {
    setCursorSpeed('session.cursorSpeed', 2.0);
    settingsStore.set('session.naturalScroll', false);
    settingsStore.set('session.haptics', false);
  });

  expect(mockProps.sessionId).toBe('123');
  expect(mockProps.cursorSpeed).toBe(2.0);
  expect(mockProps.naturalScroll).toBe(false);
  expect(mockProps.haptics).toBe(false);

  await act(() => {
    setCursorSpeed('session.cursorSpeed', 1.4);
    settingsStore.set('session.naturalScroll', true);
    settingsStore.set('session.haptics', true);
  });

  expect(mockProps.sessionId).toBe('123');
  expect(mockProps.cursorSpeed).toBe(1.4);
  expect(mockProps.naturalScroll).toBe(true);
  expect(mockProps.haptics).toBe(true);
});

it('stores native viewport and video stats and forwards page events', async () => {
  const onGesture = jest.fn();
  const onFirstFrame = jest.fn();
  await render(
    <VideoSlot {...props} sessionId="123" {...{ onGesture, onFirstFrame }} />,
    { wrapper: SettingsWrapper },
  );
  expect(mockProps.sessionId).toBe('123');
  await act(() => {
    mockProps.onViewport?.({ nativeEvent: viewport } as never);
    mockProps.onVideoStats?.({ nativeEvent: stats } as never);
    mockProps.onGesture?.({
      nativeEvent: { kind: 'twoFingerDoubleTap' },
    } as never);
    mockProps.onFirstFrame?.({
      nativeEvent: { epoch: 1, width: 1920, height: 1080 },
    } as never);
  });
  expect(videoStore.getState()).toEqual({
    stats: { '123': stats },
    viewport: { '123': viewport },
  });
  expect(onGesture).toHaveBeenCalledWith({ kind: 'twoFingerDoubleTap' });
  expect(onFirstFrame).toHaveBeenCalledTimes(1);
});

it('ignores native data before a session is attached', async () => {
  await render(<VideoSlot {...props} />, { wrapper: SettingsWrapper });
  await act(() => {
    mockProps.onViewport?.({ nativeEvent: viewport } as never);
    mockProps.onVideoStats?.({ nativeEvent: stats } as never);
  });
  expect(videoStore.getState()).toEqual({ stats: {}, viewport: {} });
});

it('forwards all view commands to the native ref', async () => {
  const ref = createRef<RemoteSessionHandle>();
  await render(<VideoSlot {...props} ref={ref} />, { wrapper: SettingsWrapper });
  expect(ref.current).not.toBeNull();
  ref.current?.zoomReset();
  ref.current?.centerCursor();
  ref.current?.showMagnifier(true);
  expect(Commands.zoomReset).toHaveBeenCalledWith(mockNativeHandle);
  expect(Commands.centerCursor).toHaveBeenCalledWith(mockNativeHandle);
  expect(Commands.showMagnifier).toHaveBeenCalledWith(mockNativeHandle, true);
});

it('renders the native component on Android and forwards events', async () => {
  jest.replaceProperty(Platform, 'OS', 'android');
  const onGesture = jest.fn();
  const onFirstFrame = jest.fn();
  const view = await render(
    <VideoSlot
      {...props}
      sessionId="123"
      onGesture={onGesture}
      onFirstFrame={onFirstFrame}
    />,
    { wrapper: SettingsWrapper },
  );
  expect(view.getByTestId('native-session')).toBeOnTheScreen();
  expect(mockProps.sessionId).toBe('123');
  expect(mockProps.mode).toBe(props.mode);
  expect(mockProps.accessibilityLabel).toBe(props.accessibilityLabel);
  await act(() => {
    mockProps.onViewport?.({ nativeEvent: viewport } as never);
    mockProps.onVideoStats?.({ nativeEvent: stats } as never);
    mockProps.onGesture?.({
      nativeEvent: { kind: 'twoFingerDoubleTap' },
    } as never);
    mockProps.onFirstFrame?.({
      nativeEvent: { epoch: 1, width: 1920, height: 1080 },
    } as never);
  });
  expect(videoStore.getState()).toEqual({
    stats: { '123': stats },
    viewport: { '123': viewport },
  });
  expect(onGesture).toHaveBeenCalledWith({ kind: 'twoFingerDoubleTap' });
  expect(onFirstFrame).toHaveBeenCalledTimes(1);
});

it('keeps the black accessible placeholder on other platforms', async () => {
  jest.replaceProperty(Platform, 'OS', 'windows');
  const view = await render(<VideoSlot {...props} />, {
    wrapper: SettingsWrapper,
  });
  expect(view.queryByTestId('native-session')).toBeNull();
  const placeholder = view.getByTestId('remote-session-view-placeholder');
  expect(placeholder).toHaveStyle({ backgroundColor: '#000' });
  expect(placeholder).toHaveProp('accessible', true);
  expect(placeholder).toHaveProp('accessibilityRole', 'image');
  expect(placeholder).toHaveProp(
    'accessibilityLabel',
    props.accessibilityLabel,
  );
});
