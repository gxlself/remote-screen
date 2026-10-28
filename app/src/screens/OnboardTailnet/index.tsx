import { useEffect, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  AppState,
  PixelRatio,
  StyleSheet,
  View,
} from 'react-native';
import type { AppStateStatus } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import type { RootScreenProps } from '../../app/navigation/routes';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { FormScroll } from '../../components/FormScroll';
import { StatusChip } from '../../components/StatusChip';
import { TextField } from '../../components/TextField';
import { useNetwork } from '../../features/events/EventsProvider';
import { judgeNetwork } from '../../features/network/judge';
import { openFirst, tailscaleUrls } from '../../features/platform/externalApps';
import { useSetting } from '../../features/settings/store';
import { useTailnetEnabled } from '../../features/tailnet/enabled';
import { useEmbeddedSwitch } from '../../features/tailnet/useEmbeddedSwitch';
import { onboarding } from '../../i18n/zh/onboarding';
import { tailnet } from '../../i18n/zh/tailnet';
import { usePlatform } from '../../native/platform/PlatformContext';
import { useCore } from '../../native/rscore/CoreContext';
import type { NetworkStatus } from '../../native/rscore/types';
import { space, tokens } from '../../theme/tokens';
import StepHeader from '../_onboarding/StepHeader';
import ModeCard from './ModeCard';

type RefreshedStatus = {
  status: NetworkStatus;
  atUs: number;
};

/** With Tailscale switched off there is no Tailscale step: go straight on. */
export default function OnboardTailnetScreen(
  props: RootScreenProps<'OnboardTailnet'>,
) {
  const enabled = useTailnetEnabled();
  const { navigation } = props;
  useEffect(() => {
    if (!enabled) {
      navigation.replace('OnboardDiscover');
    }
  }, [enabled, navigation]);
  return enabled ? <TailnetStep {...props} /> : null;
}

