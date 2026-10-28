import { Children, Fragment, isValidElement, type ReactNode } from 'react';
import { Pressable, StyleSheet, View } from 'react-native';
import type { AccessibilityState, StyleProp, ViewStyle } from 'react-native';
import { isAndroid, press, ripple } from '../theme/platform';
import { tokens } from '../theme/tokens';
import AppText from './AppText';
import { Icon } from './icons/Icon';

export type SectionHeaderProps = {
  title: string;
  right?: ReactNode;
  accessibilityLabel?: string;
};

export function SectionHeader({
  title,
  right,
  accessibilityLabel,
}: SectionHeaderProps) {
  return (
    <View style={styles.sectionHeader}>
      <AppText
        accessibilityLabel={accessibilityLabel}
        accessibilityRole="header"
        color="text2"
        style={styles.sectionTitle}
        variant="section"
      >
        {title}
      </AppText>
      {right == null ? null : (
        <AppText color="text3" style={styles.sectionRight} variant="caption">
          {right}
        </AppText>
      )}
    </View>
  );
}

export type ListGroupProps = {
  children: ReactNode;
};

export function ListGroup({ children }: ListGroupProps) {
  const items = Children.toArray(children).filter(
    child => child !== null && typeof child !== 'boolean',
  );

  return (
    <View style={styles.group}>
      {items.map((child, index) => (
        <Fragment key={isValidElement(child) ? child.key : index}>
          {index === 0 ? null : (
            <View style={styles.separator} testID="list-separator" />
          )}
          {child}
        </Fragment>
      ))}
    </View>
  );
}

export type ListRowProps = {
  title: string;
  subtitle?: string;
  value?: ReactNode;
  left?: ReactNode;
  right?: ReactNode;
  chevron?: boolean;
  onPress?: () => void;
  tone?: 'normal' | 'danger';
  disabled?: boolean;
  accessibilityLabel?: string;
  accessibilityHint?: string;
  accessibilityState?: AccessibilityState;
  testID?: string;
};

export function ListRow({
  title,
  subtitle,
  value,
  left,
  right,
  chevron = false,
  onPress,
  tone = 'normal',
  disabled = false,
  accessibilityLabel,
  accessibilityHint,
  accessibilityState,
  testID,
}: ListRowProps) {
  const interactive = onPress != null;
  const valueText =
    typeof value === 'string' || typeof value === 'number'
      ? String(value)
      : undefined;
  const label =
    accessibilityLabel ??
    [title, subtitle, valueText]
      .filter(part => part != null && part !== '')
      .join('，');
  const android = isAndroid();
  const state = disabled
    ? { ...accessibilityState, disabled: true }
    : accessibilityState;
  const rowStyle: StyleProp<ViewStyle> = [
    styles.row,
    subtitle == null ? styles.rowSingle : styles.rowWithSubtitle,
    disabled ? styles.disabled : null,
  ];

  const content = (
    <>
      {left == null ? null : <View style={styles.left}>{left}</View>}
      <View style={styles.textBlock}>
        <AppText color={tone === 'danger' ? 'bad' : 'text'} variant="body">
          {title}
        </AppText>
        {subtitle == null ? null : (
          <AppText color="text3" variant="caption">
            {subtitle}
          </AppText>
        )}
      </View>
      {value == null ? null : (
        <AppText style={styles.value} color="text2">
          {value}
        </AppText>
      )}
      {right == null ? null : <View style={styles.right}>{right}</View>}
      {chevron ? (
        <Icon
          name="chevronRight"
          size={16}
          color="muted"
          testID="list-row-chevron"
        />
      ) : null}
    </>
  );

  if (!interactive) {
    // Group only rows whose label is complete and which contain no right control.
    const accessible =
      right == null &&
      (accessibilityLabel != null || value == null || valueText != null);
    return (
      <View
        {...(accessible ? { accessible: true, accessibilityLabel: label } : null)}
        accessibilityState={state}
        style={rowStyle}
        testID={testID}
      >
        {content}
      </View>
    );
  }

  return (
    <Pressable
      accessibilityHint={accessibilityHint}
      accessibilityLabel={label}
      accessibilityRole="button"
      accessibilityState={state}
      android_ripple={
        android ? { color: ripple.neutral, foreground: true } : undefined
      }
      disabled={disabled}
      onPress={onPress}
      style={({ pressed }) => [
        rowStyle,
        pressed && !android ? styles.pressed : null,
      ]}
      testID={testID}
    >
      {content}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  sectionHeader: {
    minHeight: 24,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 12,
  },
  sectionTitle: {
    flexShrink: 1,
  },
  sectionRight: {
    flexShrink: 1,
    textAlign: 'right',
  },
  group: {
    borderRadius: 16,
    backgroundColor: tokens.card,
    borderWidth: 1,
    borderColor: tokens.cardLine,
    overflow: 'hidden',
  },
  separator: {
    height: 1,
    backgroundColor: tokens.cardLine,
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 14,
    gap: 10,
  },
  rowSingle: {
    minHeight: 52,
  },
  rowWithSubtitle: {
    minHeight: 60,
    paddingVertical: 8,
  },
  pressed: {
    backgroundColor: press.highlight,
  },
  disabled: {
    opacity: 0.4,
  },
  left: {
    alignItems: 'center',
    justifyContent: 'center',
  },
  textBlock: {
    flex: 1,
  },
  value: {
    fontSize: 14,
    lineHeight: 20,
  },
  right: {
    alignItems: 'center',
    justifyContent: 'center',
  },
});
