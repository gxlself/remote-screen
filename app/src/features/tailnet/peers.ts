import type {
  TailnetPeer,
  TailnetRouteStatus,
  TailnetStatus,
} from '../../native/rscore/types';

export type PeerPath = 'direct' | 'relay' | 'offline' | 'unknown';

export function embeddedRunning(
  route: TailnetRouteStatus | null | undefined,
): boolean {
  return route?.active === 'embedded' && route.embedded?.state === 'running';
}

export function firstIpv4(ips: readonly string[]): string | null {
  for (const ip of ips) {
    const octets = ip.split('.');
    if (
      octets.length === 4 &&
      octets.every(octet => {
        if (!/^\d{1,3}$/.test(octet)) {
          return false;
        }
        const value = Number(octet);
        return value >= 0 && value <= 255;
      })
    ) {
      return ip;
    }
  }
  return null;
}

export function embeddedAddr(
  route: TailnetRouteStatus | null | undefined,
): string | null {
  return embeddedRunning(route) ? firstIpv4(route?.embedded?.ips ?? []) : null;
}

export function peerForAddr(
  status: TailnetStatus | null,
  addr: string,
): TailnetPeer | null {
  return status?.peers.find(peer => peer.ips.includes(addr)) ?? null;
}

export function peerPath(peer: TailnetPeer | null): PeerPath {
  if (peer === null) {
    return 'unknown';
  }
  if (!peer.online) {
    return 'offline';
  }
  if (peer.direct) {
    return 'direct';
  }
  if (peer.relay !== '') {
    return 'relay';
  }
  return 'unknown';
}

export function derpName(status: TailnetStatus | null, code: string): string {
  if (code === '') {
    return '';
  }
  if (status?.self?.derpRegion === code && status.self.derpRegionName !== '') {
    return status.self.derpRegionName;
  }
  return code;
}
