import { Pressable, StyleSheet, View } from 'react-native';
import { session } from '../../i18n/zh/session';
import { hit } from '../../theme/tokens';

const HANDLE_HEIGHT = 28;

export type ToolbarHandleProps = {
  bottom: number;
  onPress: () => void;
};

/** 三指收起工具栏后留在底部的半透明把手，点一下工具栏回来。 */
export function ToolbarHandle({ bottom, onPress }: ToolbarHandleProps) {
  const slop = Math.max(0, (hit.min - HANDLE_HEIGHT) / 2);
  return (
    <View pointerEvents="box-none" style={[styles.anchor, { bottom }]}>
      <Pressable
        accessibilityLabel={session.toolbar.handle}
        accessibilityRole="button"
        hitSlop={{ top: slop, bottom: slop, left: 8, right: 8 }}
        onPress={onPress}
        style={({ pressed }) => [
          styles.handle,
          pressed ? styles.pressed : null,
        ]}
        testID="toolbar-handle"
      >
        <View style={styles.bar} />
      </Pressable>
    </View>
  );
}

export default ToolbarHandle;

const styles = StyleSheet.create({
  anchor: {
    position: 'absolute',
    left: 0,
    right: 0,
    alignItems: 'center',
  },
  handle: {
    width: 88,
    height: HANDLE_HEIGHT,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: HANDLE_HEIGHT / 2,
    backgroundColor: 'rgba(10,13,16,0.35)',
  },
  pressed: {
    backgroundColor: 'rgba(10,13,16,0.6)',
  },
  bar: {
    width: 40,
    height: 5,
    borderRadius: 3,
    backgroundColor: 'rgba(255,255,255,0.55)',
  },
});
