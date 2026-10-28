import { StyleSheet, View } from 'react-native';
import { tokens } from '../theme/tokens';
import AppText from './AppText';
import { Icon, type IconName } from './icons/Icon';
import { PathMark, type PathKind } from './PathMark';

export type StatusChipTone =
  | 'good'
  | 'relay'
  | 'neutral'
  | 'offline'
  | 'hostControlled'
  | 'track'
  | 'accent';

export type StatusChipProps = {
  label: string;
  tone: StatusChipTone;
  mark?: PathKind;
  icon?: IconName;
  size?: 'md' | 'sm';
  testID?: string;
};

const toneStyles: Record<
  StatusChipTone,
  { backgroundColor?: string; color: string; weight: 500 | 700 }
> = {
  good: {
    backgroundColor: tokens.goodSoft,
    color: tokens.good,
    weight: 500,
  },
  relay: {
    backgroundColor: tokens.relaySoft,
    color: tokens.relay,
    weight: 500,
  },
  neutral: {
    backgroundColor: tokens.raised,
    color: tokens.text2,
    weight: 500,
  },
  offline: { color: tokens.text3, weight: 500 },
  hostControlled: {
    backgroundColor: tokens.hostControlled,
    color: tokens.onHostControlled,
    weight: 700,
  },
  track: {
    backgroundColor: tokens.track,
    color: tokens.text2,
    weight: 500,
  },
  accent: {
    backgroundColor: tokens.accentSoft,
    color: tokens.accent,
    weight: 500,
  },
};

export function StatusChip({
  label,
  tone,
  mark,
  icon,
  size = 'md',
  testID,
}: StatusChipProps) {
  const small = size === 'sm';
  const toneStyle = toneStyles[tone];
  const graphicSize = small ? 12 : 14;

  return (
    <View
      accessible
      accessibilityLabel={label}
      style={[
        styles.chip,
        small ? styles.small : styles.medium,
        { backgroundColor: toneStyle.backgroundColor },
        tone === 'offline' && styles.offline,
      ]}
      testID={testID}
    >
      {mark ? (
        <PathMark kind={mark} />
      ) : icon ? (
        <Icon
          name={icon}
          size={graphicSize}
          color={toneStyle.color}
          strokeWidth={1.8}
        />
      ) : null}
      <AppText
        color={tone === 'good' ? 'good' : undefined}
        style={{ color: toneStyle.color }}
        variant={small ? 'tiny' : 'caption'}
        weight={toneStyle.weight}
      >
        {label}
      </AppText>
    </View>
  );
}

const styles = StyleSheet.create({
  chip: {
    flexDirection: 'row',
    alignItems: 'center',
  },
  // minHeight, so large text grows the chip instead of being clipped.
  medium: {
    minHeight: 26,
    borderRadius: 13,
    paddingHorizontal: 10,
    gap: 6,
  },
  small: {
    minHeight: 20,
    borderRadius: 10,
    paddingHorizontal: 7,
    gap: 4,
  },
  offline: {
    borderWidth: 1,
    borderStyle: 'dashed',
    borderColor: tokens.switchOff,
  },
});
