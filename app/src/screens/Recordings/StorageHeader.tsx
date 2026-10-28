import { StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import type { RecordingStorage } from '../../native/rscore/types';
import { storageSummary } from '../../features/recording/library';
import { tokens } from '../../theme/tokens';

export type StorageHeaderProps = {
  storage: RecordingStorage;
};

export function StorageHeader({ storage }: StorageHeaderProps) {
  return (
    <View style={styles.container} testID="recordings-storage">
      <AppText color="text2" variant="body">
        {storageSummary(storage)}
      </AppText>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    borderRadius: 16,
    borderWidth: 1,
    borderColor: tokens.cardLine,
    backgroundColor: tokens.card,
    paddingHorizontal: 14,
    paddingVertical: 14,
  },
});

export default StorageHeader;
