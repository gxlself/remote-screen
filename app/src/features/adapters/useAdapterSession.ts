import { useCallback, useEffect, useReducer, useRef, useState } from 'react';
import { useCoreEvent } from '../events/EventsProvider';
import { useCore } from '../../native/rscore/CoreContext';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import type {
  AdapterErrorInfo,
  AdapterOpenOpts,
  AdapterPhase,
  AdapterScreen,
  AdapterTarget,
  SessionId,
} from '../../native/rscore/types';
import {
  adapterReducer,
  initialAdapterState,
  isLatin1,
  toCredentials,
  type AdapterViewState,
  type CredInput,
} from './logic';

export type ClipboardSendResult =
  | 'sent'
  | 'empty'
  /** VNC carries Latin-1 only. */
  | 'latin1Only'
  | 'unsupported'
  | 'failed';

export type AdapterSessionResult = {
  state: AdapterViewState;
  screens: AdapterScreen[];
  /** false when the credentials did not reach the adapter. */
  submit(v: CredInput, save: boolean): boolean;
  cancel(): void;
  /** Opens again; `useSaved: false` asks for credentials instead. */
  retry(opts?: { useSaved?: boolean }): void;
  close(): void;
  /** Closes the session and shows it as ended with `error`. */
  end(error: AdapterErrorInfo): void;
  /** Whether the native side still runs this session; null when unknown. */
  isAlive(): boolean | null;
  trustCert(accept: boolean): void;
  typePassword(): boolean;
  sendCtrlAltDel(): boolean;
  sendClipboard(text: string): ClipboardSendResult;
  resize(viewWidth: number, viewHeight: number, scale: number): void;
};

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function closeQuietly(
  adapters: NonNullable<ReturnType<typeof useCore>['adapters']> | undefined,
  id: SessionId,
): void {
  if (adapters === undefined) {
    return;
  }
  try {
    adapters.close(id);
  } catch {
    // Closing is best effort. The native session owns the final cleanup.
  }
}

/** A session that is already open: shown as it was, without opening again. */
export type AdapterResume = {
  sessionId: SessionId;
  phase: AdapterPhase;
  error: AdapterErrorInfo | null;
  width?: number;
  height?: number;
  serverName?: string;
  screens: AdapterScreen[];
};

function resumedState(resume: AdapterResume | undefined): AdapterViewState {
  if (resume === undefined) {
    return initialAdapterState;
  }
  return {
    ...initialAdapterState,
    sessionId: resume.sessionId,
    phase: resume.phase,
    error: resume.error,
    width: resume.width,
    height: resume.height,
    serverName: resume.serverName,
  };
}

