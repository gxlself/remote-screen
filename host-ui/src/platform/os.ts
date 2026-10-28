import type { HostOs } from './types';

type HostOsGlobal = typeof globalThis & {
  __RS_HOST_OS__?: unknown;
};

export function detectHostOs(): HostOs {
  const override = (globalThis as HostOsGlobal).__RS_HOST_OS__;
  if (override === 'windows' || override === 'macos') {
    return override;
  }

  const userAgent =
    typeof navigator === 'undefined' || typeof navigator.userAgent !== 'string'
      ? ''
      : navigator.userAgent;
  return userAgent.includes('Windows') ? 'windows' : 'macos';
}
