import { useEffect, useRef } from 'react';
import { AppState } from 'react-native';
import type { AppStateStatus } from 'react-native';
import { useCoreEvent } from '../events/EventsProvider';
import { usePlatform } from '../../native/platform/PlatformContext';
import { useCore } from '../../native/rscore/CoreContext';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import type { PlatformServices } from '../../native/platform/PlatformServices';
import type { CoreEventOf } from '../../native/rscore/types';
import { isTailnetEnabled, useTailnetEnabled } from './enabled';
import { tailnetSwitchStore } from './switchStore';

function isAppActive(state: AppStateStatus): boolean {
  return state !== 'background';
}

function useOptionalCore(): RsCoreApi | null {
  try {
    return useCore();
  } catch {
    return null;
  }
}

function useOptionalPlatform(): PlatformServices | null {
  try {
    return usePlatform();
  } catch {
    return null;
  }
}

function useOptionalTailnetEnabled(): boolean {
  try {
    return useTailnetEnabled();
  } catch {
    // No SettingsProvider in the bootstrap trees; read the bound storage.
    return isTailnetEnabled();
  }
}

function useOptionalTailnetEvent(
  handler: (event: CoreEventOf<'tailnet.state'>) => void,
): void {
  try {
    useCoreEvent('tailnet.state', handler);
  } catch {
    // NativeBridges is also mounted in a few bootstrap/test trees before the
    // service providers exist.
  }
}

/**
 * Synchronizes lifecycle hints and handles the short login hand-off window.
 * With Tailscale switched off it does neither: nothing here wakes a tailnet.
 */
export default function TailnetSync(): null {
  const core = useOptionalCore();
  const platform = useOptionalPlatform();
  const enabled = useOptionalTailnetEnabled();
  const unsupported = useRef(false);
  const lastActive = useRef<boolean | null>(null);

  useEffect(() => {
    if (core === null || !enabled) {
      return;
    }
    let mounted = true;
    // Switched back on, the core hears the current state again.
    lastActive.current = null;

    const push = (active: boolean) => {
      if (!mounted || unsupported.current || lastActive.current === active) {
        return;
      }
      lastActive.current = active;
      try {
        core.settings.set('device.appActive', active);
      } catch {
        unsupported.current = true;
        console.warn('device.appActive sync is unsupported');
      }
    };

    push(AppState.currentState !== 'background');
    const subscription = AppState.addEventListener('change', state => {
      if (state === 'active' || state === 'background') {
        push(isAppActive(state));
      }
    });

    return () => {
      mounted = false;
      subscription.remove();
    };
  }, [core, enabled]);

  useOptionalTailnetEvent(event => {
    if (!enabled) {
      return;
    }
    const state = tailnetSwitchStore.getState();
    if (event.state === 'running') {
      tailnetSwitchStore.setState({
        loginWindowUntilMs: null,
        openedAuthUrl: null,
      });
      return;
    }

    if (
      event.state !== 'needsLogin' ||
      platform === null ||
      event.authUrl === null ||
      event.authUrl === '' ||
      state.loginWindowUntilMs === null ||
      Date.now() > state.loginWindowUntilMs ||
      event.authUrl === state.openedAuthUrl
    ) {
      return;
    }

    // Mark the URL before awaiting the platform so a duplicate native event
    // cannot open two browser tabs while the first call is still pending.
    tailnetSwitchStore.setState({
      openedAuthUrl: event.authUrl,
      loginWindowUntilMs: null,
    });
    try {
      platform.openUrl(event.authUrl).catch(() => undefined);
    } catch {
      // Opening an external URL is best-effort.
    }
  });

  return null;
}
