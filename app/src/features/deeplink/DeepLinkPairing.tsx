import { useCallback, useEffect, useRef, useState } from 'react';
import { Alert, Linking } from 'react-native';
import { StackActions } from '@react-navigation/native';
import { Toast, useToast } from '../../components/Toast';
import { deepLink } from '../../i18n/zh/deepLink';
import { pairAdd } from '../../i18n/zh/pairAdd';
import { useCore } from '../../native/rscore/CoreContext';
import { useSetting, useSettingsStore } from '../settings/store';
import { navigationRef } from '../../app/navigation/navigationRef';
import { useLockState } from '../lock/lockState';
import { decidePairLink, isPairLink } from './pairLink';

const DUPLICATE_LINK_WINDOW_MS = 10_000;

type PendingLink = { url: string; confirmed: boolean };

function openPairWaiting(url: string) {
  if (navigationRef.getCurrentRoute()?.name === 'PairAdd') {
    navigationRef.dispatch(
      StackActions.replace('PairWaiting', { qrText: url }),
    );
  } else {
    navigationRef.navigate('PairWaiting', { qrText: url });
  }
}

export default function DeepLinkPairing() {
  const core = useCore();
  const settingsStore = useSettingsStore();
  const [onboardingDone] = useSetting('onboarding.done');
  const locked = useLockState(state => state.locked);
  const { toast, show: showToast } = useToast();
  const [pendingLink, setPendingLink] = useState<PendingLink | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [navigationVersion, setNavigationVersion] = useState(0);
  const recentUrls = useRef(new Map<string, number>());
  const activeConfirmation = useRef<PendingLink | null>(null);

  const receiveUrl = useCallback((url: string | null | undefined) => {
    if (typeof url !== 'string' || !isPairLink(url)) {
      return;
    }
    const now = Date.now();
    for (const [recentUrl, at] of recentUrls.current) {
      if (now - at >= DUPLICATE_LINK_WINDOW_MS) {
        recentUrls.current.delete(recentUrl);
      }
    }
    if (recentUrls.current.has(url)) {
      return;
    }
    recentUrls.current.set(url, now);
    setPendingLink({ url, confirmed: false });
  }, []);

  useEffect(() => {
    let mounted = true;
    let receivedLivePairLink = false;
    Linking.getInitialURL()
      .then(url => {
        // A delayed initial URL must not replace a newer runtime link.
        if (mounted && !receivedLivePairLink) {
          receiveUrl(url);
        }
      })
      .catch(() => {});
    const subscription = Linking.addEventListener('url', event => {
      if (isPairLink(event.url)) {
        receivedLivePairLink = true;
      }
      receiveUrl(event.url);
    });
    return () => {
      mounted = false;
      subscription.remove();
    };
  }, [receiveUrl]);

  useEffect(() => {
    const recheckNavigation = () => {
      setNavigationVersion(version => version + 1);
    };
    const unsubscribeState = navigationRef.addListener(
      'state',
      recheckNavigation,
    );
    const unsubscribeReady = navigationRef.addListener(
      'ready',
      recheckNavigation,
    );
    return () => {
      unsubscribeState();
      unsubscribeReady();
    };
  }, []);

  useEffect(() => {
    if (
      pendingLink === null ||
      confirming ||
      activeConfirmation.current !== null ||
      !navigationRef.isReady()
    ) {
      return;
    }

    const topRoute = navigationRef.getCurrentRoute()?.name;
    if (decidePairLink({ onboardingDone, locked, topRoute }) === 'wait') {
      return;
    }

    const { url } = pendingLink;
    recentUrls.current.set(url, Date.now());
    setPendingLink(null);
    if (pendingLink.confirmed) {
      openPairWaiting(url);
      return;
    }

    let target: ReturnType<typeof core.pairing.parseQr>;
    try {
      target = core.pairing.parseQr(url);
    } catch {
      target = null;
    }
    if (target === null) {
      showToast(pairAdd.scan.invalid, { tone: 'bad' });
      return;
    }

    const addressText = target.addrs.join('、');
    const confirmation = pendingLink;
    activeConfirmation.current = confirmation;
    setConfirming(true);
    const closeConfirmation = () => {
      if (activeConfirmation.current !== confirmation) {
        return false;
      }
      activeConfirmation.current = null;
      setConfirming(false);
      return true;
    };
    Alert.alert(deepLink.title(target.name), deepLink.message(addressText), [
      { text: deepLink.cancel, style: 'cancel', onPress: closeConfirmation },
      {
        text: deepLink.pair,
        onPress: () => {
          if (!closeConfirmation()) {
            return;
          }
          // The App can lock or change routes while the alert is open.
          if (
            !navigationRef.isReady() ||
            decidePairLink({
              onboardingDone: settingsStore.get('onboarding.done'),
              locked: useLockState.getState().locked,
              topRoute: navigationRef.getCurrentRoute()?.name,
            }) === 'wait'
          ) {
            setPendingLink(latest => latest ?? { url, confirmed: true });
            return;
          }
          openPairWaiting(url);
        },
      },
    ]);
  }, [
    core,
    confirming,
    locked,
    navigationVersion,
    onboardingDone,
    pendingLink,
    settingsStore,
    showToast,
  ]);

  return <Toast onHide={() => {}} toast={toast} />;
}
