import type { Computer } from '../../native/rscore/types';
import { compareVersion } from '../directory/cardModel';

export type VersionInfoInput = {
  code: 170 | 171;
  host?: Computer;
  detail?: Record<string, string>;
  appVersion: string;
  minHost: string;
};

export type VersionInfo = {
  hostVersion: string | null;
  minHost: string;
  target: string | null;
  minApp: string | null;
  canUpgrade: boolean;
};

export function versionInfo(input: VersionInfoInput): VersionInfo {
  const { code, host, detail, minHost } = input;
  const hostVersion = detail?.host_version ?? host?.hostVersion ?? null;
  const requiredHost = detail?.min_version ?? minHost;
  const target = host?.update.available ?? null;
  const minApp = detail?.min_app_version ?? null;
  const canUpgrade =
    code === 170 &&
    host?.online === true &&
    target !== null &&
    compareVersion(target, requiredHost) >= 0;

  return {
    hostVersion,
    minHost: requiredHost,
    target,
    minApp,
    canUpgrade,
  };
}
