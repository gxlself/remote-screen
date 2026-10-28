import { StyleSheet, View } from 'react-native';
import { tokens } from '../theme/tokens';
import AppText from './AppText';
import { Button } from './Button';
import { Icon, type IconName } from './icons/Icon';

export type EmptyStateAction = {
  label: string;
  onPress: () => void;
  loading?: boolean;
  disabled?: boolean;
  testID?: string;
};

export type EmptyStateProps = {
  icon: IconName;
  title: string;
  message?: string;
  action?: EmptyStateAction;
};

export function EmptyState({ icon, title, message, action }: EmptyStateProps) {
  return (
    <View style={styles.container} testID="empty-state">
      <View style={styles.icon} testID="empty-icon">
        <Icon color="text2" name={icon} size={26} testID="empty-icon-glyph" />
      </View>
      <View style={styles.copy}>
        <AppText accessibilityRole="header" center variant="headline">
          {title}
        </AppText>
        {message == null ? null : (
          <AppText center color="text2" variant="body">
            {message}
          </AppText>
        )}
      </View>
      {action == null ? null : (
        <Button
          align="center"
          disabled={action.disabled}
          loading={action.loading}
          onPress={action.onPress}
          testID={action.testID}
          title={action.label}
        />
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    alignItems: 'center',
    justifyContent: 'center',
    gap: 12,
  },
  icon: {
    width: 56,
    height: 56,
    borderRadius: 16,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.raised,
  },
  copy: {
    alignItems: 'center',
    gap: 12,
  },
});
