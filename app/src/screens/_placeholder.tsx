import { StyleSheet, Text, View } from 'react-native';
import { tokens } from '../theme/tokens';

export default function ScreenPlaceholder({ name }: { name: string }) {
  return (
    <View testID={`placeholder-${name}`} style={styles.screen}>
      <Text style={styles.label}>{name}</Text>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.bg,
  },
  label: { color: tokens.text, fontFamily: 'Geist-Regular', fontSize: 24 },
});
