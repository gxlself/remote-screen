import { StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { EmptyState } from '../../components/EmptyState';
import { home } from '../../i18n/zh/home';
import { native } from '../../i18n/zh/native';

export type HomeEmptyProps = {
  onAdd(): void;
  onInstallHost(): void;
  /** Tailscale is down: nothing from it can be listed yet. */
  reason?: 'tailnetDown';
  onOpenTailscale?(): void;
};

/** No computers yet: system sharing first, installing Host is the other way. */
export function HomeEmpty({
  onAdd,
  onInstallHost,
  reason,
  onOpenTailscale,
}: HomeEmptyProps) {
  if (reason === 'tailnetDown' && onOpenTailscale !== undefined) {
    // Tailscale is optional (only for reaching computers from outside), so the
    // system-sharing guide stays first and Tailscale is the secondary way.
    return (
      <View style={styles.wrap} testID="home-empty-tailnet">
        <EmptyState
          action={{
            label: native.home.add,
            onPress: onAdd,
            testID: 'home-empty-add',
          }}
          icon="computerAdd"
          message={native.home.emptyBody}
          title={native.home.emptyTitle}
        />
        <AppText center color="text3" style={styles.note} variant="caption">
          {home.empty.tailnetDownMessage}
        </AppText>
        <Button
          align="center"
          onPress={onOpenTailscale}
          testID="home-empty-open-tailscale"
          title={home.banners.openTailscale}
          variant="text"
        />
      </View>
    );
  }

  return (
    <View style={styles.wrap} testID="home-empty">
      <EmptyState
        action={{ label: native.home.add, onPress: onAdd }}
        icon="computerAdd"
        message={native.home.emptyBody}
        title={native.home.emptyTitle}
      />
      <Button
        align="center"
        onPress={onInstallHost}
        testID="home-empty-install-host"
        title={native.home.otherWays}
        variant="text"
      />
    </View>
  );
}

const styles = StyleSheet.create({
  wrap: {
    alignItems: 'center',
    gap: 4,
    paddingVertical: 8,
  },
  note: {
    marginTop: 12,
  },
});
