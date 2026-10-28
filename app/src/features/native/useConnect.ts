import { useCallback, useRef, useState } from 'react';
import { useConnectNavigator } from '../../app/navigation/connectNavigator';
import type { ConnectNavigator } from '../../app/navigation/connectNavigator';
import type { RootStackParamList } from '../../app/navigation/routes';
import { native } from '../../i18n/zh/native';
import { useCore } from '../../native/rscore/CoreContext';
import type { DiscoverOutcome } from '../../native/rscore/types';
import { adaptersAvailable, BUILTIN_PROTOCOLS } from '../adapters/logic';
import {
  useRemoteFallback,
  type FallbackGuideState,
} from '../fallback/useRemoteFallback';
import { useSetting, useSettingsStore } from '../settings/store';
import { DEFAULT_HOST_PREFS } from '../settings/schema';
import { decideRoute, type Route } from './decideRoute';
import type { ComputerEntry } from './types';

export { decideRoute } from './decideRoute';
export type { DecideRouteInput, Route } from './decideRoute';

const PROBE_TIMEOUT_MS = 1500;

function elapsedMs(startedAt: number): number {
  return Math.max(0, Math.round(performance.now() - startedAt));
}

function probeOutcome(): DiscoverOutcome {
  // A failed native call has the same routing semantics as a timeout. The
  // AdapterSession remains responsible for the eventual user-facing error.
  return 'timeout';
}

function logRoute(
  core: ReturnType<typeof useCore>,
  entry: ComputerEntry,
  route: Route,
  via: string,
  startedAt: number,
  outcome: DiscoverOutcome | 'none',
): void {
  try {
    core.logs.write(
      'info',
      'connect',
      JSON.stringify({
        entryKind: entry.kind,
        via,
        route: route.kind,
        outcome,
        elapsedMs: elapsedMs(startedAt),
      }),
    );
  } catch {
    // Logging must not prevent a connection from opening.
  }
}

function navigateRoute(
  route: Route,
  replace: boolean,
  nav: ConnectNavigator,
): void {
  switch (route.kind) {
    case 'session': {
      // The session page says why Host opened instead of the system tool.
      const params: RootStackParamList['Session'] = {
        hostId: route.hostId,
        ...(route.fallback === undefined
          ? {}
          : { notice: native.toast.fellBack(route.fallback) }),
      };
      if (replace) {
        nav.replace('Session', params);
      } else {
        nav.navigate('Session', params);
      }
      return;
    }
    case 'adapter': {
      const params: RootStackParamList['AdapterSession'] = {
        target: route.target,
        ...(route.name === undefined ? {} : { name: route.name }),
        addrKind: route.addrKind,
        context: route.context,
      };
      if (replace) {
        nav.replace('AdapterSession', params);
      } else {
        nav.navigate('AdapterSession', params);
      }
      return;
    }
    case 'enableSharing': {
      const params: RootStackParamList['EnableSharing'] = {
        os: route.os,
        name: route.name,
        entryKey: route.entryKey,
      };
      if (replace) {
        nav.replace('EnableSharing', params);
      } else {
        nav.navigate('EnableSharing', params);
      }
      return;
    }
    case 'fallback':
    case 'probe':
      return;
  }
}

export type ConnectOptions = {
  /** Replace the current screen, so a sheet does not reappear afterwards. */
  replace?: boolean;
  /**
   * Sharing is still off. The enable-sharing guide passes this to say so on
   * its own page instead of opening itself again.
   */
  onSharingOff?: (os: 'macos' | 'windows') => void;
};

type UseConnectResult = {
  connect(entry: ComputerEntry, opts?: ConnectOptions): Promise<void>;
  busy: boolean;
  /** The missing external app's guide; the calling screen renders it. */
  guide: FallbackGuideState;
  closeGuide(): void;
};

/** C7 native-first connection routing for Home and device cards. */
export function useConnect(): UseConnectResult {
  const core = useCore();
  const nav = useConnectNavigator();
  const { open: openFallback, guide, closeGuide } = useRemoteFallback();
  const [prefer] = useSetting('connect.prefer');
  const [hostPrefs] = useSetting('hosts.prefs');
  const settingsStore = useSettingsStore();
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);

  const connect = useCallback(
    async (entry: ComputerEntry, opts?: ConnectOptions): Promise<void> => {
      if (busyRef.current) {
        return;
      }
      busyRef.current = true;
      setBusy(true);
      const startedAt = performance.now();
      let outcome: DiscoverOutcome | 'none' = 'none';
      try {
        // Read the store at tap time as well as subscribing above. This keeps
        // a just-saved per-host preference from waiting for a render turn.
        const currentPrefer = settingsStore.get('connect.prefer') ?? prefer;
        const currentHostPrefs = settingsStore.get('hosts.prefs') ?? hostPrefs;
        const available = adaptersAvailable(core);
        const hostPref =
          entry.kind === 'host'
            ? {
                ...DEFAULT_HOST_PREFS,
                ...currentHostPrefs[entry.computer.hostId],
              }
            : undefined;
        const via =
          entry.kind === 'host'
            ? hostPref?.connectVia === 'auto'
              ? currentPrefer
              : hostPref?.connectVia ?? currentPrefer
            : 'native';
        const route = decideRoute({
          entry,
          adaptersAvailable: available,
          builtinProtocols: BUILTIN_PROTOCOLS,
          connectPrefer: currentPrefer,
          hostPrefs: hostPref,
          discoverAvailable: core.discover !== undefined,
        });

        let finalRoute = route;
        if (route.kind === 'probe') {
          try {
            const result = await core.discover?.probe(
              route.addr,
              route.port,
              route.protocol,
              PROBE_TIMEOUT_MS,
            );
            outcome = result?.outcome ?? 'timeout';
          } catch {
            outcome = probeOutcome();
          }
          finalRoute = decideRoute({
            entry,
            adaptersAvailable: available,
            builtinProtocols: BUILTIN_PROTOCOLS,
            connectPrefer: currentPrefer,
            hostPrefs: hostPref,
            discoverAvailable: true,
            probe: outcome,
          });
        }

        if (finalRoute.kind === 'fallback') {
          await openFallback(finalRoute.target);
        } else if (
          finalRoute.kind === 'enableSharing' &&
          opts?.onSharingOff !== undefined
        ) {
          opts.onSharingOff(finalRoute.os);
        } else {
          navigateRoute(finalRoute, opts?.replace === true, nav);
        }
        logRoute(core, entry, finalRoute, via, startedAt, outcome);
      } finally {
        // Keep the guard set through the current turn. This covers two
        // synchronous button handlers that invoke connect back to back before
        // React has had a chance to render the new busy state.
        await Promise.resolve();
        busyRef.current = false;
        setBusy(false);
      }
    },
    [core, hostPrefs, nav, openFallback, prefer, settingsStore],
  );

  return { connect, busy, guide, closeGuide };
}
