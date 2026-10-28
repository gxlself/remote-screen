import { render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { Transport } from './transport';

const mocks = vi.hoisted(() => ({
  createTauriTransport: vi.fn(),
}));

vi.mock('./tauri', () => ({
  createTauriTransport: mocks.createTauriTransport,
}));

import { FakeIpc } from './fake';
import { useIpc } from './react';
import { IpcRoot, createDefaultTransport } from './root';

function Probe() {
  const client = useIpc();
  return <span>{client.constructor.name}</span>;
}

beforeEach(() => {
  mocks.createTauriTransport.mockReset();
  delete window.__fakeIpc;
  Reflect.deleteProperty(window, '__TAURI_INTERNALS__');
});

afterEach(() => {
  window.history.replaceState({}, '', '/');
  delete window.__fakeIpc;
  Reflect.deleteProperty(window, '__TAURI_INTERNALS__');
});

describe('createDefaultTransport', () => {
  it('creates a fake IPC from the scenario query in a browser', async () => {
    window.history.replaceState({}, '', '/?scenario=idle');

    const transport = createDefaultTransport();

    expect(transport).toBeInstanceOf(FakeIpc);
    expect(window.__fakeIpc).toBe(transport);
    await expect(transport.call('status.get', null)).resolves.toMatchObject({ sessions: [] });
  });

  it('creates the Tauri transport inside Tauri', () => {
    const transport: Transport = {
      call: vi.fn(),
      subscribe: vi.fn(),
      onConnection: vi.fn(),
      isConnected: vi.fn(),
    };
    mocks.createTauriTransport.mockReturnValue(transport);
    Object.defineProperty(window, '__TAURI_INTERNALS__', {
      configurable: true,
      value: {},
    });

    expect(createDefaultTransport()).toBe(transport);
    expect(mocks.createTauriTransport).toHaveBeenCalledOnce();
  });
});

describe('IpcRoot', () => {
  it('provides a usable IPC client to descendants', () => {
    window.history.replaceState({}, '', '/?scenario=idle');

    render(
      <IpcRoot>
        <Probe />
      </IpcRoot>,
    );

    expect(screen.getByText('IpcClient')).toBeInTheDocument();
    expect(window.__fakeIpc).toBeInstanceOf(FakeIpc);
  });
});
