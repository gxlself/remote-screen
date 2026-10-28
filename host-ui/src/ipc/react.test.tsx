import { act, renderHook, screen } from '@testing-library/react';
import { useState, type ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { FakeIpc } from './fake';
import type { HandoffEvent } from './events';
import { IpcProvider, useIpc, useIpcConnected, useIpcEvent, useIpcQuery } from './react';
import { flush, renderPage } from './testing';

function withIpc(ipc: FakeIpc) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return <IpcProvider transport={ipc}>{children}</IpcProvider>;
  };
}

afterEach(() => {
  vi.useRealTimers();
});

describe('useIpcQuery', () => {
  it('loads sessions and refetches when the requested event arrives', async () => {
    const ipc = new FakeIpc();
    const { result } = renderHook(
      () => useIpcQuery('sessions.list', { refetchOn: ['event.sessionsChanged'] }),
      { wrapper: withIpc(ipc) },
    );

    expect(result.current.loading).toBe(true);
    await flush();

    expect(result.current.loading).toBe(false);
    expect(result.current.error).toBeUndefined();
    expect(result.current.data?.sessions).toHaveLength(1);
    expect(ipc.calls.filter((call) => call.method === 'sessions.list')).toHaveLength(1);

    act(() => ipc.emit('event.sessionsChanged', {}));
    await flush();

    expect(ipc.calls.filter((call) => call.method === 'sessions.list')).toHaveLength(2);
  });

  it('keeps old data on an error and clears the error after a successful refetch', async () => {
    const ipc = new FakeIpc();
    const { result } = renderHook(() => useIpcQuery('sessions.list'), {
      wrapper: withIpc(ipc),
    });
    await flush();
    const previous = result.current.data;

    ipc.failNext('sessions.list', {
      code: -32000,
      message: 'x',
      data: { code: 'busy' },
    });
    await act(async () => {
      await result.current.refetch();
    });

    expect(result.current.error?.code).toBe('busy');
    expect(result.current.error?.message).toBe('x');
    expect(result.current.error?.rpcCode).toBe(-32000);
    expect(result.current.data).toEqual(previous);

    await act(async () => {
      await result.current.refetch();
    });

    expect(result.current.error).toBeUndefined();
    expect(result.current.data).toEqual(previous);
  });

  it('refetches when params change', async () => {
    const ipc = new FakeIpc();
    const { result, rerender } = renderHook(
      ({ limit }: { limit: number }) => useIpcQuery('audit.list', { params: { limit } }),
      { initialProps: { limit: 1 }, wrapper: withIpc(ipc) },
    );
    await flush();
    expect(result.current.data?.entries).toHaveLength(1);

    rerender({ limit: 2 });
    await flush();

    expect(result.current.data?.entries).toHaveLength(2);
    expect(ipc.calls.filter((call) => call.method === 'audit.list')).toHaveLength(2);
  });

  it('polls while mounted and stops after unmount', async () => {
    vi.useFakeTimers();
    const ipc = new FakeIpc();
    const { unmount } = renderHook(() => useIpcQuery('sessions.list', { pollMs: 1000 }), {
      wrapper: withIpc(ipc),
    });
    await flush();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });

    expect(ipc.calls.filter((call) => call.method === 'sessions.list')).toHaveLength(4);

    unmount();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(ipc.calls.filter((call) => call.method === 'sessions.list')).toHaveLength(4);
  });

  it('does nothing while disabled', async () => {
    const ipc = new FakeIpc();
    const { result } = renderHook(() => useIpcQuery('sessions.list', { enabled: false }), {
      wrapper: withIpc(ipc),
    });
    await flush();
    await result.current.refetch();

    expect(ipc.calls).toEqual([]);
    expect(result.current.loading).toBe(false);
  });

  it('refetches when the connection transitions from false to true', async () => {
    const ipc = new FakeIpc();
    renderHook(() => useIpcQuery('sessions.list'), { wrapper: withIpc(ipc) });
    await flush();
    expect(ipc.calls.filter((call) => call.method === 'sessions.list')).toHaveLength(1);

    act(() => ipc.setConnected(false));
    await flush();
    act(() => ipc.setConnected(true));
    await flush();

    expect(ipc.calls.filter((call) => call.method === 'sessions.list')).toHaveLength(2);
  });

  it('only applies the latest of out-of-order responses', async () => {
    vi.useFakeTimers();
    const ipc = new FakeIpc(undefined, {
      delay: (method, params) => {
        if (
          method === 'audit.list' &&
          typeof params === 'object' &&
          params !== null &&
          'limit' in params &&
          params.limit === 1
        ) {
          return 50;
        }
        return 0;
      },
    });
    const { result, rerender } = renderHook(
      ({ limit }: { limit: number }) => useIpcQuery('audit.list', { params: { limit } }),
      { initialProps: { limit: 1 }, wrapper: withIpc(ipc) },
    );
    await flush();
    expect(result.current.loading).toBe(true);

    rerender({ limit: 2 });
    await flush();
    expect(result.current.data?.entries).toHaveLength(2);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(50);
    });
    expect(result.current.data?.entries).toHaveLength(2);
  });

  it('does not update state after unmounting with a request in flight', async () => {
    vi.useFakeTimers();
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const ipc = new FakeIpc(undefined, { delay: () => 50 });
    const { unmount } = renderHook(() => useIpcQuery('sessions.list'), {
      wrapper: withIpc(ipc),
    });
    await flush();
    expect(ipc.calls.filter((call) => call.method === 'sessions.list')).toHaveLength(1);

    unmount();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(50);
    });

    expect(consoleError).not.toHaveBeenCalled();
  });

  it('applies optimistic mutations immediately', async () => {
    const ipc = new FakeIpc();
    const { result } = renderHook(() => useIpcQuery('sessions.list'), {
      wrapper: withIpc(ipc),
    });
    await flush();

    act(() => result.current.mutate({ sessions: [] }));

    expect(result.current.data).toEqual({ sessions: [] });
  });
});

