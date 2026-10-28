import { sshKeys } from '../../i18n/zh/sshKeys';
import { common } from '../../i18n/zh/common';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import type {
  SshKey,
  SshServer,
  TrustedHostKey,
} from '../../native/rscore/types';

export type InstallRow = {
  serverId: string;
  name: string;
  subtitle: string;
  installed: boolean;
};

export type InstallRows = {
  rows: InstallRow[];
  tailscaleNames: string[];
};

export type TrustedRow = {
  serverId: string;
  name: string;
  keyType: string;
  fp: string;
  short: string;
};

export function keyKindLabel(kind: SshKey['kind']): string {
  return kind === 'se-p256' ? sshKeys.kind.seP256 : sshKeys.kind.ed25519;
}

export function shortFp(fp: string): string {
  if (fp.length <= 24) {
    return fp;
  }
  const body = fp.startsWith('SHA256:') ? fp.slice('SHA256:'.length) : fp;
  return `SHA256:${body.slice(0, 8)}…${body.slice(-4)}`;
}

export function defaultKeyName(model: string): string {
  const slug = model
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
  return `rs-${slug}`;
}

export function isTailscaleSsh(server: SshServer): boolean {
  return server.auth === 'tailscale' || server.tailscaleSsh === true;
}

export function installRows(
  servers: SshServer[],
  key: SshKey | null,
): InstallRows {
  const rows: InstallRow[] = [];
  const tailscaleNames: string[] = [];

  for (const server of servers) {
    if (isTailscaleSsh(server)) {
      tailscaleNames.push(server.name);
      continue;
    }

    rows.push({
      serverId: server.serverId,
      name: server.name,
      subtitle:
        server.source === 'directory'
          ? sshKeys.install.directorySubtitle(server.os ?? server.host)
          : sshKeys.install.manualSubtitle(server.host, server.jump),
      installed: key?.installedOn.includes(server.serverId) ?? false,
    });
  }

  return { rows, tailscaleNames };
}

function hostKeyTypeLabel(keyType: string): string {
  const normalized = keyType.toLowerCase();
  if (normalized.includes('ed25519')) {
    return 'ED25519';
  }
  if (normalized.includes('ecdsa')) {
    return 'ECDSA';
  }
  return keyType.toUpperCase();
}

export function trustedRows(
  trusted: TrustedHostKey[],
  servers: SshServer[],
): TrustedRow[] {
  const names = new Map(servers.map(server => [server.serverId, server.name]));

  return trusted
    .map(key => ({
      serverId: key.serverId,
      name: names.get(key.serverId) ?? key.serverId,
      keyType: hostKeyTypeLabel(key.keyType),
      fp: key.fp,
      short: shortFp(key.fp),
    }))
    .sort((left, right) => left.name.localeCompare(right.name));
}

export function keyCreatedOn(
  us: number,
  tzOffsetMin = -new Date().getTimezoneOffset(),
): string {
  const date = new Date(Math.floor(us / 1000) + tzOffsetMin * 60_000);
  return common.time.monthDay(date.getUTCMonth() + 1, date.getUTCDate());
}

export function isUnsupported(error: unknown): boolean {
  return error instanceof RsCoreError && error.kind === 'unsupported';
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
