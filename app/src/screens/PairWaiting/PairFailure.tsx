import { StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Icon } from '../../components/icons/Icon';
import type { FailureModel } from '../../features/pairing/outcome';
import { tokens } from '../../theme/tokens';

export type PairFailureProps = {
  failure: FailureModel;
  onPrimary: () => void;
  onClose: () => void;
};

export function PairFailure({ failure, onPrimary, onClose }: PairFailureProps) {
  return (
    <View style={styles.container} testID="pair-failure">
      <View
        style={[
          styles.icon,
          {
            backgroundColor:
              failure.tone === 'bad' ? tokens.badSoft : tokens.relaySoft,
          },
        ]}
        testID="pair-failure-icon"
      >
        <Icon
          color={failure.tone === 'bad' ? 'bad' : 'relay'}
          name={failure.icon}
          size={26}
        />
      </View>
      <View style={styles.copy}>
        <AppText accessibilityRole="header" center variant="headline">
          {failure.title}
        </AppText>
        <AppText center color="text2" variant="body">
          {failure.message}
        </AppText>
      </View>
      <View style={styles.actions}>
        <Button
          fullWidth
          onPress={onPrimary}
          testID="pair-failure-primary"
          title={failure.primary.label}
          variant="primaryLarge"
        />
        {failure.secondary === undefined ? null : (
          <Button
            fullWidth
            onPress={onClose}
            testID="pair-failure-close"
            title={failure.secondary.label}
            variant="secondary"
          />
        )}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  container: { alignItems: 'center', gap: 14, paddingVertical: 8 },
  icon: {
    width: 56,
    height: 56,
    borderRadius: 16,
    alignItems: 'center',
    justifyContent: 'center',
  },
  copy: { alignItems: 'center', gap: 6 },
  actions: { alignSelf: 'stretch', gap: 10, marginTop: 2 },
});

export default PairFailure;
