import { useCallback, useEffect, useState } from 'react';
import { useCore } from '../../native/rscore/CoreContext';
import { usePlatform } from '../../native/platform/PlatformContext';
import { useSetting, useSettingsStore } from '../settings/store';
import { normalizeControlUrl } from './controlUrl';
import { LOGIN_WINDOW_MS, tailnetSwitchStore } from './switchStore';

export type SwitchResult = 'switched' | 'needsControlUrl' | 'unavailable';

export function useEmbeddedSwitch(): {
  available: boolean | null;
  canUndo: boolean;
  switchToEmbedded(): Promise<SwitchResult>;
  saveControlUrlAndSwitch(
    input: string,
  ): Promise<'switched' | 'invalid' | 'unavailable'>;
  undo(): void;
  /** false: neither the browser nor a new login could be started. */
  openLogin(): Promise<boolean>;
} {
  const core = useCore();
  const platform = usePlatform();
  const settings = useSettingsStore();
  const [, setMode] = useSetting('tailnet.mode');
  const [, setControlUrl] = useSetting('tailnet.controlUrl');
  const [available, setAvailable] = useState<boolean | null>(null);
  const previousMode = tailnetSwitchStore(state => state.previousMode);

  useEffect(() => {
    let active = true;
    setAvailable(null);
    Promise.resolve()
      .then(() => core.tailnet.available())
      .then(value => {
        if (active) {
          setAvailable(value);
        }
      })
      .catch(() => {
        if (active) {
          setAvailable(false);
        }
      });
    return () => {
      active = false;
    };
  }, [core]);

  const switchToEmbedded = useCallback(async (): Promise<SwitchResult> => {
    let isAvailable: boolean;
    try {
      isAvailable = await core.tailnet.available();
    } catch {
      isAvailable = false;
    }
    setAvailable(isAvailable);
    if (!isAvailable) {
      return 'unavailable';
    }

    const currentMode = settings.get('tailnet.mode');
    const currentControlUrl = settings.get('tailnet.controlUrl');
    if (currentControlUrl === null) {
      return 'needsControlUrl';
    }
    if (currentMode === 'embedded') {
      return 'switched';
    }

    tailnetSwitchStore.setState({
      previousMode: currentMode,
      loginWindowUntilMs: Date.now() + LOGIN_WINDOW_MS,
      openedAuthUrl: null,
    });
    setMode('embedded');
    return 'switched';
  }, [core, setMode, settings]);

  const saveControlUrlAndSwitch = useCallback(
    async (input: string): Promise<'switched' | 'invalid' | 'unavailable'> => {
      const normalized = normalizeControlUrl(input);
      if (normalized === null) {
        return 'invalid';
      }
      setControlUrl(normalized);
      const result = await switchToEmbedded();
      // Another caller may clear the URL during the availability check.
      return result === 'needsControlUrl' ? 'invalid' : result;
    },
    [setControlUrl, switchToEmbedded],
  );

  const undo = useCallback(() => {
    const previous = tailnetSwitchStore.getState().previousMode;
    if (previous === null) {
      return;
    }
    settings.set('tailnet.mode', previous);
    tailnetSwitchStore.setState({
      previousMode: null,
      loginWindowUntilMs: null,
    });
  }, [settings]);

  const openLogin = useCallback(async (): Promise<boolean> => {
    try {
      const status = await core.tailnet.status();
      if (status.authUrl !== null && status.authUrl !== '') {
        return await platform.openUrl(status.authUrl);
      }
      tailnetSwitchStore.setState({
        loginWindowUntilMs: Date.now() + LOGIN_WINDOW_MS,
        openedAuthUrl: null,
      });
      await core.tailnet.login();
      return true;
    } catch {
      // Never throws; the caller tells the user it did not open.
      return false;
    }
  }, [core, platform]);

  return {
    available,
    canUndo: previousMode !== null,
    switchToEmbedded,
    saveControlUrlAndSwitch,
    undo,
    openLogin,
  };
}
