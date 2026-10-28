import { useEffect, useRef, useState } from 'react';
import { AccessibilityInfo, Platform, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import BottomSheet from '../../components/BottomSheet';
import { Button } from '../../components/Button';
import { Icon } from '../../components/icons/Icon';
import { ListGroup, ListRow } from '../../components/List';
import { StepList } from '../../components/StepList';
import { useCoreEvent } from '../../features/events/EventsProvider';
import { useSshServers } from '../../features/data/queries';
import {
  authUrlHost,
  checkSteps,
  sshServerName,
} from '../../features/ssh/prompts';
import {
  authIcon,
  useAuthCapability,
  useAuthName,
} from '../../features/lock/authName';
import { sshPrompt } from '../../i18n/zh/sshPrompt';
import { common } from '../../i18n/zh/common';
import { usePlatform } from '../../native/platform/PlatformContext';
import { useCore } from '../../native/rscore/CoreContext';
import { space, tokens } from '../../theme/tokens';
import type { RootScreenProps } from '../../app/navigation/routes';

type ConfirmProps = RootScreenProps<'RootConfirm'>;

function useReduceMotion(): boolean {
  const [reduceMotion, setReduceMotion] = useState(false);

  useEffect(() => {
    let active = true;
    let changed = false;
    const subscription = AccessibilityInfo.addEventListener(
      'reduceMotionChanged',
      value => {
        changed = true;
        setReduceMotion(value);
      },
    );
    AccessibilityInfo.isReduceMotionEnabled()
      .then(value => {
        if (active && !changed) {
          setReduceMotion(value);
        }
      })
      .catch(() => {});
    return () => {
      active = false;
      subscription.remove();
    };
  }, []);

  return reduceMotion;
}

export default function RootConfirmScreen({ navigation, route }: ConfirmProps) {
  const { sshId, serverId, url, user } = route.params;
  const core = useCore();
  const platform = usePlatform();
  const authenticationName = useAuthName();
  const capability = useAuthCapability();
  const servers = useSshServers();
  const [phase, setPhase] = useState<'ask' | 'wait'>('ask');
  const [busy, setBusy] = useState(false);
  const [openFailed, setOpenFailed] = useState(false);
  const closedRef = useRef(false);
  const reduceMotion = useReduceMotion();

  useEffect(() => {
    if (openFailed && Platform.OS === 'ios') {
      AccessibilityInfo.announceForAccessibility(
        sshPrompt.rootConfirm.openFailed,
      );
    }
  }, [openFailed]);

  const name = sshServerName(servers.data, serverId);
  const host = authUrlHost(url);
  const device = platform.deviceInfo().model;
  const steps = checkSteps(
    phase,
    host,
    authenticationName,
    sshPrompt.rootConfirm.steps,
  );

  const finish = () => {
    if (closedRef.current) {
      return;
    }
    closedRef.current = true;
    navigation.goBack();
  };

  useCoreEvent('ssh.state', event => {
    if (
      event.sshId === sshId &&
      (event.state === 'open' ||
        event.state === 'closed' ||
        event.state === 'failed')
    ) {
      finish();
    }
  });

  const close = () => {
    core.ssh.close(sshId);
    finish();
  };

  const openBrowser = async (): Promise<boolean> => {
    const opened = await platform.openUrl(url);
    if (!opened) {
      setOpenFailed(true);
    } else {
      setOpenFailed(false);
    }
    return opened;
  };

  const confirm = async () => {
    if (busy) {
      return;
    }
    setBusy(true);
    try {
      const result = await platform.authenticate(
        sshPrompt.rootConfirm.authReason(user, name),
      );
      if (result !== 'ok' && result !== 'unavailable') {
        return;
      }
      if (closedRef.current) {
        return;
      }
      setPhase('wait');
      await openBrowser();
    } finally {
      setBusy(false);
    }
  };

  const reopen = async () => {
    await openBrowser();
  };

  const copy = async () => {
    await platform.copyText(url);
    AccessibilityInfo.announceForAccessibility(common.copied);
  };

  const stepModels = steps.map(step => ({
    key: step.key,
    title: step.title,
    state: step.state,
  }));

  return (
    <BottomSheet
      footer={
        <View style={styles.actions}>
          {phase === 'ask' ? (
            <Button
              fullWidth
              icon={authIcon(capability)}
              loading={busy}
              onPress={confirm}
              testID="root-confirm-authenticate"
              title={sshPrompt.rootConfirm.confirm(authenticationName)}
              variant="primaryLarge"
            />
          ) : (
            <Button
              fullWidth
              onPress={reopen}
              title={sshPrompt.rootConfirm.reopen}
              variant="secondary"
            />
          )}
          <Button
            fullWidth
            onPress={close}
            title={common.cancel}
            variant="text"
          />
        </View>
      }
      onClose={close}
      leading={
        <View style={styles.leading}>
          <Icon color="accent" name="shieldCheck" size={26} />
        </View>
      }
      scroll
      title={sshPrompt.rootConfirm.title(user, name)}
      testID="root-confirm"
    >
      <View style={styles.content}>
        <AppText color="text2" variant="body">
          {sshPrompt.rootConfirm.body(user)}
        </AppText>

        <ListGroup>
          <ListRow
            title={sshPrompt.rootConfirm.loginAs}
            value={
              <AppText mono variant="mono">
                {sshPrompt.rootConfirm.loginValue(user, name)}
              </AppText>
            }
          />
          <ListRow title={sshPrompt.rootConfirm.thisDevice} value={device} />
        </ListGroup>

        <StepList
          accessibilityLabel={sshPrompt.rootConfirm.stepsLabel}
          reduceMotion={reduceMotion}
          steps={stepModels}
        />

        {openFailed ? (
          <View style={styles.openFailed}>
            <AppText
              accessibilityLiveRegion="polite"
              color="bad"
              variant="caption"
            >
              {sshPrompt.rootConfirm.openFailed}
            </AppText>
            <Button
              align="start"
              onPress={copy}
              title={sshPrompt.rootConfirm.copyLink}
              variant="secondary"
            />
          </View>
        ) : null}
      </View>
    </BottomSheet>
  );
}

const styles = StyleSheet.create({
  leading: {
    width: 48,
    height: 48,
    borderRadius: 14,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: tokens.accentSoft,
  },
  content: {
    gap: space.lg,
  },
  openFailed: {
    gap: space.sm,
  },
  actions: {
    gap: space.sm,
  },
});
