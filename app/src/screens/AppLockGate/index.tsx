import { useCallback, useEffect, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  AppState,
  BackHandler,
  Modal,
  Platform,
  ScrollView,
  StyleSheet,
  View,
  type AppStateStatus,
} from 'react-native';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Icon } from '../../components/icons/Icon';
import { selectLive, useSessions } from '../../features/events/EventsProvider';
import { formatClock } from '../../features/format/duration';
import {
  selectLiveSsh,
  useSshSessions,
} from '../../features/sessions/sshStore';
import {
  authIcon,
  authName,
  fallbackAuthName,
  useAuthCapability,
} from '../../features/lock/authName';
import { lockOnLaunch, lockOnResume } from '../../features/lock/policy';
import { useLockState } from '../../features/lock/lockState';
import { useSetting } from '../../features/settings/store';
import { usePlatform } from '../../native/platform/PlatformContext';
import { appLock, devicePasscodeAuthName } from '../../i18n/zh/appLock';
import { tokens } from '../../theme/tokens';

export default function AppLockGate() {
  const platform = usePlatform();
  const capability = useAuthCapability();
  const authenticationName = authName(capability);
  const passcodeOnly = capability.biometry === 'none';
  const [onboardingDone] = useSetting('onboarding.done');
  const [onOpen] = useSetting('lock.onOpen');
  const liveSessions = useSessions(selectLive);
  const liveTerminals = useSshSessions(state => selectLiveSsh(state).length);
  const [locked, setLocked] = useState(() =>
    lockOnLaunch({ onboardingDone, onOpen }),
  );
  const [failed, setFailed] = useState(false);
  const [authenticating, setAuthenticating] = useState(false);
  const [nowUs, setNowUs] = useState(() => Date.now() * 1000);
  const backgroundAtMs = useRef<number | null>(null);
  const authInFlight = useRef(false);
  const autoPrompted = useRef(false);
  const mounted = useRef(true);
  const announcedFailure = useRef(false);

  useEffect(() => {
    useLockState.getState().setLocked(locked);
  }, [locked]);

  const activeSession = liveSessions.at(0) ?? null;
  const backgroundCount = liveSessions.length + liveTerminals;

  useEffect(() => {
    return () => {
      mounted.current = false;
    };
  }, []);

  useEffect(() => {
    const subscription = AppState.addEventListener(
      'change',
      (nextState: AppStateStatus) => {
        if (nextState === 'background') {
          backgroundAtMs.current = Date.now();
          return;
        }
        if (nextState !== 'active') {
          return;
        }
        const since = backgroundAtMs.current;
        if (since === null) {
          return;
        }
        if (
          lockOnResume({
            onboardingDone,
            onOpen,
            backgroundAtMs: since,
            nowMs: Date.now(),
          })
        ) {
          setLocked(true);
        }
        backgroundAtMs.current = null;
      },
    );
    return () => subscription.remove();
  }, [onOpen, onboardingDone]);

  useEffect(() => {
    if (!locked || activeSession === null) {
      return;
    }
    setNowUs(Date.now() * 1000);
    const timer = setInterval(() => {
      setNowUs(Date.now() * 1000);
    }, 1000);
    return () => clearInterval(timer);
  }, [activeSession, locked]);

  const verify = useCallback(() => {
    if (authInFlight.current) {
      return;
    }
    authInFlight.current = true;
    setAuthenticating(true);
    setFailed(false);
    platform
      .authenticate(appLock.gate.authenticateReason)
      .then(result => {
        if (!mounted.current) {
          return;
        }
        if (result === 'ok' || result === 'unavailable') {
          setLocked(false);
        } else if (result === 'failed') {
          setFailed(true);
        }
      })
      .catch(() => {
        if (mounted.current) {
          setFailed(true);
        }
      })
      .finally(() => {
        authInFlight.current = false;
        if (mounted.current) {
          setAuthenticating(false);
        }
      });
  }, [platform]);

  useEffect(() => {
    if (locked) {
      if (!autoPrompted.current) {
        autoPrompted.current = true;
        verify();
      }
      return;
    }
    autoPrompted.current = false;
  }, [locked, verify]);

  useEffect(() => {
    if (!failed) {
      announcedFailure.current = false;
      return;
    }
    if (Platform.OS === 'ios' && !announcedFailure.current) {
      announcedFailure.current = true;
      AccessibilityInfo.announceForAccessibility(appLock.gate.failed);
    }
  }, [failed]);

  const elapsedSeconds =
    activeSession === null
      ? 0
      : Math.floor((nowUs - activeSession.startedAt) / 1_000_000);

  return (
    <Modal
      animationType="none"
      onRequestClose={() => {
        if (Platform.OS === 'android') {
          BackHandler.exitApp();
        }
      }}
      presentationStyle="overFullScreen"
      testID="app-lock-gate"
      transparent={false}
      visible={locked}
    >
      <View accessibilityViewIsModal style={styles.overlay}>
        <ScrollView
          contentContainerStyle={styles.scrollContent}
          style={styles.scroll}
          testID="app-lock-gate-scroll"
        >
          <View style={styles.content}>
            <View style={styles.iconBadge}>
              <Icon color="text" name={authIcon(capability)} size={34} />
            </View>
            <AppText
              accessibilityRole="header"
              center
              style={styles.title}
              weight={600}
            >
              {appLock.gate.title}
            </AppText>
            <AppText center color="text2" variant="body">
              {appLock.gate.subtitle}
            </AppText>

            {backgroundCount === 0 ? null : (
              <View style={styles.sessionBlock}>
                <View style={styles.sessionCard}>
                  <AppText variant="button" weight={600}>
                    {appLock.gate.sessionTitle(
                      liveSessions.length,
                      liveTerminals,
                    )}
                  </AppText>
                  {activeSession === null ? null : (
                    <AppText color="text2" mono style={styles.sessionClock}>
                      {`${activeSession.hostName} · ${formatClock(
                        elapsedSeconds,
                      )}`}
                    </AppText>
                  )}
                </View>
                <AppText
                  color="text3"
                  style={styles.sessionNote}
                  variant="caption"
                >
                  {appLock.gate.sessionInfo}
                </AppText>
              </View>
            )}

            {failed ? (
              <AppText
                accessibilityLiveRegion={
                  Platform.OS === 'android' ? 'polite' : undefined
                }
                color="bad"
                variant="footnote"
              >
                {appLock.gate.failed}
              </AppText>
            ) : null}

            <Button
              fullWidth
              icon={authIcon(capability)}
              loading={authenticating}
              onPress={verify}
              title={appLock.gate.unlock(
                passcodeOnly
                  ? devicePasscodeAuthName
                  : authenticationName ?? fallbackAuthName(),
              )}
              variant="primaryLarge"
            />
            {/* S13: the system prompt offers the passcode itself; a second
                button here would only raise the same Face ID again. */}
            {passcodeOnly ? null : (
              <AppText center color="text3" variant="caption">
                {appLock.gate.devicePasscodeHint}
              </AppText>
            )}
          </View>
        </ScrollView>
      </View>
    </Modal>
  );
}

const styles = StyleSheet.create({
  overlay: {
    flex: 1,
    backgroundColor: tokens.bg,
    paddingHorizontal: 32,
  },
  scroll: {
    flex: 1,
  },
  scrollContent: {
    flexGrow: 1,
    justifyContent: 'center',
  },
  content: {
    width: '100%',
    alignItems: 'center',
    gap: 16,
  },
  iconBadge: {
    width: 72,
    height: 72,
    borderRadius: 22,
    backgroundColor: tokens.raised,
    alignItems: 'center',
    justifyContent: 'center',
  },
  title: {
    fontSize: 22,
    lineHeight: 28,
  },
  sessionBlock: {
    alignSelf: 'stretch',
  },
  sessionCard: {
    borderRadius: 16,
    backgroundColor: tokens.card,
    padding: 14,
    gap: 4,
  },
  sessionClock: {
    fontSize: 14,
    lineHeight: 20,
  },
  sessionNote: {
    marginTop: 12,
  },
});
