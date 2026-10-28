import { StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { Icon } from '../../components/icons/Icon';
import { ipadSession } from '../../i18n/zh/ipadSession';
import {
  pointerChipKind,
  usePointerLock,
} from '../../native/session/pointerLockStore';
import { useViewport } from '../../features/session/videoStore';
import { hit, tokens } from '../../theme/tokens';
import { FRAME_HEIGHT, minimapVisible } from './SessionMinimap';
import type { SlotProps } from './slots';

const CHIP_TOP = 12;
const MINIMAP_GAP = 8;

export function PointerLockChip({ ctx }: SlotProps) {
  const kind = pointerChipKind(usePointerLock(ctx.sessionId), ctx.isPad);
  const viewport = useViewport(ctx.sessionId);
  // 小地图也在右上角；它出现时提示挪到它下面。
  const belowMinimap = minimapVisible(ctx, viewport?.zoom ?? null);
  if (kind === null) {
    return null;
  }

  const copy =
    kind === 'split'
      ? {
          label: ipadSession.pointer.split,
          a11y: ipadSession.pointer.splitA11y,
        }
      : {
          label: ipadSession.pointer.escaped,
          a11y: ipadSession.pointer.escapedA11y,
        };

  return (
    <View
      accessible
      accessibilityLabel={copy.a11y}
      pointerEvents="none"
      style={[
        styles.chip,
        belowMinimap ? { top: CHIP_TOP + FRAME_HEIGHT + MINIMAP_GAP } : null,
      ]}
      testID="pointer-lock-chip"
    >
      <Icon color="#D5DCE0" name="lock" size={15} />
      <AppText style={styles.label}>{copy.label}</AppText>
    </View>
  );
}

export default PointerLockChip;

const styles = StyleSheet.create({
  chip: {
    position: 'absolute',
    top: CHIP_TOP,
    right: hit.landscapeSafe,
    height: 32,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 7,
    paddingHorizontal: 12,
    borderRadius: 16,
    backgroundColor: tokens.card,
  },
  label: { color: '#D5DCE0', fontSize: 12, lineHeight: 16 },
});
