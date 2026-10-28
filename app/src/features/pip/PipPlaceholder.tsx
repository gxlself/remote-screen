import { StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { pip } from '../../i18n/zh/pip';
import type { SlotProps } from '../../screens/Session/slots';
import { space, tokens } from '../../theme/tokens';
import { useWindowPhase } from '../session/WindowContext';
import { stopPip } from './nativePip';
import { usePipState } from './pipState';

/** App 在前台、画面在小窗里时的会话页占位。 */
export function PipPlaceholder({ ctx }: SlotProps) {
  const pipState = usePipState();
  // this page's window in front, not just the app.
  const phase = useWindowPhase();

  if (
    !pipState.active ||
    pipState.sessionId !== ctx.sessionId ||
    phase !== 'active'
  ) {
    return null;
  }

  return (
    // 外层不能 accessible，否则读屏把「回到全屏」按钮吞进一个整体里点不到。
    <View style={styles.overlay} testID="pip-placeholder">
      <View style={styles.content}>
        <AppText
          accessibilityLabel={pip.placeholderA11y}
          accessibilityRole="header"
          center
          testID="pip-placeholder-title"
          variant="headline"
        >
          {pip.placeholderTitle}
        </AppText>
        <Button
          onPress={stopPip}
          testID="pip-placeholder-back"
          title={pip.placeholderBack}
        />
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  overlay: {
    ...StyleSheet.absoluteFill,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.bg,
  },
  content: {
    alignItems: 'center',
    gap: space.lg,
    padding: space.gutter,
  },
});

export default PipPlaceholder;
