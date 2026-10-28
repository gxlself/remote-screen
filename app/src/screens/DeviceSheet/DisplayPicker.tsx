import {
  PixelRatio,
  Platform,
  Pressable,
  StyleSheet,
  View,
} from 'react-native';
import { SectionHeader } from '../../components/List';
import AppText from '../../components/AppText';
import {
  displayDetail,
  displayName,
} from '../../features/directory/displayLabel';
import { deviceSheet } from '../../i18n/zh/deviceSheet';
import type { HostDisplay } from '../../native/rscore/types';
import { tokens } from '../../theme/tokens';

export type DisplayPickerProps = {
  displays: readonly HostDisplay[];
  selectedId: number | undefined;
  onSelect: (displayId: number) => void;
};

export function DisplayPicker({
  displays,
  selectedId,
  onSelect,
}: DisplayPickerProps) {
  if (displays.length <= 1) {
    return null;
  }

  const largeFont = PixelRatio.getFontScale() > 2;
  // 同 ripple.neutral。
  const rippleProps =
    Platform.OS === 'android'
      ? { android_ripple: { color: 'rgba(255, 255, 255, 0.12)' } }
      : null;

  return (
    <View style={styles.section}>
      <SectionHeader title={deviceSheet.displays.section} />
      <View
        accessibilityLabel={deviceSheet.displays.section}
        accessibilityRole="radiogroup"
        style={styles.grid}
        testID="display-grid"
      >
        {displays.map((display, index) => {
          const title = displayName(display, index + 1);
          const detail = displayDetail(display);
          const selected = display.id === selectedId;

          return (
            <Pressable
              accessibilityLabel={deviceSheet.a11y.displayOption(title, detail)}
              accessibilityRole="radio"
              accessibilityState={{ checked: selected, selected }}
              {...rippleProps}
              key={display.id}
              onPress={() => onSelect(display.id)}
              style={[
                styles.option,
                selected ? styles.optionSelected : null,
                largeFont ? styles.optionLargeFont : null,
              ]}
              testID={`display-option-${display.id}`}
            >
              <AppText variant="bodyStrong">{title}</AppText>
              <AppText color="text3" variant="caption">
                {detail}
              </AppText>
            </Pressable>
          );
        })}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  section: {
    gap: 8,
  },
  grid: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 10,
  },
  option: {
    flexBasis: '47%',
    flexGrow: 1,
    minHeight: 62,
    justifyContent: 'center',
    gap: 2,
    paddingHorizontal: 12,
    paddingVertical: 9,
    borderWidth: 2,
    borderRadius: 14,
    borderColor: tokens.cardLine,
    backgroundColor: tokens.raised,
  },
  optionSelected: {
    borderColor: tokens.accent,
  },
  optionLargeFont: {
    flexBasis: '100%',
  },
});

export default DisplayPicker;
