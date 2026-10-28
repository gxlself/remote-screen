import { Platform, Pressable, StyleSheet, TextInput, View } from 'react-native';
import { Icon } from '../../components/icons/Icon';
import { common } from '../../i18n/zh/common';
import { home } from '../../i18n/zh/home';
import { hit, tokens } from '../../theme/tokens';
import { fontFor } from '../../theme/typography';

export type SearchFieldProps = {
  query: string;
  setQuery(q: string): void;
};

export function SearchField({ query, setQuery }: SearchFieldProps) {
  const clearHitSlop = Math.max(8, (hit.min - 32) / 2);

  return (
    <View style={[styles.field, { minHeight: hit.min }]}>
      <Icon color="text3" name="search" size={18} />
      <TextInput
        accessibilityLabel={common.a11y.search}
        autoCapitalize="none"
        autoCorrect={false}
        onChangeText={setQuery}
        placeholder={home.search.placeholder}
        placeholderTextColor={tokens.text3}
        returnKeyType="search"
        selectionColor={
          Platform.OS === 'android' ? tokens.accentSoft : undefined
        }
        cursorColor={Platform.OS === 'android' ? tokens.accent : undefined}
        underlineColorAndroid={
          Platform.OS === 'android' ? 'transparent' : undefined
        }
        style={[styles.input, { minHeight: hit.min }]}
        testID="home-search"
        value={query}
      />
      {query === '' ? null : (
        <Pressable
          accessibilityLabel={home.a11y.clearSearch}
          accessibilityRole="button"
          // 同 ripple.neutral
          android_ripple={
            Platform.OS === 'android'
              ? { borderless: true, color: 'rgba(255, 255, 255, 0.12)' }
              : undefined
          }
          hitSlop={clearHitSlop}
          onPress={() => setQuery('')}
          style={styles.clear}
          testID="home-search-clear"
        >
          <Icon color="text3" name="close" size={18} />
        </Pressable>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  field: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    borderRadius: 12,
    backgroundColor: tokens.raised,
    paddingLeft: 14,
    paddingRight: 8,
  },
  input: {
    flex: 1,
    padding: 0,
    paddingVertical: 8,
    color: tokens.text,
    fontFamily: fontFor(400),
    fontSize: 15,
  },
  clear: {
    width: 32,
    height: 32,
    alignItems: 'center',
    justifyContent: 'center',
  },
});
