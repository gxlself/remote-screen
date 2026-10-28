import { useState } from 'react';
import {
  PixelRatio,
  Platform,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import type { RootScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { StatusChip } from '../../components/StatusChip';
import { Switch } from '../../components/Switch';
import { Toast, useToast } from '../../components/Toast';
import {
  authName,
  fallbackAuthName,
  useAuthCapability,
} from '../../features/lock/authName';
import { useSetting } from '../../features/settings/store';
import { useTailnetEnabled } from '../../features/tailnet/enabled';
import { onboarding } from '../../i18n/zh/onboarding';
import { usePlatform } from '../../native/platform/PlatformContext';
import { radius, space, tokens } from '../../theme/tokens';
import StepHeader from '../_onboarding/StepHeader';

type SwitchRowProps = {
  title: string;
  subtitle: string;
  value: boolean;
  disabled?: boolean;
  onValueChange: (value: boolean) => void;
  testID: string;
};

function SwitchRow({
  title,
  subtitle,
  value,
  disabled = false,
  onValueChange,
  testID,
}: SwitchRowProps) {
  return (
    <View style={styles.switchRow}>
      <View style={styles.switchCopy}>
        <AppText variant="bodyStrong" weight={600}>
          {title}
        </AppText>
        <AppText color="text2" variant="footnote">
          {subtitle}
        </AppText>
      </View>
      <Switch
        accessibilityLabel={title}
        disabled={disabled}
        onValueChange={onValueChange}
        testID={testID}
        value={value}
      />
    </View>
  );
}

export default function OnboardProtectScreen({
  navigation,
}: RootScreenProps<'OnboardProtect'>) {
  const platform = usePlatform();
  const capability = useAuthCapability();
  const authenticationName = authName(capability);
  const [lockOnOpen, setLockOnOpen] = useSetting('lock.onOpen');
  const [notifyEnabled, setNotifyEnabled] = useSetting('notify.enabled');
  const [, setOnboardingDone] = useSetting('onboarding.done');
  const tailnetEnabled = useTailnetEnabled();
  const [authPending, setAuthPending] = useState(false);
  const [notifyPending, setNotifyPending] = useState(false);
  const [localNetworkGranted, setLocalNetworkGranted] = useState(false);
  const [localNetworkPending, setLocalNetworkPending] = useState(false);
  const { toast, show, hide } = useToast();
  const largeText = PixelRatio.getFontScale() > 2;

  const changeLockOnOpen = async (enabled: boolean) => {
    if (!enabled) {
      setLockOnOpen(false);
      return;
    }
    if (authenticationName === null) {
      show(onboarding.protect.faceId.unavailable(fallbackAuthName()), {
        tone: 'bad',
      });
      return;
    }
    setAuthPending(true);
    try {
      const result = await platform.authenticate(
        onboarding.protect.faceId.authReason(authenticationName),
      );
      if (result === 'ok') {
        setLockOnOpen(true);
      } else if (result === 'unavailable') {
        show(onboarding.protect.faceId.unavailable(authenticationName), {
          tone: 'bad',
        });
      }
    } catch {
      // Authentication failures leave the existing off state in place.
    } finally {
      setAuthPending(false);
    }
  };

  const changeNotify = async (enabled: boolean) => {
    if (!enabled) {
      setNotifyEnabled(false);
      return;
    }
    setNotifyPending(true);
    try {
      const result = await platform.requestNotifications();
      if (result === 'granted') {
        setNotifyEnabled(true);
      }
    } catch {
      // A failed permission request leaves notifications disabled.
    } finally {
      setNotifyPending(false);
    }
  };

  const requestLocalNetwork = async () => {
    setLocalNetworkPending(true);
    try {
      const result = await platform.requestLocalNetwork();
      if (result === 'granted') {
        setLocalNetworkGranted(true);
      }
    } catch {
      // Keep the permission card in its previous state.
    } finally {
      setLocalNetworkPending(false);
    }
  };

  const footer = (
    <View style={styles.footer}>
      <Button
        fullWidth
        onPress={() => navigation.goBack()}
        title={onboarding.protect.back}
        variant="secondary"
      />
      <Button
        fullWidth
        onPress={() => {
          setOnboardingDone(true);
          navigation.reset({
            index: 0,
            routes: [{ name: 'MainTabs' }],
          });
        }}
        title={onboarding.protect.start}
        variant="primaryLarge"
      />
      <AppText
        center
        color="text3"
        style={styles.settingsHint}
        variant="caption"
      >
        {onboarding.protect.settingsHint}
      </AppText>
    </View>
  );

  return (
    <SafeAreaView edges={['top', 'bottom']} style={styles.screen}>
      <Toast onHide={hide} position="top" toast={toast} />
      <ScrollView
        contentContainerStyle={styles.content}
        showsVerticalScrollIndicator={false}
        testID="onboard-protect-scroll"
      >
        <StepHeader step="protect" />
        <View style={styles.intro}>
          <AppText accessibilityRole="header" variant="title" weight={700}>
            {onboarding.protect.title}
          </AppText>
          <AppText color="text2" variant="body">
            {onboarding.protect.body}
          </AppText>
        </View>
        <View style={styles.settingsCard}>
          <SwitchRow
            disabled={authPending}
            onValueChange={enabled => {
              changeLockOnOpen(enabled);
            }}
            subtitle={onboarding.protect.faceId.subtitle}
            testID="onboard-protect-face-id"
            title={onboarding.protect.faceId.title(
              authenticationName ?? fallbackAuthName(),
            )}
            value={lockOnOpen}
          />
          <View style={styles.divider} />
          <SwitchRow
            disabled={notifyPending}
            onValueChange={enabled => {
              changeNotify(enabled);
            }}
            subtitle={onboarding.protect.notify.subtitle}
            testID="onboard-protect-notify"
            title={onboarding.protect.notify.title}
            value={notifyEnabled}
          />
        </View>
        {Platform.OS === 'android' ? null : (
          <View style={styles.localCard}>
            <View style={styles.localHeader}>
              <View style={styles.localCopy}>
                <AppText color="text3" variant="section">
                  {onboarding.protect.localNetwork.eyebrow}
                </AppText>
                <AppText variant="bodyStrong" weight={600}>
                  {onboarding.protect.localNetwork.title}
                </AppText>
              </View>
              {localNetworkGranted ? (
                <StatusChip
                  label={onboarding.protect.localNetwork.granted}
                  tone="good"
                />
              ) : (
                <Button
                  disabled={localNetworkPending}
                  onPress={() => {
                    requestLocalNetwork();
                  }}
                  title={onboarding.protect.localNetwork.allow}
                  variant="secondary"
                />
              )}
            </View>
            <AppText color="text2" variant="footnote">
              {tailnetEnabled
                ? onboarding.protect.localNetwork.detail
                : onboarding.protect.localNetwork.detailLan}
            </AppText>
            <AppText color="text3" variant="caption">
              {localNetworkGranted
                ? onboarding.protect.localNetwork.hintAfter
                : onboarding.protect.localNetwork.hintBefore}
            </AppText>
          </View>
        )}
        {largeText ? footer : null}
      </ScrollView>
      {largeText ? null : footer}
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: tokens.bg,
    paddingHorizontal: space.gutter,
  },
  content: {
    gap: space.section,
    paddingTop: space.sm,
    paddingBottom: space.section,
  },
  intro: {
    gap: space.md,
  },
  settingsCard: {
    borderRadius: radius.card,
    borderWidth: 1,
    borderColor: tokens.cardLine,
    backgroundColor: tokens.card,
    paddingHorizontal: space.lg,
  },
  switchRow: {
    minHeight: 68,
    flexDirection: 'row',
    alignItems: 'center',
    gap: space.lg,
    paddingVertical: space.md,
  },
  switchCopy: {
    flex: 1,
    gap: 2,
  },
  divider: {
    height: StyleSheet.hairlineWidth,
    backgroundColor: tokens.cardLine,
  },
  localCard: {
    gap: space.md,
    borderRadius: radius.card,
    borderWidth: 1,
    borderColor: tokens.cardLine,
    backgroundColor: tokens.card,
    padding: space.lg,
  },
  localHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: space.md,
  },
  localCopy: {
    flex: 1,
    gap: 2,
  },
  footer: {
    gap: space.md,
    paddingTop: space.md,
    paddingBottom: space.sm,
  },
  settingsHint: {
    marginTop: -space.xs,
  },
});
