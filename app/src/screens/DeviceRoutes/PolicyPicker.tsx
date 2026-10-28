import { Platform, Pressable, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import type { HostPrefs } from '../../features/settings/schema';
import {
  autoPolicySubtitle,
  type RouteRowModel,
} from '../../features/directory/routes';
import { deviceRoutes } from '../../i18n/zh/deviceRoutes';
import { tokens } from '../../theme/tokens';

export type RoutePolicy = HostPrefs['routePolicy'];

const POLICY_ORDER: RoutePolicy[] = ['auto', 'tailnet', 'intranet'];

export type PolicyPickerProps = {
  value: RoutePolicy;
  onChange: (value: RoutePolicy) => void;
  rows: readonly RouteRowModel[];
  /** Off: no 优先 tailnet option, unless it is what this computer uses. */
  tailnetEnabled?: boolean;
};

export function PolicyPicker({
  value,
  onChange,
  rows,
  tailnetEnabled = true,
}: PolicyPickerProps) {
  const policies = POLICY_ORDER.filter(
    policy => tailnetEnabled || policy !== 'tailnet' || value === 'tailnet',
  );
  // 同 ripple.neutral。
  const rippleProps =
    Platform.OS === 'android'
      ? { android_ripple: { color: 'rgba(255, 255, 255, 0.12)' } }
      : null;

  return (
    <View
      accessibilityLabel={deviceRoutes.policySection}
      accessibilityRole="radiogroup"
      style={styles.group}
      testID="policy-picker"
    >
      {policies.map(policy => {
        const selected = policy === value;
        const title = deviceRoutes.policies[policy].title;
        const subtitle =
          policy === 'auto'
            ? autoPolicySubtitle(rows)
            : policy === 'intranet' && !tailnetEnabled
            ? deviceRoutes.policies.intranet.subtitleLan
            : deviceRoutes.policies[policy].subtitle;
        return (
          <Pressable
            key={policy}
            accessibilityLabel={title}
            accessibilityHint={subtitle}
            accessibilityRole="radio"
            accessibilityState={{ checked: selected, selected }}
            {...rippleProps}
            onPress={() => {
              if (!selected) {
                onChange(policy);
              }
            }}
            style={[styles.option, selected ? styles.selected : null]}
            testID={`policy-${policy}`}
          >
            <View style={styles.optionCopy}>
              <AppText variant="body" weight={500}>
                {title}
              </AppText>
              <AppText color="text3" variant="caption">
                {subtitle}
              </AppText>
            </View>
            <View
              style={[
                styles.radio,
                selected ? styles.radioSelected : styles.radioIdle,
              ]}
              testID={`policy-${policy}-radio`}
            >
              {selected ? <View style={styles.dot} /> : null}
            </View>
          </Pressable>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create({
  group: {
    borderRadius: 16,
    borderWidth: 1,
    borderColor: tokens.cardLine,
    backgroundColor: tokens.card,
    overflow: 'hidden',
  },
  option: {
    minHeight: 64,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    paddingHorizontal: 14,
    paddingVertical: 9,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: tokens.cardLine,
  },
  selected: {
    backgroundColor: tokens.accentTint,
  },
  optionCopy: {
    flex: 1,
    gap: 2,
  },
  radio: {
    width: 22,
    height: 22,
    borderRadius: 11,
    alignItems: 'center',
    justifyContent: 'center',
  },
  radioSelected: {
    borderWidth: 2,
    borderColor: tokens.accent,
  },
  radioIdle: {
    borderWidth: 1.5,
    borderColor: tokens.switchOff,
  },
  dot: {
    width: 10,
    height: 10,
    borderRadius: 5,
    backgroundColor: tokens.accent,
  },
});

export default PolicyPicker;
