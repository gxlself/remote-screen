import { useEffect, useRef, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import type { RootScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { Icon } from '../../components/icons/Icon';
import { ListGroup, ListRow, SectionHeader } from '../../components/List';
import { Switch } from '../../components/Switch';
import { showToast } from '../../components/ToastHost';
import { useDirectory } from '../../features/data/queries';
import { useAuthName } from '../../features/lock/authName';
import { needsAuthToChange } from '../../features/lock/policy';
import { toggleSensitive } from '../../features/settings/schema';
import { useSetting } from '../../features/settings/store';
import { appLock } from '../../i18n/zh/appLock';
import { common } from '../../i18n/zh/common';
import { usePlatform } from '../../native/platform/PlatformContext';
import { isAndroid, press } from '../../theme/platform';
import { hit, space, tokens } from '../../theme/tokens';

export default function AppLockScreen({
  navigation,
}: RootScreenProps<'AppLock'>) {
  const insets = useSafeAreaInsets();
  const platform = usePlatform();
  const authenticationName = useAuthName();
  const { directory } = useDirectory();
  const [onOpen, setOnOpen] = useSetting('lock.onOpen');
  const [beforeSession, setBeforeSession] = useSetting('lock.beforeSession');
  const [sensitiveHosts, setSensitiveHosts] = useSetting('lock.sensitiveHosts');
  const sensitiveHostsRef = useRef(sensitiveHosts);
  sensitiveHostsRef.current = sensitiveHosts;
  const [authenticating, setAuthenticating] = useState(false);
  const authInFlight = useRef(false);
  const mounted = useRef(true);
  const computers = directory?.computers ?? [];

  useEffect(() => {
    return () => {
      mounted.current = false;
    };
  }, []);

  const commit = (from: boolean, to: boolean, apply: () => void) => {
    if (!needsAuthToChange(from, to)) {
      apply();
      return;
    }
    if (authInFlight.current) {
      return;
    }
    authInFlight.current = true;
    setAuthenticating(true);
    platform
      .authenticate(
        appLock.screen.authenticateDisableReason(authenticationName),
      )
      .then(result => {
        if (result === 'ok' || result === 'unavailable') {
          apply();
        } else if (result === 'failed') {
          // S24: say why the switch sprang back; a cancel stays silent.
          showToast({ message: appLock.screen.disableFailed });
        }
      })
      .catch(() => {
        // A failed platform call leaves the protection unchanged.
        showToast({ message: appLock.screen.disableFailed });
      })
      .finally(() => {
        authInFlight.current = false;
        if (mounted.current) {
          setAuthenticating(false);
        }
      });
  };

  return (
    <ScrollView
      contentContainerStyle={[
        styles.content,
        { paddingTop: insets.top + space.sm },
      ]}
      style={styles.screen}
      testID="app-lock-scroll"
    >
      <Pressable
        accessibilityLabel={appLock.screen.back}
        accessibilityRole="button"
        onPress={() => navigation.goBack()}
        style={({ pressed }) => [
          styles.back,
          { minHeight: hit.min },
          pressed && !isAndroid() ? styles.pressed : null,
        ]}
        testID="app-lock-back"
      >
        <Icon color="accent" name="chevronLeft" size={18} />
        <AppText color="accent" variant="body">
          {common.settings}
        </AppText>
      </Pressable>

      <View style={styles.header}>
        <AppText accessibilityRole="header" variant="title">
          {appLock.screen.title(authenticationName)}
        </AppText>
        <AppText color="text2" variant="body">
          {appLock.screen.description(authenticationName)}
        </AppText>
      </View>

      <ListGroup>
        <ListRow
          right={
            <Switch
              accessibilityHint={appLock.screen.onOpenSubtitle}
              accessibilityLabel={appLock.screen.onOpenTitle}
              disabled={authenticating}
              onValueChange={next => {
                commit(onOpen, next, () => setOnOpen(next));
              }}
              value={onOpen}
            />
          }
          subtitle={appLock.screen.onOpenSubtitle}
          title={appLock.screen.onOpenTitle}
        />
        <ListRow
          right={
            <Switch
              accessibilityHint={appLock.screen.beforeSessionSubtitle}
              accessibilityLabel={appLock.screen.beforeSessionTitle}
              disabled={authenticating}
              onValueChange={next => {
                commit(beforeSession, next, () => setBeforeSession(next));
              }}
              value={beforeSession}
            />
          }
          subtitle={appLock.screen.beforeSessionSubtitle}
          title={appLock.screen.beforeSessionTitle}
        />
      </ListGroup>

      <View style={styles.section}>
        <SectionHeader
          right={appLock.screen.sensitiveRight}
          title={appLock.screen.sensitiveTitle}
        />
        <ListGroup>
          {computers.length === 0 ? (
            <View style={styles.emptyRow}>
              <AppText color="text3" variant="body">
                {appLock.screen.emptyHosts}
              </AppText>
            </View>
          ) : (
            computers.map(computer => {
              const sensitive = sensitiveHosts.includes(computer.hostId);
              return (
                <ListRow
                  key={computer.hostId}
                  right={
                    <Switch
                      accessibilityHint={appLock.screen.sensitiveRight}
                      accessibilityLabel={appLock.screen.sensitiveSwitchLabel(
                        computer.name,
                      )}
                      disabled={authenticating}
                      onValueChange={next => {
                        const hosts = toggleSensitive(
                          sensitiveHostsRef.current,
                          computer.hostId,
                          next,
                        );
                        commit(sensitive, next, () => setSensitiveHosts(hosts));
                      }}
                      value={sensitive}
                    />
                  }
                  title={computer.name}
                />
              );
            })
          )}
        </ListGroup>
      </View>

      {/* S15/S20: a fact, not a setting: no chevron, no press. */}
      <View style={styles.factRow} testID="app-lock-key-fact">
        <AppText color="text2" variant="footnote">
          {`${appLock.screen.alwaysTitle}：${appLock.screen.alwaysValue}`}
        </AppText>
      </View>
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: tokens.bg,
  },
  content: {
    padding: space.gutter,
    gap: space.md,
  },
  back: {
    alignSelf: 'flex-start',
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
  },
  header: {
    gap: space.sm,
    marginBottom: space.xs,
  },
  section: {
    gap: space.sm,
    marginTop: space.md,
  },
  pressed: { opacity: press.opacity },
  factRow: {
    paddingHorizontal: space.xs,
  },
  emptyRow: {
    minHeight: 52,
    paddingHorizontal: 14,
    justifyContent: 'center',
  },
});
