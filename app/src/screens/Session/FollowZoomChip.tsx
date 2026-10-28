import { PixelRatio, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { Icon } from '../../components/icons/Icon';
import { useViewport } from '../../features/session/videoStore';
import { zoomLabel } from '../../features/session/viewportText';
import { sessionViewport } from '../../i18n/zh/sessionViewport';
import type { SessionId } from '../../native/rscore/types';
import { tokens } from '../../theme/tokens';

const LARGE_TEXT_STYLE = { height: undefined, minHeight: 30 } as const;

export type FollowZoomChipProps = { sessionId: SessionId | null };

export function FollowZoomChip({ sessionId }: FollowZoomChipProps) {
  const viewport = useViewport(sessionId);
  const largeText = PixelRatio.getFontScale() > 2;

  return (
    <View
      accessible={false}
      style={[
        styles.chip,
        largeText ? LARGE_TEXT_STYLE : null,
      ]}
      testID="follow-zoom-chip"
    >
      <Icon color="#A9B4BB" name="zoomIn" size={14} />
      <AppText color="text" style={styles.label} variant="caption">
        {sessionViewport.followZoom.tracking}
      </AppText>
      <AppText mono style={styles.zoom} variant="caption">
        {zoomLabel(viewport?.zoom ?? 1)}
      </AppText>
    </View>
  );
}

export default FollowZoomChip;

const styles = StyleSheet.create({
  chip: {
    height: 30,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: 10,
    borderRadius: 15,
    backgroundColor: 'rgba(10,13,16,0.8)',
  },
  label: { color: tokens.text, fontSize: 12, lineHeight: 16 },
  zoom: { color: '#A9B4BB', fontSize: 12, lineHeight: 16 },
});
