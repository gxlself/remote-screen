import { useCallback, useEffect, useRef, useState } from 'react';
import { useCoreEvent } from '../events/EventsProvider';
import { useCore } from '../../native/rscore/CoreContext';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import type {
  TailnetStatus,
  TailnetRouteStatus,
} from '../../native/rscore/types';
import { useTailnetEnabled } from './enabled';
import { embeddedRunning } from './peers';
import { useTailnetRoute } from './useTailnetRoute';

export type EmbeddedTailnet = {
  available: boolean | null;
  route: TailnetRouteStatus | null;
  status: TailnetStatus | null;
  refresh(): void;
};

type RequestState = {
  core: RsCoreApi;
  available: boolean;
  request: number;
};

export function useEmbeddedTailnet(opts?: {
  pollMs?: number;
}): EmbeddedTailnet {
  const core = useCore();
  // Switched off, the in-app tailnet is never asked for its status.
  const tailnetEnabled = useTailnetEnabled();
  const route = useTailnetRoute();
  const [available, setAvailable] = useState<boolean | null>(null);
  const [status, setStatus] = useState<TailnetStatus | null>(null);
  const requests = useRef<RequestState | null>(null);
  const running = embeddedRunning(route);
  const pollMs = opts?.pollMs;

  const refresh = useCallback(() => {
    const current = requests.current;
    if (current === null || current.core !== core || !current.available) {
      return;
    }
    const request = ++current.request;
    Promise.resolve()
      .then(() => core.tailnet.status())
      .then(next => {
        if (requests.current === current && request === current.request) {
          setStatus(next);
        }
      })
      .catch(() => {
        // Keep the previous status when a native refresh fails.
      });
  }, [core]);

  useEffect(() => {
    const current: RequestState = { core, available: false, request: 0 };
    requests.current = current;
    setAvailable(null);
    setStatus(null);
    if (!tailnetEnabled) {
      setAvailable(false);
      return () => {
        requests.current = null;
      };
    }

    Promise.resolve()
      .then(() => core.tailnet.available())
      .then(value => {
        if (requests.current !== current) {
          return;
        }
        current.available = value;
        setAvailable(value);
        if (value) {
          refresh();
        }
      })
      .catch(() => {
        if (requests.current !== current) {
          return;
        }
        setAvailable(false);
      });

    return () => {
      requests.current = null;
    };
  }, [core, refresh, tailnetEnabled]);

  useCoreEvent('tailnet.state', refresh);

  useEffect(() => {
    if (
      available !== true ||
      pollMs === undefined ||
      !Number.isFinite(pollMs) ||
      pollMs <= 0 ||
      !running
    ) {
      return;
    }
    const timer = setInterval(refresh, pollMs);
    return () => clearInterval(timer);
  }, [available, pollMs, refresh, running]);

  return { available, route, status, refresh };
}
