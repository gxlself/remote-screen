import { describe, expect, it, vi } from 'vitest';

import { IpcClient, IpcError } from './client';
import { AGENT_UNAVAILABLE, type RpcErrorShape, type Transport } from './transport';

interface MemoryTransport extends Transport {
  readonly calls: Array<[string, unknown]>;
  emit(method: string, params: unknown): void;
}

function createMemoryTransport(): MemoryTransport {
  const calls: Array<[string, unknown]> = [];
  const eventListeners = new Set<(method: string, params: unknown) => void>();

  return {
    calls,
    async call(method: string, params: unknown): Promise<unknown> {
      calls.push([method, params]);
      return {};
    },
    subscribe(listener: (method: string, params: unknown) => void): () => void {
      eventListeners.add(listener);
      return () => eventListeners.delete(listener);
    },
    onConnection(): () => void {
      return () => undefined;
    },
    async isConnected(): Promise<boolean> {
      return true;
    },
    emit(method: string, params: unknown): void {
      for (const listener of eventListeners) {
        listener(method, params);
      }
    },
  };
}

function createRejectingTransport(error: unknown): Transport {
  return {
    async call(): Promise<unknown> {
      throw error;
    },
    subscribe(): () => void {
      return () => undefined;
    },
    onConnection(): () => void {
      return () => undefined;
    },
    async isConnected(): Promise<boolean> {
      return false;
    },
  };
}

async function captureIpcError(promise: Promise<unknown>): Promise<IpcError> {
  let caught: unknown;
  try {
    await promise;
  } catch (error) {
    caught = error;
  }

  expect(caught).toBeInstanceOf(IpcError);
  return caught as IpcError;
}

describe('IpcClient', () => {
  it('passes null params for methods without arguments', async () => {
    const transport = createMemoryTransport();
    const client = new IpcClient(transport);

    await expect(client.call('status.get')).resolves.toEqual({});
    expect(transport.calls).toEqual([['status.get', null]]);
  });

  it('passes method params without changing them', async () => {
    const transport = createMemoryTransport();
    const client = new IpcClient(transport);
    const params = { session_id: 's-1' };

    await expect(client.call('sessions.disconnect', params)).resolves.toEqual({});
    expect(transport.calls).toEqual([['sessions.disconnect', params]]);
  });

  it('maps a JSON-RPC error with a data code', async () => {
    const rpcError: RpcErrorShape = {
      code: -32000,
      message: 'session not found',
      data: { code: 'not_found' },
    };
    const client = new IpcClient(createRejectingTransport(rpcError));

    const error = await captureIpcError(client.call('sessions.disconnect', { session_id: 's-1' }));

    expect(error.rpcCode).toBe(-32000);
    expect(error.code).toBe('not_found');
    expect(error.message).toBe('session not found');
  });

  it('synthesizes a code when the error has no data code', async () => {
    const client = new IpcClient(
      createRejectingTransport({ code: -32601, message: 'method not found' }),
    );

    const error = await captureIpcError(client.call('status.get'));

    expect(error.code).toBe('rpc_-32601');
  });

  it('maps non-RPC rejection values to transport errors', async () => {
    const client = new IpcClient(createRejectingTransport('boom'));

    const error = await captureIpcError(client.call('status.get'));

    expect(error.rpcCode).toBe(-32603);
    expect(error.code).toBe('transport');
    expect(error.message).toBe('boom');
  });

  it('maps AGENT_UNAVAILABLE to its stable code', async () => {
    const client = new IpcClient(createRejectingTransport(AGENT_UNAVAILABLE));

    const error = await captureIpcError(client.call('status.get'));

    expect(error.rpcCode).toBe(-32099);
    expect(error.code).toBe('agent_unavailable');
  });

  it('delivers events to every registered handler', () => {
    const transport = createMemoryTransport();
    const client = new IpcClient(transport);
    const first = vi.fn();
    const second = vi.fn();

    client.on('event.handoff', first);
    client.on('event.handoff', second);
    transport.emit('event.handoff', { session_id: 's-1' });

    expect(first).toHaveBeenCalledWith({ session_id: 's-1' });
    expect(second).toHaveBeenCalledWith({ session_id: 's-1' });
  });

  it('stops delivering events after unsubscribe', () => {
    const transport = createMemoryTransport();
    const client = new IpcClient(transport);
    const handler = vi.fn();

    const unsubscribe = client.on('event.handoff', handler);
    unsubscribe();
    transport.emit('event.handoff', {});

    expect(handler).not.toHaveBeenCalled();
  });

  it('ignores unknown event names', () => {
    const transport = createMemoryTransport();
    const client = new IpcClient(transport);
    const handler = vi.fn();

    client.on('event.handoff', handler);
    transport.emit('event.unknown', {});

    expect(handler).not.toHaveBeenCalled();
  });

  it('drops invalid approval requests', () => {
    const transport = createMemoryTransport();
    const client = new IpcClient(transport);
    const handler = vi.fn();
    vi.spyOn(console, 'warn').mockImplementation(() => undefined);

    client.on('event.approvalRequested', handler);
    transport.emit('event.approvalRequested', { kind: 'pairing' });

    expect(handler).not.toHaveBeenCalled();
  });

  it('isolates a throwing handler from the remaining handlers', () => {
    const transport = createMemoryTransport();
    const client = new IpcClient(transport);
    const later = vi.fn();
    vi.spyOn(console, 'error').mockImplementation(() => undefined);

    client.on('event.handoff', () => {
      throw new Error('handler failed');
    });
    client.on('event.handoff', later);

    expect(() => transport.emit('event.handoff', {})).not.toThrow();
    expect(later).toHaveBeenCalledWith({});
  });
});
