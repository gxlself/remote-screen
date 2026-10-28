import { EVENT_NAMES, parseEvent, type EventName, type IpcEvents } from './events';
import type { RpcErrorShape, Transport } from './transport';
import type { MethodName, Params, Result } from './types';

type ParamsArgs<M extends MethodName> = Params<M> extends null ? [] : [params: Params<M>];
type EventHandler = (payload: unknown) => void;

export class IpcError extends Error {
  readonly rpcCode: number;
  readonly code: string;

  constructor(rpcCode: number, code: string, message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = 'IpcError';
    this.rpcCode = rpcCode;
    this.code = code;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isRpcErrorShape(value: unknown): value is RpcErrorShape {
  return isRecord(value) && typeof value.code === 'number' && typeof value.message === 'string';
}

function toIpcError(value: unknown): IpcError {
  if (isRpcErrorShape(value)) {
    const dataCode = value.data?.code;
    const code = typeof dataCode === 'string' ? dataCode : `rpc_${value.code}`;
    return new IpcError(value.code, code, value.message, { cause: value });
  }

  return new IpcError(-32603, 'transport', String(value), { cause: value });
}

function isEventName(value: string): value is EventName {
  return (EVENT_NAMES as readonly string[]).includes(value);
}

export class IpcClient {
  private readonly handlers = new Map<EventName, Set<EventHandler>>();
  private eventUnsubscribe: (() => void) | null = null;

  constructor(private readonly transport: Transport) {}

  async call<M extends MethodName>(method: M, ...args: ParamsArgs<M>): Promise<Result<M>> {
    const params = args.length === 0 ? null : (args[0] as Params<M>);

    try {
      return (await this.transport.call(method, params)) as Result<M>;
    } catch (error) {
      throw toIpcError(error);
    }
  }

  on<E extends EventName>(event: E, handler: (payload: IpcEvents[E]) => void): () => void {
    let handlers = this.handlers.get(event);
    if (!handlers) {
      handlers = new Set<EventHandler>();
      this.handlers.set(event, handlers);
    }

    const registered = handler as unknown as EventHandler;
    handlers.add(registered);
    this.ensureEventSubscription();

    let active = true;
    return () => {
      if (!active) {
        return;
      }
      active = false;
      handlers.delete(registered);
      if (handlers.size === 0) {
        this.handlers.delete(event);
      }
    };
  }

  onConnection(handler: (connected: boolean) => void): () => void {
    return this.transport.onConnection(handler);
  }

  isConnected(): Promise<boolean> {
    return this.transport.isConnected();
  }

  private ensureEventSubscription(): void {
    if (this.eventUnsubscribe) {
      return;
    }

    this.eventUnsubscribe = this.transport.subscribe((method, params) => {
      this.dispatchEvent(method, params);
    });
  }

  private dispatchEvent(method: string, params: unknown): void {
    if (!isEventName(method)) {
      return;
    }

    const payload = parseEvent(method, params);
    if (payload === null) {
      console.warn(`IPC event ${method} had an invalid payload`);
      return;
    }

    const handlers = this.handlers.get(method);
    if (!handlers) {
      return;
    }

    for (const handler of [...handlers]) {
      try {
        handler(payload);
      } catch {
        console.error(`IPC event ${method} handler failed`);
      }
    }
  }
}
