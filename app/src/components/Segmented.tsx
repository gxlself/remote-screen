import { Pressable, StyleSheet, View } from 'react-native';
import { boxHeight } from '../theme/fontScale';
import { isAndroid, press, ripple } from '../theme/platform';
import { hit, tokens } from '../theme/tokens';
import AppText from './AppText';

export type SegmentedVariant = 'page' | 'overlay';

export type SegmentedOption<T extends string> = {
  value: T;
  label: string;
  accessibilityLabel?: string;
  disabled?: boolean;
};

export type SegmentedProps<T extends string> = {
  options: SegmentedOption<T>[];
  value: T;
  onChange: (value: T) => void;
  variant?: SegmentedVariant;
  accessibilityLabel: string;
  testID?: string;
};

export function Segmented<T extends string>({
  options,
  value,
  onChange,
  variant = 'page',
  accessibilityLabel,
  testID,
}: SegmentedProps<T>) {
  const android = isAndroid();
  const overlay = variant === 'overlay';
  const optionHitSlop = {
    top: (hit.min - 40) / 2,
    bottom: (hit.min - 40) / 2,
  };
  const optionHeight = boxHeight(40);

  return (
    <View
      accessibilityLabel={accessibilityLabel}
      accessibilityRole="radiogroup"
      style={[
        styles.group,
        overlay ? styles.overlayGroup : styles.pageGroup,
        // Android clips hitSlop to the parent, including the overlay variant.
        android ? { minHeight: hit.min } : null,
      ]}
      testID={testID}
    >
      {options.map(option => {
        const selected = option.value === value;
        const disabled = option.disabled === true;
        return (
          <Pressable
            key={option.value}
            accessibilityLabel={option.accessibilityLabel ?? option.label}
            accessibilityRole="radio"
            accessibilityState={{ selected }}
            android_ripple={android ? { color: ripple.neutral } : undefined}
            // Pressable adds `disabled` to the state only when it is set.
            disabled={option.disabled}
            hitSlop={optionHitSlop}
            onPress={() => {
              if (!selected) {
                onChange(option.value);
              }
            }}
            style={({ pressed }) => [
              styles.option,
              optionHeight,
              android ? { minWidth: hit.min } : null,
              overlay ? styles.overlayOption : styles.pageOption,
              selected
                ? overlay
                  ? styles.overlaySelected
                  : styles.pageSelected
                : null,
              pressed && !selected && !android ? styles.pressed : null,
              disabled ? styles.disabled : null,
            ]}
            testID={testID == null ? undefined : `${testID}-${option.value}`}
          >
            <AppText
              color={selected ? (overlay ? 'bg' : 'text') : 'text2'}
              variant={overlay ? 'footnote' : 'button'}
              weight={600}
            >
              {option.label}
            </AppText>
          </Pressable>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create({
  group: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  pageGroup: {
    alignSelf: 'stretch',
    backgroundColor: tokens.bg,
    borderRadius: 12,
    padding: 4,
    gap: 4,
  },
  overlayGroup: {
    alignSelf: 'flex-start',
    gap: 4,
  },
  option: {
    alignItems: 'center',
    justifyContent: 'center',
  },
  pageOption: {
    flex: 1,
    borderRadius: 9,
  },
  overlayOption: {
    borderRadius: 20,
    paddingHorizontal: 12,
  },
  pageSelected: {
    backgroundColor: tokens.selected,
  },
  overlaySelected: {
    backgroundColor: tokens.knob,
  },
  pressed: {
    backgroundColor: press.highlight,
  },
  disabled: {
    opacity: 0.4,
  },
});
