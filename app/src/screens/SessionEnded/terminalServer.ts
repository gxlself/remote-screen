import { useSshServers } from '../../features/data/queries';
import type { Computer, SshServer } from '../../native/rscore/types';

/**
 * The SSH server that reaches this computer: listed under its node name or
 * at one of its addresses. `ssh.open` only knows listed servers (V58).
 */
export function sshServerFor(
  servers: readonly SshServer[],
  host: Computer | undefined,
): string | null {
  if (host === undefined) {
    return null;
  }
  const addrs = new Set([host.node, ...host.addresses.map(a => a.addr)]);
  const server =
    servers.find(candidate => candidate.serverId === host.node) ??
    servers.find(candidate => addrs.has(candidate.host));
  return server?.serverId ?? null;
}

export function useTerminalServer(host: Computer | undefined): string | null {
  const { data } = useSshServers();
  return sshServerFor(data ?? [], host);
}
