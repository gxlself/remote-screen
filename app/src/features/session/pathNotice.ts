import type { NetPath } from '../../native/rscore/types';
import { sharedRelayTcp } from '../../i18n/zh/sharedRelayTcp';
import { ipInCidr } from '../network/cidr';
import type { NetworkEntry } from '../settings/schema';

export function intranetNetworkAt(
  addr: string | null,
  networks: NetworkEntry[],
): NetworkEntry | null {
  if (addr === null) {
    return null;
  }
  for (const entry of networks) {
    if (entry.cidrs.some(cidr => ipInCidr(addr, cidr))) {
      return entry;
    }
  }
  return null;
}

export function hudCaption(i: {
  relay: boolean;
  tcp: boolean;
  path: NetPath | null;
  addr: string | null;
  networks: NetworkEntry[];
}): string | null {
  if (i.relay) {
    return sharedRelayTcp.path.relayCaption;
  }
  if (!i.tcp || i.addr === null) {
    return null;
  }

  const network = intranetNetworkAt(i.addr, i.networks);
  if (network !== null) {
    return sharedRelayTcp.path.intranetCaption(network.name, i.addr);
  }
  if (i.path === 'intranet' || i.path === 'lan') {
    return sharedRelayTcp.path.genericIntranetCaption(i.addr);
  }
  return null;
}

export type NoticeKind = 'relay' | 'tcp';

export type PathNoticeText = {
  title: string;
  body: string;
};

export function noticeFor(
  kind: NoticeKind,
  i: {
    region: string | null;
    height: number | null;
    fps: number | null;
    intranet: boolean;
  },
): PathNoticeText {
  if (kind === 'relay') {
    return {
      title: sharedRelayTcp.path.relayTitle(i.region, i.height, i.fps),
      body: sharedRelayTcp.path.relayBody,
    };
  }
  return {
    title: sharedRelayTcp.path.tcpTitle(i.intranet),
    body: sharedRelayTcp.path.tcpBody,
  };
}
