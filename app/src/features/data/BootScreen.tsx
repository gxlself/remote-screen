import { useEffect, useState } from 'react';
import {
  AccessibilityInfo,
  ActivityIndicator,
  StyleSheet,
  View,
} from 'react-native';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { boot } from '../../i18n/zh/boot';
import { tokens } from '../../theme/tokens';

export default function BootScreen({
  state,
  message,
  onRetry,
}: {
  state: 'booting' | 'failed';
  message?: string;
  onRetry?: () => void;
}) {
  const [showSpinner, setShowSpinner] = useState(false);

  useEffect(() => {
    setShowSpinner(false);
    if (state === 'failed') {
      AccessibilityInfo.announceForAccessibility(
        message ? `${boot.failedTitle}，${message}` : boot.failedTitle,
      );
      return;
    }
    const timer = setTimeout(() => setShowSpinner(true), 600);
    return () => clearTimeout(timer);
  }, [state, message]);

  return (
    <View style={styles.root} testID="boot-screen">
      {state === 'failed' ? (
        <>
          <AppText accessibilityRole="header" variant="title" weight={600}>
            {boot.failedTitle}
          </AppText>
          <AppText color="text2" numberOfLines={1} style={styles.message}>
            {message}
          </AppText>
          {onRetry === undefined ? null : (
            <Button title={boot.retry} onPress={onRetry} />
          )}
        </>
      ) : showSpinner ? (
        <ActivityIndicator
          accessibilityLabel={boot.loading}
          color={tokens.accent}
          testID="boot-spinner"
        />
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  root: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.bg,
    padding: 24,
    gap: 16,
  },
  message: { alignSelf: 'stretch', textAlign: 'center' },
});
