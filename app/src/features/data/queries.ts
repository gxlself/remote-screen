import { useQuery } from '@tanstack/react-query';
import { usePlatform } from '../../native/platform/PlatformContext';
import { useCore } from '../../native/rscore/CoreContext';
import { qk } from './queryClient';
export { useDirectory } from './directoryQuery';
export { useSync, usePutSync } from './syncQuery';

export function usePairedHosts() {
  const core = useCore();
  return useQuery({
    queryKey: qk.pairedHosts,
    queryFn: () => core.hosts.paired(),
    staleTime: 60000,
  });
}

export function useIdentity() {
  const core = useCore();
  const platform = usePlatform();
  return useQuery({
    queryKey: qk.identity,
    // 07 does not specify idempotence; ensure is treated as idempotent here.
    queryFn: () => core.identity.ensure(platform.deviceInfo().model),
    staleTime: Infinity,
  });
}

export function useSshServers() {
  const core = useCore();
  return useQuery({
    queryKey: qk.sshServers,
    queryFn: () => core.ssh.servers(),
    staleTime: 30000,
  });
}

export function useSshKeys() {
  const core = useCore();
  return useQuery({
    queryKey: qk.sshKeys,
    queryFn: () => core.ssh.keys(),
    staleTime: 30000,
  });
}

export function useTrustedHostKeys() {
  const core = useCore();
  return useQuery({
    queryKey: qk.trustedHostKeys,
    queryFn: () => core.ssh.trustedHostKeys(),
    staleTime: 30000,
  });
}
