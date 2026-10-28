import type { PathKind } from '../../components/PathMark';
import { common } from '../../i18n/zh/common';
import { qualityDisplay } from '../../i18n/zh/qualityDisplay';
import type {
  CoreEventOf,
  DialAttempt,
  Quality,
} from '../../native/rscore/types';
import {
  formatBitrate,
  formatFps,
  formatMs,
  formatResolution,
} from '../format/latency';

export const QUALITY_OPTIONS: Quality[] = ['auto', 'smooth', 'clear', 'eco'];

const statsText = qualityDisplay.stats;

export function qualityDesc(quality: Quality): string {
  return qualityDisplay.quality.desc[quality];
}

export function statRows(
  stats: CoreEventOf<'session.stats'> | null,
  _dialOk?: DialAttempt,
): { label: string; value: string }[] {
  if (stats === null) {
    return [
      { label: statsText.labels.latency, value: statsText.empty },
      { label: statsText.labels.fps, value: statsText.empty },
      { label: statsText.labels.bitrate, value: statsText.empty },
      { label: statsText.labels.resolution, value: statsText.empty },
      { label: statsText.labels.loss, value: statsText.empty },
      { label: statsText.labels.codec, value: statsText.empty },
    ];
  }

  return [
    { label: statsText.labels.latency, value: formatMs(stats.rttMs) },
    { label: statsText.labels.fps, value: formatFps(stats.fps) },
    {
      label: statsText.labels.bitrate,
      value: formatBitrate(stats.bitrateKbps),
    },
    {
      label: statsText.labels.resolution,
      value: formatResolution(stats.width, stats.height),
    },
    { label: statsText.labels.loss, value: `${stats.lossPct.toFixed(1)}%` },
    {
      label: statsText.labels.codec,
      value: statsText.codec[stats.codec],
    },
  ];
}

function spokenValue(value: string, label: string): string {
  if (value === statsText.empty) {
    return value;
  }
  if (label === statsText.labels.latency) {
    return value.replace(` ${common.units.ms}`, ` ${common.units.msSpoken}`);
  }
  if (label === statsText.labels.fps) {
    return value.replace(` ${common.units.fps}`, '');
  }
  return value;
}

export function statsLabel(rows: { label: string; value: string }[]): string {
  const values = rows.map(row => spokenValue(row.value, row.label));
  return statsText.a11y(
    values[0],
    values[1],
    values[2],
    values[3],
    values[4],
    values[5],
  );
}

function okDial(dialOk: DialAttempt | undefined): DialAttempt | null {
  return dialOk?.state === 'ok' ? dialOk : null;
}

function attemptedPath(dialOk: DialAttempt | undefined): DialAttempt | null {
  if (dialOk === undefined || dialOk.addr.trim() === '') {
    return null;
  }
  return okDial(dialOk);
}

export function pathLine(
  stats: CoreEventOf<'session.stats'> | null,
  dialOk?: DialAttempt,
): string {
  const attempt = attemptedPath(dialOk);
  const path = stats?.path ?? attempt?.path ?? 'offline';
  const pathName = common.path[path];
  return attempt === null ? pathName : `${pathName} ${attempt.addr}`;
}

export function dialPath(
  stats: CoreEventOf<'session.stats'> | null,
  dialOk?: DialAttempt,
): PathKind {
  return stats?.path ?? attemptedPath(dialOk)?.path ?? 'offline';
}