describe('useIpcEvent', () => {
  it('keeps one subscription while swapping handlers', () => {
    const ipc = new FakeIpc();
    const subscribe = vi.spyOn(ipc, 'subscribe');
    const handlers = [vi.fn(), vi.fn(), vi.fn()];
    const { rerender, unmount } = renderHook(
      ({ handler }: { handler: (payload: HandoffEvent) => void }) =>
        useIpcEvent('event.handoff', handler),
      { initialProps: { handler: handlers[0]! }, wrapper: withIpc(ipc) },
    );

    rerender({ handler: handlers[1]! });
    rerender({ handler: handlers[2]! });
    act(() => ipc.emit('event.handoff', { session_id: 's-2' }));

    expect(subscribe).toHaveBeenCalledTimes(1);
    expect(handlers[0]).not.toHaveBeenCalled();
    expect(handlers[1]).not.toHaveBeenCalled();
    expect(handlers[2]).toHaveBeenCalledOnce();

    unmount();
    act(() => ipc.emit('event.handoff', { session_id: 's-3' }));

    expect(handlers[2]).toHaveBeenCalledOnce();
  });
});

describe('useIpcConnected', () => {
  it('starts optimistic and follows connection events', async () => {
    const ipc = new FakeIpc();
    const { result } = renderHook(() => useIpcConnected(), { wrapper: withIpc(ipc) });

    expect(result.current).toBe(true);
    await flush();
    act(() => ipc.setConnected(false));

    expect(result.current).toBe(false);
  });
});

describe('useIpc', () => {
  it('throws without a provider', () => {
    expect(() => renderHook(() => useIpc())).toThrow('useIpc must be used inside an IpcProvider');
  });
});

describe('renderPage', () => {
  function PageProbe() {
    const { data } = useIpcQuery('status.get');
    const [clicked, setClicked] = useState(false);

    return (
      <div>
        <span>{data?.version ?? 'loading'}</span>
        <button type="button" onClick={() => setClicked(true)}>
          click
        </button>
        {clicked ? <span>clicked</span> : null}
      </div>
    );
  }

  it('renders with fake IPC and supports user events with fake timers', async () => {
    vi.useFakeTimers();
    const { user } = renderPage(<PageProbe />);

    await flush();
    expect(screen.getByText('0.3.1')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'click' }));
    expect(screen.getByText('clicked')).toBeInTheDocument();
  });
});
