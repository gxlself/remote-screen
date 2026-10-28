import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

import type { Transport } from './transport';

function subscribeEvent<T>(eventName: string, listener: (payload: T) => void): () => void {
  let active = true;
  let unlisten: (() => void) | null = null;

  void listen<T>(eventName, (event) => {
    if (active) {
      listener(event.payload);
    }
  }).then(
    (stop) => {
      unlisten = stop;
      if (!active) {
        unlisten();
        unlisten = null;
      }
    },
    () => {
      console.error(`failed to subscribe to ${eventName}`);
    },
  );

  return () => {
    active = false;
    if (unlisten) {
      unlisten();
      unlisten = null;
    }
  };
}

export function createTauriTransport(): Transport {
  return {
    call(method: string, params: unknown): Promise<unknown> {
      return invoke('ipc_call', { method, params });
    },
    subscribe(listener: (method: string, params: unknown) => void): () => void {
      return subscribeEvent<{ method: string; params: unknown }>('ipc://event', (payload) => {
        listener(payload.method, payload.params);
      });
    },
    onConnection(listener: (connected: boolean) => void): () => void {
      return subscribeEvent<{ connected: boolean }>('ipc://connection', (payload) => {
        listener(payload.connected);
      });
    },
    isConnected(): Promise<boolean> {
      return invoke<boolean>('ipc_connected');
    },
  };
}
