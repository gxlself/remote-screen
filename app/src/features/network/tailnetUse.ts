import type { SshServer } from '../../native/rscore/types';
import type { ComputerEntry } from '../native/types';
import type { ManualServer } from '../servers/manual';
import { isTailnetAddr } from './cidr';

const TAILNET_IPV6_PREFIX = 'fd7a:115c:a1e0:';

/** A Tailscale address or MagicDNS name, as the user typed it. */
export function isTailnetHost(host: string): boolean {
  const value = host
    .trim()
    .toLowerCase()
    .replace(/^\[|\]$/g, '')
    .replace(/\.$/, '');
  return (
    isTailnetAddr(value) ||
    value.startsWith(TAILNET_IPV6_PREFIX) ||
    value.endsWith('.ts.net')
  );
}

export type TailnetUseInputs = {
  /** Settings › 网络接入 › 外网访问 (`tailnet.enabled`). */
  enabled: boolean;
  entries: readonly ComputerEntry[];
  sshServers: readonly SshServer[];
  manualServers: readonly ManualServer[];
  /** A Tailscale API token is saved. */
  apiConfigured: boolean;
  /** The in-app tailnet is built in and has a Headscale address. */
  embeddedConfigured: boolean;
};

/**
 * Whether this user reaches anything over Tailscale. Without that, "Tailscale
 * is not connected" is noise: someone who only uses the local network never
 * set it up (and an App Review tester never will).
 */
export function tailnetInUse(inputs: TailnetUseInputs): boolean {
  // Switched off, Tailscale is never brought up, whatever the list holds.
  if (!inputs.enabled) {
    return false;
  }
  if (inputs.apiConfigured || inputs.embeddedConfigured) {
    return true;
  }
  const computer = inputs.entries.some(entry => {
    if (entry.kind === 'host') {
      return entry.computer.addresses.some(
        address => address.kind === 'tailnet' || isTailnetHost(address.addr),
      );
    }
    return entry.kind === 'tailnet' || isTailnetHost(entry.manual.host);
  });
  return (
    computer ||
    inputs.sshServers.some(
      server => server.source === 'directory' || isTailnetHost(server.host),
    ) ||
    inputs.manualServers.some(server => isTailnetHost(server.host))
  );
}
