import type { NetworkStatus } from '../../native/rscore/types';
import type { NetworkEntry } from '../settings/schema';
import { embeddedAddr } from '../tailnet/peers';
import { ipInCidr, isTailnetAddr } from './cidr';

export type NetVerdict = {
  tailnet: 'up' | 'down' | 'unknown';
  tailnetAddr: string | null;
  companyVpn: { network: NetworkEntry; addr: string } | null;
  atCompany: NetworkEntry | null;
  banner: 'none' | 'vpnConflict' | 'tailnetDown' | 'embeddedLogin';
  /** Only present when the network status includes a tailnet summary. */
  via?: 'system' | 'embedded';
};

type Address = { iface: NetworkStatus['ifaces'][number]; addr: string };

// Rust reports interface addresses as `ip/prefix`; the judge compares hosts.
function addresses(status: NetworkStatus): Address[] {
  return status.ifaces.flatMap(iface =>
    iface.addrs.map(addr => ({ iface, addr: addr.split('/')[0] })),
  );
}

export function localHostAddrs(status: NetworkStatus | null): string[] {
  return status === null ? [] : addresses(status).map(({ addr }) => addr);
}

function findInPools(
  candidates: Address[],
  poolsOf: (network: NetworkEntry) => string[],
  networks: NetworkEntry[],
): { network: NetworkEntry; addr: string } | null {
  for (const candidate of candidates) {
    for (const network of networks) {
      if (poolsOf(network).some(cidr => ipInCidr(candidate.addr, cidr))) {
        return { network, addr: candidate.addr };
      }
    }
  }
  return null;
}

function findNetwork(
  candidates: Address[],
  cidrsOf: (network: NetworkEntry) => string[],
  networks: NetworkEntry[],
): NetworkEntry | null {
  for (const candidate of candidates) {
    for (const network of networks) {
      if (cidrsOf(network).some(cidr => ipInCidr(candidate.addr, cidr))) {
        return network;
      }
    }
  }
  return null;
}

export function judgeNetwork(
  status: NetworkStatus | null,
  networks: NetworkEntry[],
): NetVerdict {
  if (status === null) {
    return {
      tailnet: 'unknown',
      tailnetAddr: null,
      companyVpn: null,
      atCompany: null,
      banner: 'none',
    };
  }

  const all = addresses(status);
  const tailnetAddr = all.find(item => isTailnetAddr(item.addr))?.addr ?? null;
  const companyVpn = findInPools(
    all.filter(item => item.iface.isVpn && item.iface.workProfile !== true),
    network => network.vpnPools,
    networks,
  );
  const atCompany = findNetwork(all, network => network.cidrs, networks);

  const route = status.tailnet;
  if (route !== undefined && route.active === 'embedded') {
    const addr = embeddedAddr(route);
    const state = route.embedded?.state ?? null;
    if (addr !== null) {
      return {
        tailnet: 'up',
        tailnetAddr: addr,
        companyVpn,
        atCompany,
        banner: 'none',
        via: 'embedded',
      };
    }
    if (state === 'needsLogin' || state === 'needsMachineAuth') {
      return {
        tailnet: 'down',
        tailnetAddr: null,
        companyVpn,
        atCompany,
        banner: 'embeddedLogin',
        via: 'embedded',
      };
    }
    if (
      state === null ||
      state === 'noState' ||
      state === 'starting' ||
      state === 'running'
    ) {
      return {
        tailnet: 'unknown',
        tailnetAddr: null,
        companyVpn,
        atCompany,
        banner: 'none',
        via: 'embedded',
      };
    }
    return {
      tailnet: 'down',
      tailnetAddr: null,
      companyVpn,
      atCompany,
      banner: 'none',
      via: 'embedded',
    };
  }

  if (tailnetAddr !== null) {
    return {
      tailnet: 'up',
      tailnetAddr,
      companyVpn,
      atCompany,
      banner: 'none',
      ...(route === undefined ? {} : { via: 'system' as const }),
    };
  }

  return {
    tailnet: 'down',
    tailnetAddr: null,
    companyVpn,
    atCompany,
    banner: companyVpn === null ? 'tailnetDown' : 'vpnConflict',
    ...(route === undefined ? {} : { via: 'system' as const }),
  };
}
