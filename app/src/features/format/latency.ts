import { common } from '../../i18n/zh/common';

const { units } = common;

export function formatMs(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) {
    return '—';
  }
  if (ms < 1) {
    return `<1 ${units.ms}`;
  }
  return `${Math.round(ms)} ${units.ms}`;
}

export function spokenMs(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) {
    return '—';
  }
  if (ms < 1) {
    return `<1 ${units.msSpoken}`;
  }
  return `${Math.round(ms)} ${units.msSpoken}`;
}

export function formatFps(fps: number): string {
  return `${Math.round(fps)} ${units.fps}`;
}

export function formatBitrate(kbps: number): string {
  if (kbps >= 1000) {
    return `${(kbps / 1000).toFixed(1)} Mbps`;
  }
  return `${Math.round(kbps)} kbps`;
}

export function formatResolution(width: number, height: number): string {
  return `${width}×${height}`;
}
