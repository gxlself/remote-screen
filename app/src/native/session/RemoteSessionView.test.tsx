import { createRef } from 'react';
import { act, render } from '@testing-library/react-native';
import { Commands } from '../specs/RemoteSessionViewNativeComponent';
import type { NativeProps } from '../specs/RemoteSessionViewNativeComponent';
import { pointerLockStore } from './pointerLockStore';
import { RemoteSessionView } from './RemoteSessionView';
import type {
  RemoteSessionHandle,
  RemoteSessionViewProps,
} from './RemoteSessionView';

let mockProps: NativeProps;
const mockNativeHandle = {};

jest.mock('../specs/RemoteSessionViewNativeComponent', () => {
  const React = require('react');
  const { View } = require('react-native');
  return {
    __esModule: true,
    default: function MockNativeSession(
      p: NativeProps & { ref?: React.Ref<unknown> },
    ) {
      mockProps = p;
      React.useImperativeHandle(p.ref, () => mockNativeHandle);
      return <View testID="native-session" />;
    },
    Commands: {
      zoomReset: jest.fn(),
      centerCursor: jest.fn(),
      showMagnifier: jest.fn(),
      animateSpace: jest.fn(),
    },
  };
});

const props: RemoteSessionViewProps = {
  sessionId: null,
  mode: 'trackpad',
  reduceMotion: false,
  toolbarCollapsed: true,
  keyboardInset: 0,
  accessibilityLabel: '远端画面',
};

beforeEach(() => {
  jest.clearAllMocks();
  pointerLockStore.setState({ states: {} });
});

it('normalizes an empty session and forwards accessibility and display props', async () => {
  await render(<RemoteSessionView {...props} />);
  expect(mockProps.sessionId).toBe('');
  expect(mockProps.accessible).toBe(true);
  expect(mockProps.accessibilityRole).toBe('image');
  expect(mockProps.accessibilityLabel).toBe('远端画面');
  expect(mockProps.mode).toBe('trackpad');
  expect(mockProps.toolbarCollapsed).toBe(true);
});

it('forwards gesture preferences to the native view', async () => {
  await render(
    <RemoteSessionView
      {...props}
      cursorSpeed={2}
      naturalScroll={false}
      haptics={false}
    />,
  );
  expect(mockProps.cursorSpeed).toBe(2);
  expect(mockProps.naturalScroll).toBe(false);
  expect(mockProps.haptics).toBe(false);
});

it('unwraps native events for page callbacks', async () => {
  const onFirstFrame = jest.fn();
  const onGesture = jest.fn();
  const onViewport = jest.fn();
  const onVideoStats = jest.fn();
  await render(
    <RemoteSessionView
      {...props}
      sessionId="123"
      {...{ onFirstFrame, onGesture, onViewport, onVideoStats }}
    />,
  );
  const gesture = { kind: 'twoFingerDoubleTap' };
  const viewport = { zoom: 2, x: 0.25, y: 0.5 };
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
  await act(async () => {
    mockProps.onFirstFrame?.({
      nativeEvent: { epoch: 3, width: 1920, height: 1080 },
    } as never);
    mockProps.onGesture?.({ nativeEvent: gesture } as never);
    mockProps.onViewport?.({ nativeEvent: viewport } as never);
    mockProps.onVideoStats?.({ nativeEvent: stats } as never);
  });
  expect(onFirstFrame).toHaveBeenCalledWith();
  expect(onGesture).toHaveBeenCalledWith(gesture);
  expect(onViewport).toHaveBeenCalledWith(viewport);
  expect(onVideoStats).toHaveBeenCalledWith(stats);
});

it('normalizes pointer lock events, stores them, and clears the old session', async () => {
  const onPointerLock = jest.fn();
  const view = await render(
    <RemoteSessionView
      {...props}
      sessionId="123"
      onPointerLock={onPointerLock}
    />,
  );
  await act(async () => {
    mockProps.onPointerLock?.({
      nativeEvent: { mouse: true, locked: false, reason: 'notFullScreen' },
    } as never);
  });
  const normalized = {
    mouse: true,
    locked: false,
    reason: 'notFullScreen',
  };
  expect(onPointerLock).toHaveBeenCalledWith(normalized);
  expect(pointerLockStore.getState().states['123']).toEqual(normalized);

  await act(async () => {
    view.rerender(
      <RemoteSessionView
        {...props}
        sessionId="456"
        onPointerLock={onPointerLock}
      />,
    );
  });
  expect(pointerLockStore.getState().states['123']).toBeUndefined();
});

it('forwards imperative commands to the native view ref', async () => {
  const ref = createRef<RemoteSessionHandle>();
  await render(<RemoteSessionView {...props} ref={ref} />);
  ref.current?.showMagnifier(true);
  ref.current?.zoomReset();
  ref.current?.centerCursor();
  expect(Commands.showMagnifier).toHaveBeenCalledWith(mockNativeHandle, true);
  expect(Commands.zoomReset).toHaveBeenCalledWith(mockNativeHandle);
  expect(Commands.centerCursor).toHaveBeenCalledWith(mockNativeHandle);
});

it('passes the swipe order, snapshot and space events through (RS-SPACES1)', async () => {
  const onSpaceSwipe = jest.fn();
  const onSpaceSwitch = jest.fn();
  const ref = createRef<RemoteSessionHandle>();
  await render(
    <RemoteSessionView
      {...props}
      onSpaceSwipe={onSpaceSwipe}
      onSpaceSwitch={onSpaceSwitch}
      ref={ref}
      sessionId="7"
      showSnapshot
      spaces={[
        { id: '3', name: 'Studio' },
        { id: '7', name: 'Win PC' },
      ]}
    />,
  );
  expect(mockProps.spaceIds).toEqual(['3', '7']);
  expect(mockProps.spaceNames).toEqual(['Studio', 'Win PC']);
  expect(mockProps.showSnapshot).toBe(true);

  await act(async () => {
    mockProps.onSpaceSwipe?.({ nativeEvent: { phase: 'began' } } as never);
    mockProps.onSpaceSwitch?.({ nativeEvent: { sessionId: '3' } } as never);
  });
  expect(onSpaceSwipe).toHaveBeenCalledWith({ phase: 'began' });
  expect(onSpaceSwitch).toHaveBeenCalledWith({ sessionId: '3' });

  ref.current?.animateSpace('3', 'previous');
  expect(Commands.animateSpace).toHaveBeenCalledWith(
    mockNativeHandle,
    '3',
    'previous',
  );
});

it('sends no swipe order and no snapshot by default', async () => {
  await render(<RemoteSessionView {...props} />);
  expect(mockProps.spaceIds).toEqual([]);
  expect(mockProps.spaceNames).toEqual([]);
  expect(mockProps.showSnapshot).toBe(false);
});
