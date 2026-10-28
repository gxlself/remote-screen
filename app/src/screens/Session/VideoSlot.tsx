import { Platform, StyleSheet, View } from 'react-native';
import type { ViewProps } from 'react-native';
import { setVideoStats, setViewport } from '../../features/session/videoStore';
import { useSetting } from '../../features/settings/store';
import { RemoteSessionView } from '../../native/session/RemoteSessionView';
import type { VideoSlotProps } from './slots';

export function VideoSlot(props: VideoSlotProps) {
  const { sessionId } = props;
  const [cursorSpeed] = useSetting('session.cursorSpeed');
  const [naturalScroll] = useSetting('session.naturalScroll');
  const [haptics] = useSetting('session.haptics');
  if (Platform.OS === 'ios' || Platform.OS === 'android') {
    return (
      <RemoteSessionView
        {...props}
        cursorSpeed={cursorSpeed}
        naturalScroll={naturalScroll}
        haptics={haptics}
        onViewport={viewport => {
          if (sessionId) {
            setViewport(sessionId, viewport);
          }
        }}
        onVideoStats={stats => {
          if (sessionId) {
            setVideoStats(sessionId, stats);
          }
        }}
      />
    );
  }

  const events: ViewProps & Pick<VideoSlotProps, 'onGesture' | 'onFirstFrame'> =
    {
      onGesture: props.onGesture,
      onFirstFrame: props.onFirstFrame,
    };
  return (
    <View
      {...events}
      accessible
      accessibilityLabel={props.accessibilityLabel}
      accessibilityRole="image"
      testID="remote-session-view-placeholder"
      style={styles.video}
    />
  );
}

const styles = StyleSheet.create({
  video: { ...StyleSheet.absoluteFill, backgroundColor: '#000' },
});

export default VideoSlot;
