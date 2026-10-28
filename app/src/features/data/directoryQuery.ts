import { useCallback, useContext, useMemo, useSyncExternalStore } from 'react';
import {
  NavigationContext,
  NavigationRouteContext,
} from '@react-navigation/native';
import { useQuery } from '@tanstack/react-query';
import { useCore } from '../../native/rscore/CoreContext';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import type {
  Directory,
  PairedHost,
  TailnetStatus,
  TailscaleDevice,
} from '../../native/rscore/types';
import { ipInCidr } from '../network/cidr';
import type { NetworkEntry } from '../settings/schema';
import { getSetting, useSetting } from '../settings/store';
import type { KvStore } from '../storage/kv';
import { isTailnetEnabled, useTailnetEnabled } from '../tailnet/enabled';
import {
  probeTargets,
  tailnetProbesWithin,
  useTailnetProbes,
} from '../tailscale/tailnetProbe';
import { buildLocalDirectory } from './localDirectory';
import { readLocalReach, useLocalReach } from './localReach';
import { qk } from './queryClient';

export type DirectoryQuery = {
  directory: Directory | undefined;
  isLoading: boolean;
  error: Error | null;
  refetch: () => Promise<unknown>;
};

type LocalInputs = {
  paired: PairedHost[];
  tailnet: TailnetStatus | null;
  apiDevices: TailscaleDevice[] | null;
};

/**
 * Host addresses inside a registered network are intranet addresses of that
 * network, as Rust treats them when it dials and probes.
 */
export function markIntranet(
  directory: Directory,
  networks: readonly NetworkEntry[],
): Directory {
  const cidrs = networks.flatMap(network => network.cidrs);
  if (cidrs.length === 0) {
    return directory;
  }
  return {
    ...directory,
    computers: directory.computers.map(computer => ({
      ...computer,
      addresses: computer.addresses.map(address => {
        if (address.kind !== 'lan') {
          return address;
        }
        const cidr = cidrs.find(value => ipInCidr(address.addr, value));
        return cidr === undefined
          ? address
          : { ...address, kind: 'intranet' as const, cidr };
      }),
    })),
  };
}

// Nested under qk.directory so the existing pair / forget invalidations reach it.
const LOCAL_INPUTS_KEY = [...qk.directory, 'local'] as const;

/** Only the in-app tailnet lists nodes; without it, status is never asked. */
async function runningTailnet(core: RsCoreApi): Promise<TailnetStatus | null> {
  try {
    if (!(await core.tailnet.available())) {
      return null;
    }
    const status = await core.tailnet.status();
    return status.state === 'running' ? status : null;
  } catch {
    return null;
  }
}

/** Devices from the Tailscale API token; any failure lists none. */
async function tailscaleApiDevices(
  core: RsCoreApi,
): Promise<TailscaleDevice[] | null> {
  const api = core.tailscaleApi;
  if (api === undefined) {
    return null;
  }
  try {
    const status = await api.status();
    return status.configured ? await api.devices(false) : null;
  } catch {
    return null;
  }
}

/**
 * With Tailscale switched off only the paired Hosts are read: the tailnet is
 * never asked for its status and the Tailscale API is never called.
 */
async function loadLocalInputs(
  core: RsCoreApi,
  tailnetEnabled: boolean,
): Promise<LocalInputs> {
  const [paired, tailnet, apiDevices] = await Promise.all([
    core.hosts.paired(),
    tailnetEnabled ? runningTailnet(core) : null,
    tailnetEnabled ? tailscaleApiDevices(core) : null,
  ]);
  return { paired, tailnet, apiDevices };
}

/**
 * Non-hook read; combines paired Hosts, cached reach, the tailnet and, given
 * `kv`, the port classes of Headscale devices (waiting at most PROBE_WAIT_MS
 * for devices that are due).
 */
export async function loadDirectory(
  core: RsCoreApi,
  kv?: KvStore,
): Promise<Directory> {
  const { paired, tailnet, apiDevices } = await loadLocalInputs(
    core,
    isTailnetEnabled(),
  );
  const apiProbes =
    kv === undefined || apiDevices === null
      ? {}
      : await tailnetProbesWithin(core, kv, probeTargets(apiDevices));
  return markIntranet(
    buildLocalDirectory({
      paired,
      reach: kv === undefined ? {} : readLocalReach(kv),
      tailnet,
      apiDevices,
      apiProbes,
      now: Date.now() * 1000,
    }),
    getSetting('networks'),
  );
}

/** Only the focused Home tab probes; other screens read the last results. */
function useHomeVisible(): boolean {
  const navigation = useContext(NavigationContext);
  const route = useContext(NavigationRouteContext);
  const subscribe = useCallback(
    (onChange: () => void) => {
      if (navigation === undefined) {
        return () => undefined;
      }
      const offFocus = navigation.addListener('focus', onChange);
      const offBlur = navigation.addListener('blur', onChange);
      return () => {
        offFocus();
        offBlur();
      };
    },
    [navigation],
  );
  const focused = useSyncExternalStore(
    subscribe,
    () => navigation?.isFocused() ?? false,
  );
  return route?.name === 'Home' && focused;
}

function useLocalDirectory(): DirectoryQuery {
  const core = useCore();
  const homeVisible = useHomeVisible();
  const tailnetEnabled = useTailnetEnabled();
  const inputs = useQuery({
    queryKey: [...LOCAL_INPUTS_KEY, tailnetEnabled],
    queryFn: () => loadLocalInputs(core, tailnetEnabled),
    staleTime: 5000,
    refetchInterval: 10000,
    refetchIntervalInBackground: false,
  });
  const hostIds = useMemo(
    () => inputs.data?.paired.map(host => host.hostId) ?? [],
    [inputs.data],
  );
  const { reach, refresh } = useLocalReach(hostIds, homeVisible);
  // Never waits for a probe round: saved classes first, fresh ones as they come.
  const { probes: apiProbes, refresh: reprobe } = useTailnetProbes(
    inputs.data?.apiDevices ?? null,
    tailnetEnabled,
  );
  const [networks] = useSetting('networks');
  const directory = useMemo(
    () =>
      inputs.data === undefined
        ? undefined
        : markIntranet(
            buildLocalDirectory({
              paired: inputs.data.paired,
              reach,
              tailnet: inputs.data.tailnet,
              apiDevices: inputs.data.apiDevices,
              apiProbes,
              now: Date.now() * 1000,
            }),
            networks,
          ),
    [inputs.data, reach, apiProbes, networks],
  );
  const refetchInputs = inputs.refetch;
  // Pull to refresh probes the Headscale devices again too.
  const refetch = useCallback(
    () => Promise.all([refetchInputs(), refresh(), reprobe()]),
    [refetchInputs, refresh, reprobe],
  );
  return {
    directory,
    isLoading: inputs.isLoading,
    error: inputs.error,
    refetch,
  };
}

export function useDirectory(): DirectoryQuery {
  return useLocalDirectory();
}
