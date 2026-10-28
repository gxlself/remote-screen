import type { PathKind } from '../../components/PathMark';
import { spokenMs } from '../format/latency';
import { sessionHud } from '../../i18n/zh/sessionHud';
import type { SessionRecord } from './store';

export type HudModel = {
  path: PathKind;
  pathText: string;
  latencyMs: number | null;
  fps: number | null;
  tcp: boolean;
  alertText: string | null;
  a11y: string;
};

function regionOf(rec: SessionRecord | undefined): string | null {
  const region = rec?.stats?.derpRegion ?? rec?.derpRegion;
  if (region === undefined || region === null || region.trim() === '') {
    return null;
  }
  return region.trim();
}

function pathTextFor(
  path: PathKind,
  region: string | null,
  tcp: boolean,
): string {
  switch (path) {
    case 'tailnetDirect':
      return sessionHud.hud.path.tailnetDirect;
    case 'tailnetDerp':
      return region === null
        ? sessionHud.hud.path.tailnetDerp
        : sessionHud.hud.path.relayWithRegion(region);
    case 'intranet':
      return tcp
        ? sessionHud.hud.path.intranetTcp
        : sessionHud.hud.path.intranet;
    case 'lan':
      return sessionHud.hud.path.lan;
    case 'manual':
      return sessionHud.hud.path.manual;
    case 'offline':
      return sessionHud.hud.path.offline;
  }
}

function a11yFor(
  path: PathKind,
  region: string | null,
  latency: string,
  fps: string,
  tcp: boolean,
): string {
  switch (path) {
    case 'tailnetDirect':
      return sessionHud.hud.a11y.direct(latency, fps, tcp);
    case 'tailnetDerp':
      return sessionHud.hud.a11y.relay(region, latency, fps, tcp);
    case 'intranet':
      return sessionHud.hud.a11y.intranet(latency, fps, tcp);
    case 'lan':
      return sessionHud.hud.a11y.lan(latency, fps, tcp);
    case 'manual':
      return sessionHud.hud.a11y.manual(latency, fps, tcp);
    case 'offline':
      return sessionHud.hud.a11y.offline;
  }
}

export function hudModel(rec: SessionRecord | undefined): HudModel {
  const stats = rec?.stats ?? null;
  const path: PathKind = stats?.path ?? rec?.path ?? 'offline';
  const region = regionOf(rec);
  const tcp = (stats?.transport ?? rec?.transport) === 'tcp';
  const latencyMs = stats?.rttMs ?? null;
  const fps = stats?.fps ?? null;
  const rateReason = stats?.rateReason;
  const alertText =
    stats !== null && rateReason !== undefined && rateReason.trim() !== ''
      ? sessionHud.hud.alert
      : null;
  const a11y =
    stats === null
      ? sessionHud.hud.measuring
      : a11yFor(
          path,
          region,
          spokenMs(stats.rttMs),
          sessionHud.hud.fps(stats.fps),
          tcp,
        );

  return {
    path,
    pathText: pathTextFor(path, region, tcp),
    latencyMs,
    fps,
    tcp,
    alertText,
    a11y,
  };
}
