import { useCallback } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { tailscaleApi as copy } from '../../i18n/zh/tailscaleApi';
import { useCore } from '../../native/rscore/CoreContext';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import type {
  TailscaleApiStatus,
  TailscaleDevice,
} from '../../native/rscore/types';
import { qk } from '../data/queryClient';
import { useTailnetEnabled } from '../tailnet/enabled';

export const TAILSCALE_API_STATUS_KEY = ['tailscaleApi', 'status'] as const;

type TailscaleApi = NonNullable<RsCoreApi['tailscaleApi']>;

function required(api: TailscaleApi | undefined): TailscaleApi {
  if (api === undefined) {
    throw new RsCoreError('unsupported', 'tailscale api unavailable');
  }
  return api;
}

/** `https://hs.example.com:8443` → `hs.example.com`. */
export function controlHost(controlUrl: string): string {
  return controlUrl.replace(/^https:\/\//, '').replace(/:\d+$/, '');
}

function stateText(status: TailscaleApiStatus, headscale: boolean): string {
  if (status.lastError === 'network' && headscale) {
    return copy.row.networkHeadscale;
  }
  if (status.lastError !== null) {
    return copy.row[status.lastError];
  }
  return status.deviceCount === null
    ? copy.row.configured
    : copy.row.connected(status.deviceCount);
}

/** With a Headscale server, prefixed by its host name. */
export function statusText(status: TailscaleApiStatus | undefined): string {
  if (status === undefined || !status.configured) {
    return copy.row.notSet;
  }
  // Older native packages leave controlUrl out.
  const control = status.controlUrl ?? null;
  const state = stateText(status, control !== null);
  return control === null
    ? state
    : copy.row.withHost(controlHost(control), state);
}

export function errorText(error: unknown, headscale = false): string {
  if (error instanceof RsCoreError) {
    if (
      error.kind === 'http' &&
      (error.httpStatus === 401 || error.httpStatus === 403)
    ) {
      return headscale
        ? copy.sheet.errors.authHeadscale
        : copy.sheet.errors.auth;
    }
    if (error.kind === 'network') {
      return headscale
        ? copy.sheet.errors.networkHeadscale
        : copy.sheet.errors.network;
    }
    if (error.kind === 'internal' && error.message === 'bad control url') {
      return copy.sheet.errors.controlUrl;
    }
  }
  return copy.sheet.errors.generic;
}

/**
 * Status and actions of the Tailscale API token. The token goes straight to
 * the native keychain: no mutation cache, kv or log ever holds it.
 */
export function useTailscaleApi() {
  const api = useCore().tailscaleApi;
  const queryClient = useQueryClient();
  // Tailscale switched off: the API is never asked, not even for its status.
  const tailnetEnabled = useTailnetEnabled();
  const status = useQuery({
    queryKey: TAILSCALE_API_STATUS_KEY,
    queryFn: () => required(api).status(),
    enabled: api !== undefined && tailnetEnabled,
  });

  const settle = useCallback(() => {
    queryClient.invalidateQueries({ queryKey: TAILSCALE_API_STATUS_KEY });
    queryClient.invalidateQueries({ queryKey: qk.directory });
  }, [queryClient]);

  /** `controlUrl` null: the official Tailscale API. */
  const save = useCallback(
    async (
      token: string,
      controlUrl: string | null,
    ): Promise<TailscaleDevice[]> => {
      try {
        return await required(api).setToken(token, controlUrl);
      } finally {
        settle();
      }
    },
    [api, settle],
  );

  const refresh = useCallback(async (): Promise<TailscaleDevice[]> => {
    try {
      return await required(api).devices(true);
    } finally {
      settle();
    }
  }, [api, settle]);

  const clear = useCallback(async (): Promise<void> => {
    try {
      await required(api).clearToken();
    } finally {
      settle();
    }
  }, [api, settle]);

  return {
    status: status.data,
    isLoading: status.isLoading,
    isError: status.isError,
    save,
    refresh,
    clear,
  };
}
