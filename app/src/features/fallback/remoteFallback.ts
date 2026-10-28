import type { Address, Computer } from '../../native/rscore/types';
import { currentOs, playStoreUrls, type AppOs } from '../platform/externalApps';

export type FallbackKind = 'rdp' | 'vnc';

export const FALLBACK_APPS = {
  rdp: {
    name: 'Windows App',
    storeId: '714464092',
    androidPackage: 'com.microsoft.rdc.androidx',
  },
  vnc: {
    name: 'VNC Viewer',
    storeId: '352019548',
    androidPackage: 'com.realvnc.viewer.android',
  },
} as const;

export function fallbackKind(os: Computer['os']): FallbackKind {
  return os === 'windows' ? 'rdp' : 'vnc';
}

export function urlHost(addr: string): string {
  const unbracketed =
    addr.startsWith('[') && addr.endsWith(']') ? addr.slice(1, -1) : addr;
  const escaped = unbracketed.replace(/%/g, '%25');
  return escaped.includes(':') ? `[${escaped}]` : escaped;
}

export function rdpUrl(addr: string, port = 3389): string {
  return `rdp://full%20address=s:${urlHost(addr)}:${port}`;
}

export function vncUrl(addr: string, port?: number): string {
  const suffix = port !== undefined && port !== 5900 ? `:${port}` : '';
  return `vnc://${urlHost(addr)}${suffix}`;
}

export function fallbackUrl(
  kind: FallbackKind,
  addr: string,
  port?: number,
): string {
  return kind === 'rdp' ? rdpUrl(addr, port) : vncUrl(addr, port);
}

export function pickAddress(addresses: readonly Address[]): string | null {
  for (const kind of ['tailnet', 'intranet', 'lan'] as const) {
    const address = addresses.find(candidate => candidate.kind === kind);
    if (address !== undefined) {
      return address.addr;
    }
  }
  return null;
}

export function storeUrl(kind: FallbackKind): string {
  return `itms-apps://apps.apple.com/app/id${FALLBACK_APPS[kind].storeId}`;
}

export function storeUrls(
  kind: FallbackKind,
  os: AppOs = currentOs(),
): readonly string[] {
  return os === 'android'
    ? playStoreUrls(FALLBACK_APPS[kind].androidPackage)
    : [storeUrl(kind)];
}
