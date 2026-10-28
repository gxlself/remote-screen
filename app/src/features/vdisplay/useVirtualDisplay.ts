import { useCallback, useEffect } from 'react';
import { useCoreEvent } from '../events/EventsProvider';
import { useCore } from '../../native/rscore/CoreContext';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import type { CoreEventOf, SessionDisplay } from '../../native/rscore/types';
import { useVdisplayStore } from './store';
import { statusText, VDISPLAY_TIMEOUT_MS, virtualDisplayOpts } from './request';
import type { ScreenInfo } from './request';

export type VdState =
  | { kind: 'idle' }
  | { kind: 'pending'; requestId: number }
  | { kind: 'error'; text: string };

type VdResult =
  | { kind: 'created'; display: SessionDisplay }
  | { kind: 'error'; text: string };

/** 用一条 `session.vdisplay` 事件了结这个会话还在等的请求；不相干的事件返回 null。 */
function resolveEvent(
  sessionId: string | null,
  event: CoreEventOf<'session.vdisplay'>,
): VdResult | null {
  if (sessionId === null || event.sessionId !== sessionId) {
    return null;
  }
  const store = useVdisplayStore.getState();
  const pending = store.pending[sessionId];
  if (pending === undefined || pending.requestId !== event.requestId) {
    return null;
  }
  store.setPending(sessionId, null);
  if (event.status === 'ok' && event.display !== undefined) {
    store.setFresh(sessionId, event.display.id);
    store.setOwned(sessionId, event.display.id);
    return { kind: 'created', display: event.display };
  }
  return { kind: 'error', text: statusText(event.status) };
}

/** 等待超过期限时了结请求，返回超时文案；请求已经换了或已了结返回 null。 */
function resolveTimeout(sessionId: string, requestId: number): string | null {
  const store = useVdisplayStore.getState();
  if (store.pending[sessionId]?.requestId !== requestId) {
    return null;
  }
  store.setPending(sessionId, null);
  return statusText('timeout');
}

/** 在 enabled 时接住这个会话的新建结果和超时。 */
function useVdisplayResult(opts: {
  sessionId: string | null;
  enabled: boolean;
  onResult(result: VdResult): void;
}): void {
  const { sessionId, enabled, onResult } = opts;
  const pending = useVdisplayStore(store =>
    sessionId === null ? undefined : store.pending[sessionId],
  );

  useCoreEvent('session.vdisplay', event => {
    if (!enabled) {
      return;
    }
    const result = resolveEvent(sessionId, event);
    if (result !== null) {
      onResult(result);
    }
  });

  const requestId = pending?.requestId;
  const deadlineMs = pending?.deadlineMs;
  useEffect(() => {
    if (
      !enabled ||
      sessionId === null ||
      requestId === undefined ||
      deadlineMs === undefined
    ) {
      return undefined;
    }
    const timer = setTimeout(() => {
      const text = resolveTimeout(sessionId, requestId);
      if (text !== null) {
        onResult({ kind: 'error', text });
      }
    }, Math.max(0, deadlineMs - Date.now()));
    return () => clearTimeout(timer);
    // onResult 每次渲染都会变；计时只跟请求走。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [deadlineMs, enabled, requestId, sessionId]);
}

/**
 * 显示器面板关着时，由会话页接住还没回来的新建结果并给提示（R07）。
 * 面板开着时由面板自己处理，这里不动。
 */
export function useVirtualDisplayWatcher(opts: {
  sessionId: string | null;
  enabled: boolean;
  onCreated(display: SessionDisplay): void;
  onError(text: string): void;
}): void {
  const { onCreated, onError } = opts;
  useVdisplayResult({
    sessionId: opts.sessionId,
    enabled: opts.enabled,
    onResult: result => {
      if (result.kind === 'created') {
        onCreated(result.display);
      } else {
        onError(result.text);
      }
    },
  });
}

export function useVirtualDisplay(opts: {
  sessionId: string | null;
  displays: SessionDisplay[];
  onCreated(display: SessionDisplay): void;
}): {
  state: VdState;
  ownedId: number | null;
  create(screen: ScreenInfo): void;
  release(displayId: number): void;
} {
  const core = useCore();
  const { sessionId, displays, onCreated } = opts;
  const owned = useVdisplayStore(store =>
    sessionId === null ? null : store.owned[sessionId] ?? null,
  );
  const fresh = useVdisplayStore(store =>
    sessionId === null ? null : store.fresh[sessionId] ?? null,
  );
  const pending = useVdisplayStore(store =>
    sessionId === null ? undefined : store.pending[sessionId],
  );
  const error = useVdisplayStore(store =>
    sessionId === null ? undefined : store.errors[sessionId],
  );
  const ownedId =
    owned !== null && displays.some(display => display.id === owned)
      ? owned
      : null;
  const state: VdState =
    pending !== undefined
      ? { kind: 'pending', requestId: pending.requestId }
      : error !== undefined
      ? { kind: 'error', text: error }
      : { kind: 'idle' };

  useEffect(() => {
    if (sessionId === null || owned === null || displays.length === 0) {
      return;
    }
    const store = useVdisplayStore.getState();
    if (displays.some(display => display.id === owned)) {
      if (fresh === owned) {
        store.setFresh(sessionId, null);
      }
      return;
    }
    if (fresh === owned) {
      // 刚建好，显示器列表还没跟上。
      return;
    }
    store.clearOwned(sessionId);
  }, [displays, fresh, owned, sessionId]);

  useVdisplayResult({
    sessionId,
    enabled: true,
    onResult: result => {
      if (sessionId === null) {
        return;
      }
      if (result.kind === 'created') {
        onCreated(result.display);
      } else {
        useVdisplayStore.getState().setError(sessionId, result.text);
      }
    },
  });

  // 面板关掉后错误不再保留，下次打开从头开始；还在等的请求留给会话页接住。
  useEffect(
    () => () => {
      if (sessionId !== null) {
        useVdisplayStore.getState().setError(sessionId, null);
      }
    },
    [sessionId],
  );

  const create = useCallback(
    (screen: ScreenInfo) => {
      if (sessionId === null) {
        return;
      }
      const store = useVdisplayStore.getState();
      if (store.pending[sessionId] !== undefined) {
        return;
      }

      let requestId: number;
      try {
        requestId = core.sessions.createVirtualDisplay(
          sessionId,
          virtualDisplayOpts(screen),
        );
      } catch (caught) {
        store.setError(
          sessionId,
          caught instanceof RsCoreError && caught.kind === 'unsupported'
            ? statusText('unsupported')
            : statusText('failed'),
        );
        return;
      }
      store.setError(sessionId, null);
      store.setPending(sessionId, {
        requestId,
        deadlineMs: Date.now() + VDISPLAY_TIMEOUT_MS,
      });
    },
    [core, sessionId],
  );

  const release = useCallback(
    (displayId: number) => {
      if (sessionId === null) {
        return;
      }
      core.sessions.releaseVirtualDisplay(sessionId, displayId);
      useVdisplayStore.getState().clearOwned(sessionId);
    },
    [core, sessionId],
  );

  return { state, ownedId, create, release };
}
