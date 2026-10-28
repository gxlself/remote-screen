import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type JSX,
  type ReactNode,
} from 'react';

import { IpcClient, IpcError } from './client';
import type { EventName, IpcEvents } from './events';
import type { Transport } from './transport';
import type { MethodName, Params, Result } from './types';

const IpcContext = createContext<IpcClient | null>(null);

interface QueryOptions<M extends MethodName> {
  params?: Params<M>;
  refetchOn?: EventName[];
  pollMs?: number;
  enabled?: boolean;
}

type QueryArgs<M extends MethodName> =
  Params<M> extends null
    ? [options?: QueryOptions<M>]
    : [options: QueryOptions<M> & { params: Params<M> }];

interface QueryResult<M extends MethodName> {
  data: Result<M> | undefined;
  error: IpcError | undefined;
  loading: boolean;
  refetch: () => Promise<void>;
  mutate: (next: Result<M> | ((prev: Result<M> | undefined) => Result<M>)) => void;
}

interface LatestQuery<M extends MethodName> {
  client: IpcClient;
  method: M;
  params: Params<M> | undefined;
  enabled: boolean;
}

function parseEvents(key: string): EventName[] {
  return JSON.parse(key) as EventName[];
}

function asIpcError(error: unknown): IpcError {
  if (error instanceof IpcError) {
    return error;
  }
  return new IpcError(-32603, 'transport', String(error), { cause: error });
}

function callMethod<M extends MethodName>(
  client: IpcClient,
  method: M,
  params: Params<M> | undefined,
): Promise<Result<M>> {
  const call = client.call.bind(client) as unknown as (
    method: M,
    params: Params<M> | null,
  ) => Promise<Result<M>>;
  return call(method, params ?? null);
}

export function IpcProvider({
  transport,
  children,
}: {
  transport: Transport;
  children: ReactNode;
}): JSX.Element {
  const client = useMemo(() => new IpcClient(transport), [transport]);
  return <IpcContext.Provider value={client}>{children}</IpcContext.Provider>;
}

export function useIpc(): IpcClient {
  const client = useContext(IpcContext);
  if (client === null) {
    throw new Error('useIpc must be used inside an IpcProvider');
  }
  return client;
}

export function useIpcConnected(): boolean {
  const client = useIpc();
  const [connected, setConnected] = useState(true);

  useEffect(() => {
    let active = true;
    let connectionUpdates = 0;
    const unsubscribe = client.onConnection((next) => {
      connectionUpdates += 1;
      if (active) {
        setConnected(next);
      }
    });

    void client.isConnected().then(
      (next) => {
        if (active && connectionUpdates === 0) {
          setConnected(next);
        }
      },
      () => undefined,
    );

    return () => {
      active = false;
      unsubscribe();
    };
  }, [client]);

  return connected;
}

export function useIpcEvent<E extends EventName>(
  event: E,
  handler: (payload: IpcEvents[E]) => void,
): void {
  const client = useIpc();
  const handlerRef = useRef(handler);

  useEffect(() => {
    handlerRef.current = handler;
  });

  useEffect(() => client.on(event, (payload) => handlerRef.current(payload)), [client, event]);
}

export function useIpcQuery<M extends MethodName>(
  method: M,
  ...args: QueryArgs<M>
): QueryResult<M> {
  const client = useIpc();
  const options = args[0] as QueryOptions<M> | undefined;
  const params = options?.params;
  const enabled = options?.enabled !== false;
  const pollMs = options?.pollMs ?? 0;
  const paramsKey = JSON.stringify(params ?? null);
  const refetchOnKey = JSON.stringify(options?.refetchOn ?? []);
  const connected = useIpcConnected();

  const [data, setData] = useState<Result<M> | undefined>(undefined);
  const [error, setError] = useState<IpcError | undefined>(undefined);
  const [settled, setSettled] = useState(false);
  const mountedRef = useRef(true);
  const requestSeqRef = useRef(0);
  const queryKey = `${method}:${paramsKey}`;
  const latestRef = useRef<LatestQuery<M>>({ client, method, params, enabled });
  const wasConnectedRef = useRef<boolean | null>(null);

  useEffect(() => {
    latestRef.current = { client, method, params, enabled };
  });

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      requestSeqRef.current += 1;
    };
  }, []);

  const refetch = useCallback(async (): Promise<void> => {
    const latest = latestRef.current;
    if (!mountedRef.current || !latest.enabled) {
      return;
    }

    const sequence = requestSeqRef.current + 1;
    requestSeqRef.current = sequence;

    try {
      const next = await callMethod(latest.client, latest.method, latest.params);
      if (!mountedRef.current || requestSeqRef.current !== sequence) {
        return;
      }
      setData(next);
      setError(undefined);
      setSettled(true);
    } catch (caught) {
      if (!mountedRef.current || requestSeqRef.current !== sequence) {
        return;
      }
      setError(asIpcError(caught));
      setSettled(true);
    }
  }, []);

  useEffect(() => {
    if (!enabled) {
      requestSeqRef.current += 1;
      return;
    }
    void refetch();
  }, [client, enabled, method, queryKey, refetch]);

  useEffect(() => {
    const wasConnected = wasConnectedRef.current;
    wasConnectedRef.current = connected;
    if (connected && wasConnected === false) {
      void refetch();
    }
  }, [connected, refetch]);

  useEffect(() => {
    const unsubscribers = parseEvents(refetchOnKey).map((event) =>
      client.on(event, () => {
        void refetch();
      }),
    );
    return () => {
      for (const unsubscribe of unsubscribers) {
        unsubscribe();
      }
    };
  }, [client, refetch, refetchOnKey]);

  useEffect(() => {
    if (!enabled || pollMs <= 0) {
      return;
    }
    const timer = setInterval(() => {
      void refetch();
    }, pollMs);
    return () => clearInterval(timer);
  }, [enabled, pollMs, refetch]);

  const mutate = useCallback(
    (next: Result<M> | ((prev: Result<M> | undefined) => Result<M>)): void => {
      setData((previous) =>
        typeof next === 'function'
          ? (next as (prev: Result<M> | undefined) => Result<M>)(previous)
          : next,
      );
      setSettled(true);
    },
    [],
  );

  return {
    data,
    error,
    loading: enabled && !settled,
    refetch,
    mutate,
  };
}
