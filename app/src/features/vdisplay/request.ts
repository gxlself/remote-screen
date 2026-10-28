import { vdisplay } from '../../i18n/zh/vdisplay';
import {
  hasHostFeature,
  HOST_FEATURE_VIRTUAL_DISPLAY,
} from '../../native/rscore/types';
import type {
  VirtualDisplayOpts,
  VdisplayStatus,
} from '../../native/rscore/types';

export type ScreenInfo = {
  width: number;
  height: number;
  pixelRatio: number;
};

/** 新建虚拟屏请求在没有结果时的最长等待时间。 */
export const VDISPLAY_TIMEOUT_MS = 15_000;

/** 把本机屏幕尺寸转换成 Host 使用的横向虚拟屏模式。 */
export function virtualDisplayOpts(screen: ScreenInfo): VirtualDisplayOpts {
  const longSide = Math.max(screen.width, screen.height);
  const shortSide = Math.min(screen.width, screen.height);
  const scale = screen.pixelRatio >= 2 ? 2 : 1;
  return {
    widthPx: Math.round(longSide * screen.pixelRatio),
    heightPx: Math.round(shortSide * screen.pixelRatio),
    scale,
    refreshHz: 60,
    select: true,
  };
}

export function canCreate(args: {
  features: number | undefined;
  role: 'controller' | 'viewer' | null | undefined;
  sessionId: string | null;
  ownedId: number | null;
}): boolean {
  return (
    hasHostFeature(args.features ?? 0, HOST_FEATURE_VIRTUAL_DISPLAY) &&
    args.role === 'controller' &&
    args.sessionId !== null &&
    args.ownedId === null
  );
}

export function statusText(status: VdisplayStatus | 'timeout'): string {
  switch (status) {
    case 'unsupported':
      return vdisplay.error.unsupported;
    case 'notAllowed':
      return vdisplay.error.notAllowed;
    case 'limit':
      return vdisplay.error.limit;
    case 'badRequest':
      return vdisplay.error.badRequest;
    case 'timeout':
      return vdisplay.error.timeout;
    case 'ok':
      return vdisplay.created;
    case 'failed':
    default:
      return vdisplay.error.failed;
  }
}
