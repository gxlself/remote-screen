import { useMemo } from 'react';
import type { RootScreenProps } from '../../app/navigation/routes';
import { useToast } from '../../components/Toast';
import type { ToastState } from '../../components/Toast';
import { pickAddress } from '../../features/fallback/remoteFallback';
import type { FallbackGuideState } from '../../features/fallback/useRemoteFallback';
import { useRemoteFallback } from '../../features/fallback/useRemoteFallback';
import { sessionEnded } from '../../i18n/zh/sessionEnded';
import { usePlatform } from '../../native/platform/PlatformContext';
import type { Computer } from '../../native/rscore/types';
import type { EndActions } from './boardTypes';

export type UseEndActionsProps = {
  route: RootScreenProps<'SessionEnded'>['route'];
  navigation: RootScreenProps<'SessionEnded'>['navigation'];
  host: Computer | undefined;
};

export type EndActionsResult = EndActions & {
  toastState: ToastState | null;
  hideToast(): void;
  guide: FallbackGuideState;
  closeGuide(): void;
};

export function useEndActions({
  route,
  navigation,
  host,
}: UseEndActionsProps): EndActionsResult {
  const platform = usePlatform();
  const { toast: toastState, show, hide } = useToast();
  const { open: openFallback, guide, closeGuide } = useRemoteFallback();
  const { hostId } = route.params;

  const actions = useMemo<EndActions>(() => {
    const notify = (message: string) => show(message);

    return {
      retry(opts) {
        const params: { hostId: string; viewOnly?: boolean } = { hostId };
        if (opts?.viewOnly !== undefined) {
          params.viewOnly = opts.viewOnly;
        }
        navigation.replace('Session', params);
      },
      openSession(otherHostId) {
        navigation.replace('Session', { hostId: otherHostId });
      },
      back() {
        navigation.goBack();
      },
      openTerminal(serverId) {
        navigation.replace('Terminal', { serverId });
      },
      async openScreenSharing() {
        const address = host === undefined ? null : pickAddress(host.addresses);
        if (host === undefined || address === null) {
          notify(sessionEnded.screenShareNoAddress(host?.os ?? null));
          return;
        }
        const selectedAddress = host.addresses.find(
          candidate => candidate.addr === address,
        );
        const context =
          route.params.code === 190
            ? 'loginWindow'
            : route.params.code === 120
            ? 'noHost'
            : 'fallback';
        const result = await openFallback({
          os: host.os,
          addr: address,
          hostId: host.hostId,
          name: host.name,
          addrKind: selectedAddress?.kind,
          context,
          replace: true,
        });
        if (result === 'failed') {
          notify(sessionEnded.screenShareFailed(host.os));
        }
      },
      copyText(text, message) {
        platform.copyText(text).catch(() => undefined);
        if (message !== undefined) {
          notify(message);
        }
      },
      copyCode() {
        const code = route.params.code;
        const text = code === undefined ? 'RS-E199' : `RS-E${code}`;
        platform.copyText(text).catch(() => undefined);
        notify(sessionEnded.copiedCode);
      },
      pair(tab) {
        navigation.replace('PairAdd', { tab, hostId });
      },
      openDiagnostics() {
        navigation.navigate('Diagnostics', { hostId });
      },
      toast: notify,
    };
  }, [host, hostId, navigation, openFallback, platform, route.params, show]);

  return {
    ...actions,
    toastState,
    hideToast: hide,
    guide,
    closeGuide,
  };
}
