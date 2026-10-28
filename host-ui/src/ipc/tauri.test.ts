import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
}));

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: mocks.listen }));

import { createTauriTransport } from './tauri';

describe('createTauriTransport', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('calls ipc_call with method and params', async () => {
    mocks.invoke.mockResolvedValue({ qr_url: 'rs://pair/example' });
    const transport = createTauriTransport();

    await expect(transport.call('pairing.start', null)).resolves.toEqual({
      qr_url: 'rs://pair/example',
    });
    expect(mocks.invoke).toHaveBeenCalledWith('ipc_call', {
      method: 'pairing.start',
      params: null,
    });
  });

  it('forwards ipc://event payloads to subscribers', () => {
    let eventHandler: ((event: { payload: unknown }) => void) | undefined;
    mocks.listen.mockImplementation(
      (_eventName: string, handler: (event: { payload: unknown }) => void) => {
        eventHandler = handler;
        return Promise.resolve(() => undefined);
      },
    );
    const transport = createTauriTransport();
    const listener = vi.fn();

    transport.subscribe(listener);
    expect(mocks.listen).toHaveBeenCalledWith('ipc://event', expect.any(Function));
    expect(eventHandler).toBeDefined();
    eventHandler?.({ payload: { method: 'event.handoff', params: { session_id: 's-1' } } });

    expect(listener).toHaveBeenCalledWith('event.handoff', { session_id: 's-1' });
  });

  it('unlistens when unsubscribe runs before listen resolves', async () => {
    let resolveListen: ((stop: () => void) => void) | undefined;
    mocks.listen.mockReturnValue(
      new Promise<() => void>((resolve) => {
        resolveListen = resolve;
      }),
    );
    const unlisten = vi.fn();
    const transport = createTauriTransport();

    const unsubscribe = transport.subscribe(() => undefined);
    unsubscribe();
    expect(unlisten).not.toHaveBeenCalled();

    resolveListen?.(unlisten);
    await vi.waitFor(() => expect(unlisten).toHaveBeenCalledTimes(1));
  });
});
