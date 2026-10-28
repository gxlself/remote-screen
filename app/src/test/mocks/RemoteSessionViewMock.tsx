import { useImperativeHandle, useRef } from 'react';
import { View } from 'react-native';
import type { ViewProps } from 'react-native';
import type { RemoteSessionViewProps } from '../../native/session/RemoteSessionView';

/** slides asked of the native view, as (sessionId, direction). */
export const mockAnimateSpace = jest.fn<void, [string, 'next' | 'previous']>();

export function RemoteSessionView(props: RemoteSessionViewProps) {
  const latest = useRef(props);
  latest.current = props;
  useImperativeHandle(
    props.ref,
    () => ({
      zoomReset: jest.fn(),
      centerCursor: jest.fn(),
      showMagnifier: jest.fn(),
      // Like the native view: the slide ends and asks for the switch.
      animateSpace: (sessionId: string, direction: 'next' | 'previous') => {
        mockAnimateSpace(sessionId, direction);
        latest.current.onSpaceSwitch?.({ sessionId });
      },
    }),
    [],
  );
  const events: ViewProps &
    Pick<
      RemoteSessionViewProps,
      | 'onGesture'
      | 'onFirstFrame'
      | 'onViewport'
      | 'onVideoStats'
      | 'onSpaceSwipe'
      | 'onSpaceSwitch'
      | 'showSnapshot'
    > = {
    onGesture: props.onGesture,
    onFirstFrame: props.onFirstFrame,
    onViewport: props.onViewport,
    onVideoStats: props.onVideoStats,
    // the swipe events and the last-picture flag, as on the
    // native view.
    onSpaceSwipe: props.onSpaceSwipe,
    onSpaceSwitch: props.onSpaceSwitch,
    showSnapshot: props.showSnapshot ?? false,
  };
  return (
    <View
      {...events}
      accessible
      accessibilityRole="image"
      accessibilityLabel={props.accessibilityLabel}
      testID="remote-session-view-placeholder"
    />
  );
}

export default RemoteSessionView;