function TailnetStep({ navigation }: RootScreenProps<'OnboardTailnet'>) {
  const core = useCore();
  const platform = usePlatform();
  const { available, saveControlUrlAndSwitch } = useEmbeddedSwitch();
  const networkStatus = useNetwork(state => state.status);
  const networkUpdatedAtUs = useNetwork(state => state.updatedAtUs);
  const [, setTailnetMode] = useSetting('tailnet.mode');
  const [refreshed, setRefreshed] = useState<RefreshedStatus | null>(null);
  const [choice, setChoice] = useState<'system' | 'embedded'>('system');
  const [controlUrlText, setControlUrlText] = useState('');
  const [urlError, setUrlError] = useState<string | null>(null);
  const [continuing, setContinuing] = useState(false);
  const [switchError, setSwitchError] = useState<string | null>(null);
  const previousSystemLabel = useRef<string | null>(null);

  useEffect(() => {
    let active = true;
    const subscription = AppState.addEventListener(
      'change',
      (state: AppStateStatus) => {
        if (state !== 'active') {
          return;
        }
        const atUs = Date.now() * 1000;
        core.network
          .status()
          .then(status => {
            if (active) {
              setRefreshed({ status, atUs });
            }
          })
          .catch(() => {
            // A failed foreground refresh keeps the last known status.
          });
      },
    );
    return () => {
      active = false;
      subscription.remove();
    };
  }, [core]);

  const refreshIsNewest =
    refreshed !== null &&
    (networkUpdatedAtUs === null || refreshed.atUs >= networkUpdatedAtUs);
  const status = refreshIsNewest ? refreshed.status : networkStatus;
  const tailnetAddress = judgeNetwork(
    status === null ? null : { ifaces: status.ifaces },
    [],
  ).tailnetAddr;
  const connected = tailnetAddress !== null;
  const detecting = status === null;
  let systemLabel: string = onboarding.tailnet.system.detecting;
  let systemTone: 'neutral' | 'good' | 'relay' = 'neutral';
  let systemDetail: string = onboarding.tailnet.system.detectingDetail;
  if (connected) {
    systemLabel = onboarding.tailnet.system.connected;
    systemTone = 'good';
    systemDetail = onboarding.tailnet.system.connectedDetail(tailnetAddress);
  } else if (!detecting) {
    systemLabel = onboarding.tailnet.system.disconnected;
    systemTone = 'relay';
    systemDetail = onboarding.tailnet.system.disconnectedDetail;
  }

  useEffect(() => {
    // 状态文字在可聚焦的卡片内部，Android 的 live region 不可靠；这里也没有别的
    // live region，因此两端都主动播报一次，不会念两遍。
    if (
      previousSystemLabel.current !== null &&
      previousSystemLabel.current !== systemLabel
    ) {
      AccessibilityInfo.announceForAccessibility(systemLabel);
    }
    previousSystemLabel.current = systemLabel;
  }, [systemLabel]);

  const largeText = PixelRatio.getFontScale() > 2;
  const embeddedAvailable = available === true;

  const footer = (
    <View style={styles.footer}>
      {choice === 'system' && !detecting && !connected ? (
        <Button
          fullWidth
          onPress={() => {
            openFirst(platform, tailscaleUrls()).catch(() => undefined);
          }}
          title={onboarding.tailnet.openTailscale}
          variant="secondary"
        />
      ) : null}
      {switchError === null ? null : (
        <AppText
          accessibilityLiveRegion="polite"
          color="bad"
          testID="onboard-tailnet-error"
          variant="footnote"
        >
          {switchError}
        </AppText>
      )}
      <Button
        disabled={continuing}
        fullWidth
        loading={continuing}
        onPress={() => {
          if (continuing) {
            return;
          }
          setSwitchError(null);
          if (choice === 'embedded') {
            setContinuing(true);
            saveControlUrlAndSwitch(controlUrlText)
              .then(result => {
                setContinuing(false);
                if (result === 'invalid') {
                  setUrlError(tailnet.controlUrl.invalid);
                } else if (result === 'unavailable') {
                  setChoice('system');
                  setSwitchError(onboarding.tailnet.embeddedUnavailable);
                } else {
                  navigation.navigate('OnboardDiscover');
                }
              })
              .catch(() => {
                setContinuing(false);
                setSwitchError(onboarding.tailnet.switchFailed);
              });
            return;
          }
          setTailnetMode('system');
          navigation.navigate('OnboardDiscover');
        }}
        testID="onboard-tailnet-continue"
        title={onboarding.tailnet.continue}
        variant="primaryLarge"
      />
      <AppText
        center
        color="text3"
        style={styles.settingsHint}
        variant="caption"
      >
        {onboarding.tailnet.settingsHint}
      </AppText>
    </View>
  );

  return (
    <SafeAreaView edges={['top', 'bottom']} style={styles.screen}>
      <FormScroll
        contentContainerStyle={styles.content}
        footer={largeText ? null : footer}
        style={styles.form}
        testID="onboard-tailnet"
      >
        <StepHeader step="tailnet" />
        <View style={styles.intro}>
          <AppText accessibilityRole="header" variant="title" weight={700}>
            {onboarding.tailnet.title}
          </AppText>
          <AppText color="text2" variant="body">
            {onboarding.tailnet.body}
          </AppText>
        </View>
        <View
          accessibilityLabel={onboarding.tailnet.modeGroupLabel}
          accessibilityRole="radiogroup"
          style={styles.modes}
        >
          <ModeCard
            description={systemDetail}
            onPress={() => {
              if (embeddedAvailable) {
                setChoice('system');
              }
            }}
            selected={!embeddedAvailable || choice === 'system'}
            status={<StatusChip label={systemLabel} tone={systemTone} />}
            testID="tailnet-mode-system"
            title={onboarding.tailnet.system.title}
          />
          {/* Builds without the in-app tailnet (App Store 1.0) show no card
              for it at all, rather than one that cannot be picked. */}
          {embeddedAvailable ? (
            <ModeCard
              accessibilityLabel={onboarding.tailnet.embedded.title}
              description={onboarding.tailnet.embedded.detail}
              onPress={() => setChoice('embedded')}
              selected={choice === 'embedded'}
              status={
                <StatusChip
                  label={onboarding.tailnet.embedded.beta}
                  tone="track"
                />
              }
              testID="tailnet-mode-embedded"
              title={onboarding.tailnet.embedded.title}
            />
          ) : null}
        </View>
        {embeddedAvailable && choice === 'embedded' ? (
          <View style={styles.controlUrl}>
            <TextField
              autoCapitalize="none"
              autoCorrect={false}
              error={urlError ?? undefined}
              keyboardType="url"
              label={tailnet.controlUrl.label}
              onChangeText={value => {
                setControlUrlText(value);
                if (urlError !== null) {
                  setUrlError(null);
                }
              }}
              placeholder={tailnet.controlUrl.placeholder}
              value={controlUrlText}
            />
            <AppText color="text3" variant="caption">
              {tailnet.controlUrl.note}
            </AppText>
          </View>
        ) : null}
        {largeText ? footer : null}
      </FormScroll>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: tokens.bg,
    paddingHorizontal: space.gutter,
  },
  form: {
    flex: 1,
  },
  content: {
    gap: space.section,
    paddingTop: space.sm,
    paddingBottom: space.section,
  },
  intro: {
    gap: space.md,
  },
  modes: {
    gap: space.md,
  },
  controlUrl: {
    gap: space.sm,
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
