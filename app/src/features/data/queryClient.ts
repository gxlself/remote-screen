import { focusManager, QueryClient } from '@tanstack/react-query';
import { AppState } from 'react-native';

export const qk = {
  directory: ['directory'] as const,
  pairedHosts: ['pairedHosts'] as const,
  sync: (ns: string) => ['sync', ns] as const,
  identity: ['identity'] as const,
  sshServers: ['sshServers'] as const,
  sshKeys: ['sshKeys'] as const,
  trustedHostKeys: ['trustedHostKeys'] as const,
};

export function createQueryClient(opts?: { test?: boolean }): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        retry: opts?.test ? false : 1,
        staleTime: 0,
        ...(opts?.test ? { gcTime: Infinity } : {}),
      },
      ...(opts?.test ? { mutations: { gcTime: Infinity } } : {}),
    },
  });
}

export function setupAppFocus(): () => void {
  focusManager.setFocused(AppState.currentState === 'active');
  const subscription = AppState.addEventListener('change', state => {
    focusManager.setFocused(state === 'active');
  });
  return () => subscription.remove();
}
