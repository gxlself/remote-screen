import { useCallback, useEffect, useRef } from 'react';
import { Linking } from 'react-native';
import { navigationRef } from '../../app/navigation/navigationRef';
import NativeRsDevLog from '../../native/specs/NativeRsDevLog';
import NativeRsFiles from '../../native/specs/NativeRsFiles';
import {
  classifyPairLink,
  isContentUrl,
  type IncomingPairLink,
} from './pairLinks';

const DUPLICATE_WINDOW_MS = 2000;
let registeredPairFileNavigator: ((link: IncomingPairLink) => void) | undefined;
let pendingExternalPairFile: IncomingPairLink | undefined;

export function recordImportFailure(): void {
  // Keep source paths and provider error text out of logs.
  NativeRsDevLog?.log?.('RS-PAIRFILE-INCOMING {"ok":false}');
}

function navigateToPairFile(link: IncomingPairLink): void {
  try {
    navigationRef.navigate('PairFile', link);
  } catch {
    // Navigation is best-effort; link handling must never break app startup.
  }
}

/** Opens an already imported pairing file with its local file URI. */
export function openIncomingPairFile(uri: string): void {
  const link = { uri };
  if (registeredPairFileNavigator !== undefined) {
    registeredPairFileNavigator(link);
    return;
  }
  if (navigationRef.isReady()) {
    navigateToPairFile(link);
  } else {
    pendingExternalPairFile = link;
  }
}

/** 处理打开的 `.rspair` 文件与 `rsp://pairfile` 深链。 */
export default function IncomingPairLinks() {
  const mounted = useRef(false);
  const pendingLink = useRef<IncomingPairLink | null>(null);
  const recentUrls = useRef(new Map<string, number>());

  const navigateWhenReady = useCallback((link: IncomingPairLink) => {
    if (!mounted.current) {
      return;
    }
    if (navigationRef.isReady()) {
      navigateToPairFile(link);
    } else {
      pendingLink.current = link;
    }
  }, []);

  const importPairFile = useCallback(
    async (uri: string) => {
      let link: IncomingPairLink = { uri };
      try {
        const imported = await NativeRsFiles?.importIncoming(uri);
        if (imported !== null && imported !== undefined) {
          link = { uri: imported.uri };
        }
      } catch {
        recordImportFailure();
      }
      if (mounted.current) {
        navigateWhenReady(link);
      }
    },
    [navigateWhenReady],
  );

  const receiveUrl = useCallback(
    (url: string | null | undefined) => {
      if (!mounted.current || typeof url !== 'string') {
        return;
      }
      if (isContentUrl(url)) {
        return;
      }
      const link = classifyPairLink(url);
      if (link === null) {
        return;
      }
      const now = Date.now();
      for (const [recentUrl, at] of recentUrls.current) {
        if (now - at >= DUPLICATE_WINDOW_MS) {
          recentUrls.current.delete(recentUrl);
        }
      }
      if (recentUrls.current.has(url)) {
        return;
      }
      recentUrls.current.set(url, now);
      if ('text' in link) {
        navigateWhenReady(link);
      } else {
        importPairFile(link.uri).catch(() => undefined);
      }
    },
    [importPairFile, navigateWhenReady],
  );

  useEffect(() => {
    mounted.current = true;
    registeredPairFileNavigator = navigateWhenReady;
    if (pendingExternalPairFile !== undefined) {
      const link = pendingExternalPairFile;
      pendingExternalPairFile = undefined;
      navigateWhenReady(link);
    }
    const unsubscribeState = navigationRef.addListener('state', () => {
      if (!mounted.current || pendingLink.current === null) {
        return;
      }
      if (!navigationRef.isReady()) {
        return;
      }
      const link = pendingLink.current;
      pendingLink.current = null;
      navigateToPairFile(link);
    });

    Linking.getInitialURL()
      .then(url => {
        receiveUrl(url);
      })
      .catch(() => {});
    const subscription = Linking.addEventListener('url', event => {
      receiveUrl(event.url);
    });

    return () => {
      mounted.current = false;
      pendingLink.current = null;
      if (registeredPairFileNavigator === navigateWhenReady) {
        registeredPairFileNavigator = undefined;
      }
      subscription.remove();
      unsubscribeState();
    };
  }, [navigateWhenReady, receiveUrl]);

  return null;
}
