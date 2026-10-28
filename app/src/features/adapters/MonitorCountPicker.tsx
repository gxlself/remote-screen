import type { JSX } from 'react';
import { StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { SectionHeader } from '../../components/List';
import { Segmented } from '../../components/Segmented';
import { adapters } from '../../i18n/zh/adapters';
import { space } from '../../theme/tokens';
import { MONITOR_COUNTS, type MonitorCount } from './monitorMemory';

export type MonitorCountPickerProps = {
  count: MonitorCount;
  onChange(count: MonitorCount): void;
  testID?: string;
};

/** 「显示器数量：1 / 2 / 3 / 4」，只给远程桌面用。 */
export function MonitorCountPicker({
  count,
  onChange,
  testID = 'adapter-monitors',
}: MonitorCountPickerProps): JSX.Element {
  return (
    <View style={styles.section}>
      <SectionHeader title={adapters.monitors.title} />
      <Segmented
        accessibilityLabel={adapters.monitors.title}
        onChange={value => {
          const next = MONITOR_COUNTS.find(n => String(n) === value);
          if (next !== undefined) {
            onChange(next);
          }
        }}
        options={MONITOR_COUNTS.map(n => ({
          value: String(n),
          label: String(n),
          accessibilityLabel: adapters.monitors.value(n),
        }))}
        testID={testID}
        value={String(count)}
      />
      <AppText color="text3" style={styles.note} variant="footnote">
        {adapters.monitors.note}
      </AppText>
    </View>
  );
}

const styles = StyleSheet.create({
  section: {
    gap: space.sm,
  },
  note: {
    paddingHorizontal: space.xs,
  },
});

export default MonitorCountPicker;