export function useAdapterSession(p: {
  target: AdapterTarget;
  opts: AdapterOpenOpts;
  onClipboard?: (text: string) => void;
  /** Picks this session up instead of opening one (V33). */
  resume?: AdapterResume;
  /**
   * Asked on unmount with the session still open; true leaves it running
   * (folded away) instead of closing it.
   */
  keepOnUnmount?: (id: SessionId) => boolean;
}): AdapterSessionResult {
  const core = useCore();
  const [state, dispatch] = useReducer(
    adapterReducer,
    p.resume,
    resumedState,
  );
  const [screens, setScreens] = useState<AdapterScreen[]>(
    () => p.resume?.screens ?? [],
  );
  const idRef = useRef<SessionId | null>(p.resume?.sessionId ?? null);
  const resumeRef = useRef(p.resume !== undefined);
  const keepRef = useRef(p.keepOnUnmount);
  keepRef.current = p.keepOnUnmount;
  const generationRef = useRef(0);
  const mountedRef = useRef(false);
  const ignoredIdsRef = useRef<Set<SessionId>>(new Set());
  const onClipboardRef = useRef(p.onClipboard);
  onClipboardRef.current = p.onClipboard;
  const targetRef = useRef(p.target);
  const optsRef = useRef(p.opts);
  targetRef.current = p.target;
  optsRef.current = p.opts;

  useCoreEvent('adapter.state', event => {
    if (ignoredIdsRef.current.has(event.sessionId)) {
      return;
    }
    dispatch({ type: 'event', event });
  });

  useCoreEvent('adapter.certPrompt', event => {
    if (ignoredIdsRef.current.has(event.sessionId)) {
      return;
    }
    dispatch({ type: 'event', event });
  });

  useCoreEvent('adapter.clipboard', event => {
    if (idRef.current === event.sessionId) {
      onClipboardRef.current?.(event.text);
    }
  });

  useCoreEvent('adapter.screens', event => {
    if (idRef.current === event.sessionId) {
      setScreens(event.screens);
    }
  });

  const startOpen = useCallback(
    (opts: AdapterOpenOpts, reset: boolean) => {
      const adapters = core.adapters;
      if (reset) {
        dispatch({ type: 'reset' });
        setScreens([]);
      }
      idRef.current = null;
      const generation = ++generationRef.current;
      if (adapters === undefined) {
        return;
      }

      let pending: Promise<SessionId>;
      try {
        pending = adapters.open(targetRef.current, opts);
      } catch (error) {
        if (mountedRef.current && generation === generationRef.current) {
          dispatch({ type: 'openFailed', message: errorMessage(error) });
        }
        return;
      }

      Promise.resolve(pending).then(
        id => {
          if (!mountedRef.current || generation !== generationRef.current) {
            ignoredIdsRef.current.add(id);
            closeQuietly(adapters, id);
            return;
          }
          idRef.current = id;
          dispatch({ type: 'opened', id });
        },
        error => {
          if (mountedRef.current && generation === generationRef.current) {
            dispatch({ type: 'openFailed', message: errorMessage(error) });
          }
        },
      );
    },
    [core],
  );

  useEffect(() => {
    mountedRef.current = true;
    const ignoredIds = ignoredIdsRef.current;
    if (resumeRef.current) {
      resumeRef.current = false;
    } else {
      startOpen(optsRef.current, true);
    }
    return () => {
      mountedRef.current = false;
      const id = idRef.current;
      const adapters = core.adapters;
      generationRef.current += 1;
      if (id !== null && keepRef.current?.(id) !== true) {
        ignoredIds.add(id);
        closeQuietly(adapters, id);
      }
      idRef.current = null;
    };
  }, [core, startOpen]);

  const currentAdapters = core.adapters;
  const submit = useCallback(
    (v: CredInput, save: boolean): boolean => {
      const id = idRef.current;
      const adapters = core.adapters;
      if (id === null || adapters === undefined) {
        return false;
      }
      try {
        adapters.submitCredentials(
          id,
          toCredentials(state.credFields, v, save),
        );
        return true;
      } catch {
        core.logs.write('warn', 'adapters', 'submit credentials failed');
        return false;
      }
    },
    [core, state.credFields],
  );

  const cancel = useCallback(() => {
    const id = idRef.current;
    if (id === null || currentAdapters === undefined) {
      return;
    }
    try {
      currentAdapters.cancelCredentials(id);
    } catch {
      // A cancelled/ended native session needs no additional UI action.
    }
  }, [currentAdapters]);

  const close = useCallback(() => {
    const id = idRef.current;
    generationRef.current += 1;
    if (id === null) {
      return;
    }
    ignoredIdsRef.current.add(id);
    closeQuietly(currentAdapters, id);
    idRef.current = null;
  }, [currentAdapters]);

  const retry = useCallback(
    (opts?: { useSaved?: boolean }) => {
      const id = idRef.current;
      if (currentAdapters === undefined) {
        return;
      }
      if (id !== null) {
        ignoredIdsRef.current.add(id);
        if (state.phase !== 'ended') {
          closeQuietly(currentAdapters, id);
        }
      }
      idRef.current = null;
      startOpen({ ...optsRef.current, useSaved: opts?.useSaved ?? true }, true);
    },
    [currentAdapters, startOpen, state.phase],
  );

  const end = useCallback(
    (error: AdapterErrorInfo) => {
      close();
      dispatch({ type: 'lost', error });
    },
    [close],
  );

  const isAlive = useCallback((): boolean | null => {
    const id = idRef.current;
    if (id === null || currentAdapters === undefined) {
      return null;
    }
    try {
      return currentAdapters
        .list()
        .some(summary => summary.sessionId === id && summary.phase !== 'ended');
    } catch {
      return null;
    }
  }, [currentAdapters]);

  const trustCert = useCallback(
    (accept: boolean) => {
      const id = idRef.current;
      if (id === null || currentAdapters === undefined) {
        return;
      }
      try {
        currentAdapters.trustCert(id, accept);
      } catch {
        // The next adapter.state carries the native failure, if any.
      } finally {
        dispatch({ type: 'certAnswered' });
      }
    },
    [currentAdapters],
  );

  const typePassword = useCallback((): boolean => {
    const id = idRef.current;
    if (id === null || currentAdapters === undefined) {
      return false;
    }
    try {
      currentAdapters.typePassword(id);
      return true;
    } catch {
      return false;
    }
  }, [currentAdapters]);

  const sendCtrlAltDel = useCallback((): boolean => {
    const id = idRef.current;
    if (id === null || currentAdapters === undefined) {
      return false;
    }
    try {
      currentAdapters.sendCtrlAltDel(id);
      return true;
    } catch {
      // The adapter may have ended between rendering and the button press.
      return false;
    }
  }, [currentAdapters]);

  const sendClipboard = useCallback(
    (text: string): ClipboardSendResult => {
      if (text === '') {
        return 'empty';
      }
      if (targetRef.current.protocol === 'vnc' && !isLatin1(text)) {
        return 'latin1Only';
      }
      const id = idRef.current;
      if (id === null) {
        return 'failed';
      }
      try {
        core.sessions.clipboardSend(id, text);
        return 'sent';
      } catch (error) {
        return error instanceof RsCoreError && error.kind === 'unsupported'
          ? 'unsupported'
          : 'failed';
      }
    },
    [core],
  );

  const resize = useCallback(
    (viewWidth: number, viewHeight: number, scale: number) => {
      const id = idRef.current;
      if (id === null || currentAdapters === undefined) {
        return;
      }
      try {
        currentAdapters.resize(id, viewWidth, viewHeight, scale);
      } catch {
        // Resize is best effort and must not interrupt the session page.
      }
    },
    [currentAdapters],
  );

  return {
    state,
    screens,
    submit,
    cancel,
    retry,
    close,
    end,
    isAlive,
    trustCert,
    typePassword,
    sendCtrlAltDel,
    sendClipboard,
    resize,
  };
}
