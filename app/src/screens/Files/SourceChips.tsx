import { Pressable, ScrollView, StyleSheet } from 'react-native';
import AppText from '../../components/AppText';
import { filesBrowse } from '../../i18n/zh/filesBrowse';
import type { SourceChip } from '../../features/files/browseView';
import { hit, tokens } from '../../theme/tokens';

export function SourceChips({
  chips,
  selectedKey,
  onSelect,
}: {
  chips: SourceChip[];
  selectedKey: string | null;
  onSelect(key: string): void;
}) {
  return (
    <ScrollView
      accessibilityLabel={filesBrowse.chipsLabel}
      contentContainerStyle={styles.content}
      horizontal
      showsHorizontalScrollIndicator={false}
    >
      {chips.map(chip => {
        const selected = chip.key === selectedKey;
        return (
          <Pressable
            accessibilityLabel={chip.label}
            accessibilityRole="button"
            accessibilityState={{ selected }}
            key={chip.key}
            onPress={() => onSelect(chip.key)}
            style={[
              styles.chip,
              { minHeight: hit.min },
              selected ? styles.selected : null,
            ]}
          >
            <AppText
              color={selected ? 'bg' : 'text'}
              variant="button"
              weight={selected ? 600 : 400}
            >
              {chip.label}
            </AppText>
          </Pressable>
        );
      })}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  content: {
    gap: 6,
  },
  chip: {
    borderRadius: 22,
    paddingHorizontal: 14,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.raised,
  },
  selected: {
    backgroundColor: tokens.text,
  },
});
