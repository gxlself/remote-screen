import { useCallback, useEffect, useRef } from 'react';
import { Linking } from 'react-native';
import { navigationRef } from '../../app/navigation/navigationRef';
import NativeRsDevLog from '../../native/specs/NativeRsDevLog';
import NativeRsFiles from '../../native/specs/NativeRsFiles';
import {
  openIncomingPairFile,
  recordImportFailure,
} from '../pairing/IncomingPairLinks';
import { isContentUrl } from '../pairing/pairLinks';
import { incomingShare, isIncomingFileUrl } from './incomingShare';

const DUPLICATE_WINDOW_MS = 10_000;

function recordFailure(): void {
  // Keep paths and provider error text out of logs: shared URLs may contain
  // private file names. The user can continue in the Files page.
  NativeRsDevLog?.log?.('RS-FILES-INCOMING {"ok":false}');
}

export default function IncomingFiles() {
  const mounted = useRef(true);
  const recentUrls = useRef(new Map<string, number>());
  const pendingNavigation = useRef(false);

  const navigateWhenReady = useCallback(() => {
    if (!pendingNavigation.current || !navigationRef.isReady()) {
      return;
    }
    pendingNavigation.current = false;
    try {
      navigationRef.navigate('MainTabs', { screen: 'Files' });
    } catch {
      pendingNavigation.current = true;
      // A navigation container can become unavailable during a root swap.
      // The ready/state listeners will retry the pending navigation.
    }
  }, []);

  const importIncoming = useCallback(
    async (url: string) => {
      const contentUrl = isContentUrl(url);
      try {
        const result = await NativeRsFiles?.importIncoming(url);
        if (!mounted.current) {
          return;
        }
        if (result == null) {
          if (contentUrl) {
            recordImportFailure();
          }
          return;
        }
        if (contentUrl && result.name.toLowerCase().endsWith('.rspair')) {
          openIncomingPairFile(result.uri);
          return;
        }
        incomingShare.push({
          uri: result.uri,
          name: result.name,
          size: result.size < 0 ? null : result.size,
        });
        pendingNavigation.current = true;
        navigateWhenReady();
      } catch {
        if (contentUrl) {
          recordImportFailure();
        } else {
          recordFailure();
        }
      }
    },
    [navigateWhenReady],
  );

  const receiveUrl = useCallback(
    (url: string | null | undefined) => {
      if (
        !mounted.current ||
        typeof url !== 'string' ||
        (!isContentUrl(url) && !isIncomingFileUrl(url))
      ) {
        return;
      }

      const now = Date.now();
      for (const [recentUrl, receivedAt] of recentUrls.current) {
        if (now - receivedAt >= DUPLICATE_WINDOW_MS) {
          recentUrls.current.delete(recentUrl);
        }
      }
      if (recentUrls.current.has(url)) {
        return;
      }
      recentUrls.current.set(url, now);

      importIncoming(url).catch(() => undefined);
    },
    [importIncoming],
  );

  useEffect(() => {
    mounted.current = true;
    const unsubscribeReady = navigationRef.addListener(
      'ready',
      navigateWhenReady,
    );
    const unsubscribeState = navigationRef.addListener(
      'state',
      navigateWhenReady,
    );

    Linking.getInitialURL()
      .then(receiveUrl)
      .catch(() => undefined);
    const subscription = Linking.addEventListener('url', event => {
      receiveUrl(event.url);
    });

    return () => {
      mounted.current = false;
      pendingNavigation.current = false;
      subscription.remove();
      unsubscribeReady();
      unsubscribeState();
    };
  }, [navigateWhenReady, receiveUrl]);

  return null;
}
