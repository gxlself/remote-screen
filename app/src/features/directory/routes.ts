import type { PathKind } from '../../components/PathMark';
import type {
  HostDetail,
  NetPath,
  TailnetStatus,
} from '../../native/rscore/types';
import { deviceRoutes } from '../../i18n/zh/deviceRoutes';
import { spokenMs } from '../format/latency';
import { ipInCidr } from '../network/cidr';
import type { NetVerdict } from '../network/judge';
import type { NetworkEntry } from '../settings/schema';
import { derpName, peerForAddr, peerPath } from '../tailnet/peers';
import type { RouteResult } from './pathStore';

export type RouteRowModel = {
  key: string;
  kind: 'tailnet' | 'intranet' | 'lan';
  title: string;
  addrText: string;
  subtitle: string;
  latencyMs: number | null;
  mark: PathKind;
  inUse: boolean;
  state: 'ok' | 'unreachable' | 'testing' | 'untested';
};

export type EmbeddedRouteInfo = {
  /** 新连接是否走 App 内节点（NetworkStatus.tailnet.active === 'embedded'） */
  active: boolean;
  status: TailnetStatus | null;
};

export type BuildRouteRowsInput = {
  detail: Pick<HostDetail, 'node' | 'addresses'>;
  verdict: NetVerdict;
  networks: NetworkEntry[];
  localAddrs: string[];
  routes: Record<string, RouteResult>;
  inUsePath: NetPath | null;
  derpRegion?: string;
  testing: boolean;
  hasLiveSession?: boolean;
  embedded?: EmbeddedRouteInfo;
};

const KIND_ORDER: Record<RouteRowModel['kind'], number> = {
  intranet: 0,
  lan: 1,
  tailnet: 2,
};

function networkFor(
  addr: string,
  networks: NetworkEntry[],
): NetworkEntry | undefined {
  return networks.find(network =>
    network.cidrs.some(cidr => ipInCidr(addr, cidr)),
  );
}

function rowState(
  result: RouteResult | undefined,
  testing: boolean,
): RouteRowModel['state'] {
  if (result === undefined) {
    return testing ? 'testing' : 'untested';
  }
  return result.outcome === 'ok' ? 'ok' : 'unreachable';
}

function matchesInUsePath(
  inUsePath: NetPath | null,
  kind: RouteRowModel['kind'],
): boolean {
  if (inUsePath === null) {
    return false;
  }
  if (kind === 'tailnet') {
    return inUsePath === 'tailnetDirect' || inUsePath === 'tailnetDerp';
  }
  return inUsePath === kind;
}

export function buildRouteRows(input: BuildRouteRowsInput): RouteRowModel[] {
  const rows: RouteRowModel[] = [];
  const sessionTailnetPath =
    input.hasLiveSession === true &&
    (input.inUsePath === 'tailnetDirect' || input.inUsePath === 'tailnetDerp')
      ? input.inUsePath
      : null;

  for (const address of input.detail.addresses) {
    if (address.kind !== 'intranet' && address.kind !== 'lan') {
      continue;
    }

    const key = `${address.addr}:${address.port}`;
    const result = input.routes[key];
    if (address.kind === 'intranet') {
      rows.push({
        key,
        kind: 'intranet',
        title:
          networkFor(address.addr, input.networks)?.name ??
          deviceRoutes.intranetFallback,
        addrText: key,
        subtitle:
          input.verdict.companyVpn === null
            ? deviceRoutes.vpnDisconnected
            : deviceRoutes.vpnConnected,
        latencyMs:
          result !== undefined && result.outcome === 'ok' ? result.ms : null,
        mark: 'intranet',
        inUse: false,
        state: rowState(result, input.testing),
      });
      continue;
    }

    const sameNetwork = input.localAddrs.some(local =>
      ipInCidr(local, address.cidr ?? ''),
    );
    rows.push({
      key,
      kind: 'lan',
      title: deviceRoutes.lanTitle,
      addrText: key,
      subtitle: sameNetwork
        ? deviceRoutes.sameNetwork
        : deviceRoutes.differentNetwork,
      latencyMs: null,
      mark: 'lan',
      inUse: false,
      state: rowState(result, input.testing),
    });
  }

  for (const address of input.detail.addresses) {
    if (address.kind !== 'tailnet') {
      continue;
    }

    const key = `${address.addr}:${address.port}`;
    const result = input.routes[key];
    const path = result?.path ?? sessionTailnetPath ?? 'tailnetDirect';
    const hasResult = result !== undefined || sessionTailnetPath !== null;
    let title: string = deviceRoutes.tailnetTitle;
    let subtitle =
      path === 'tailnetDerp' && input.derpRegion !== undefined
        ? deviceRoutes.viaRelay(input.detail.node, input.derpRegion)
        : path === 'tailnetDirect' && hasResult
        ? deviceRoutes.viaDirect(input.detail.node)
        : input.detail.node;
    let mark: PathKind = path;
    if (input.embedded?.active === true) {
      const peer = peerForAddr(input.embedded.status, address.addr);
      const kind = peerPath(peer);
      title = deviceRoutes.tailnetEmbeddedTitle;
      if (kind === 'relay') {
        mark = 'tailnetDerp';
        subtitle = deviceRoutes.viaRelay(
          input.detail.node,
          derpName(input.embedded.status, peer?.relay ?? ''),
        );
      } else if (kind === 'direct') {
        mark = 'tailnetDirect';
        subtitle = deviceRoutes.viaDirect(input.detail.node);
      } else if (kind === 'offline') {
        mark = 'offline';
        subtitle = deviceRoutes.peerOffline(input.detail.node);
      }
    }
    rows.push({
      key,
      kind: 'tailnet',
      title,
      addrText: address.addr,
      subtitle,
      latencyMs:
        result !== undefined && result.outcome === 'ok' ? result.ms : null,
      mark,
      inUse: false,
      state: rowState(result, input.testing),
    });
  }

  rows.sort((left, right) => KIND_ORDER[left.kind] - KIND_ORDER[right.kind]);

  const inUse = rows.find(
    row =>
      matchesInUsePath(input.inUsePath, row.kind) &&
      (row.state === 'ok' || input.hasLiveSession === true),
  );
  if (inUse !== undefined) {
    inUse.inUse = true;
  }

  return rows;
}

export function summaryText(
  rows: readonly RouteRowModel[],
  testing: boolean,
): string {
  return testing
    ? deviceRoutes.summaryTesting(rows.length)
    : deviceRoutes.summaryTested(rows.length);
}

export function fastestRoute(
  rows: readonly RouteRowModel[],
): RouteRowModel | null {
  let fastest: RouteRowModel | null = null;
  for (const row of rows) {
    if (row.latencyMs === null || row.state !== 'ok') {
      continue;
    }
    if (fastest === null || row.latencyMs < (fastest.latencyMs ?? Infinity)) {
      fastest = row;
    }
  }
  return fastest;
}

export function autoPolicySubtitle(rows: readonly RouteRowModel[]): string {
  const row = fastestRoute(rows);
  if (row === null || row.latencyMs === null) {
    return deviceRoutes.policies.auto.untested;
  }
  return deviceRoutes.policies.auto.fastest(
    deviceRoutes.policies.auto.kind[row.kind],
    row.latencyMs,
  );
}

export function routeAccessibilityLabel(row: RouteRowModel): string {
  const fields = [row.title, row.addrText, row.subtitle];
  if (row.latencyMs !== null) {
    fields.push(spokenMs(row.latencyMs));
  }
  if (row.inUse) {
    fields.push(deviceRoutes.current);
  }
  return fields.join('，');
}
