import { useCallback, useState } from 'react';
import { StackActions } from '@react-navigation/native';
import type { AdapterContext, Computer } from '../../native/rscore/types';
import { navigationRef } from '../../app/navigation/navigationRef';
import { useCore } from '../../native/rscore/CoreContext';
import { usePlatform } from '../../native/platform/PlatformContext';
import {
  adaptersAvailable,
  adapterProtocolFor,
  BUILTIN_PROTOCOLS,
  targetForAddress,
  type AdapterAddrKind,
} from '../adapters/logic';
import { currentOs } from '../platform/externalApps';
import { fallbackKind, fallbackUrl, type FallbackKind } from './remoteFallback';

export type RemoteFallbackTarget = {
  os: Computer['os'];
  addr: string;
  port?: number;
  /** M3 V4：下面几项只给内置适配器用。 */
  hostId?: string;
  name?: string;
  addrKind?: AdapterAddrKind;
  context?: AdapterContext;
  /** true = 跳过内置，直接交给外部 App（适配器错误页的「用其他 App 打开」）。 */
  preferExternal?: boolean;
  /** true = 内置会话替换当前页（从断开页进来时总是替换）。 */
  replace?: boolean;
};

/** Pages that should not stay under the adapter session (V06). */
const REPLACED_ROUTES: readonly string[] = ['SessionEnded'];

function shouldReplace(target: RemoteFallbackTarget): boolean {
  if (target.replace === true) {
    return true;
  }
  const route = navigationRef.current?.getCurrentRoute();
  return route !== undefined && REPLACED_ROUTES.includes(route.name);
}

export type RemoteFallbackResult =
  | 'adapter'
  | 'opened'
  | 'notInstalled'
  | 'failed';

export type FallbackGuideState = { kind: FallbackKind } | null;

export type UseRemoteFallbackResult = {
  open(target: RemoteFallbackTarget): Promise<RemoteFallbackResult>;
  guide: FallbackGuideState;
  closeGuide(): void;
};

export function useRemoteFallback(): UseRemoteFallbackResult {
  const core = useCore();
  const platform = usePlatform();
  const [guide, setGuide] = useState<FallbackGuideState>(null);

  const closeGuide = useCallback(() => setGuide(null), []);

  const open = useCallback(
    async (target: RemoteFallbackTarget): Promise<RemoteFallbackResult> => {
      const kind = fallbackKind(target.os);
      const protocol = adapterProtocolFor(target.os);
      if (
        !target.preferExternal &&
        adaptersAvailable(core) &&
        navigationRef.isReady() &&
        (BUILTIN_PROTOCOLS as readonly string[]).includes(protocol)
      ) {
        const adapterTarget = targetForAddress(
          protocol,
          target.addr,
          target.port,
        );
        if (adapterTarget !== null) {
          if (target.hostId !== undefined) {
            adapterTarget.hostId = target.hostId;
          }
          const addrKind =
            target.addrKind ??
            (target.hostId === undefined ? 'manual' : undefined);
          const context =
            target.context ??
            (target.hostId === undefined ? 'manual' : 'fallback');
          const params = {
            target: adapterTarget,
            ...(target.name === undefined ? {} : { name: target.name }),
            ...(addrKind === undefined ? {} : { addrKind }),
            context,
          };
          // Back from the session should not land on a stale error page.
          if (shouldReplace(target)) {
            navigationRef.dispatch(
              StackActions.replace('AdapterSession', params),
            );
          } else {
            navigationRef.navigate('AdapterSession', params);
          }
          core.logs.write(
            'info',
            'fallback',
            JSON.stringify({ kind, result: 'adapter' }),
          );
          return 'adapter';
        }
      }
      const url = fallbackUrl(kind, target.addr, target.port);
      let result: RemoteFallbackResult;

      try {
        if (currentOs() === 'android') {
          // Android 11+ package visibility makes canOpenUrl return false for
          // unlisted schemes, so opening the URL is the only reliable check.
          // An installed app that fails to open is also reported as missing.
          if (await platform.openUrl(url)) {
            result = 'opened';
          } else {
            result = 'notInstalled';
            setGuide({ kind });
          }
        } else if (!(await platform.canOpenUrl(url))) {
          result = 'notInstalled';
          setGuide({ kind });
        } else {
          result = (await platform.openUrl(url)) ? 'opened' : 'failed';
        }
      } catch {
        result = 'failed';
      }

      core.logs.write('info', 'fallback', JSON.stringify({ kind, result }));
      return result;
    },
    [core, platform],
  );

  return { open, guide, closeGuide };
}
