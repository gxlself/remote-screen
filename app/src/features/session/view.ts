import { sessionHud } from '../../i18n/zh/sessionHud';
import {
  HOST_FEATURE_VIRTUAL_DISPLAY,
  hasHostFeature,
} from '../../native/rscore/types';
import type { GoodbyeReason, Quality } from '../../native/rscore/types';
import type { SessionRecord } from './store';

export type EndRoute =
  | { kind: 'stay' }
  | { kind: 'goBack'; toast?: string }
  | {
      kind: 'error';
      code?: number;
      reason?: GoodbyeReason;
      detail?: Record<string, string>;
    };

export function endRoute(
  rec: Pick<SessionRecord, 'phase' | 'lastError'>,
): EndRoute {
  if (rec.phase !== 'ended') {
    return { kind: 'stay' };
  }

  const lastError = rec.lastError;
  const reason = lastError?.reason;
  switch (reason) {
    case 'userClosed':
    case 'replaced':
      return { kind: 'goBack' };
    case 'background':
      return { kind: 'goBack', toast: sessionHud.end.backgroundExpiredToast };
    case 'hostUpdating':
      return { kind: 'stay' };
    case 'hostShutdown':
      return {
        kind: 'goBack',
        toast: sessionHud.end.hostShutdownToast,
      };
    case 'revoked':
      return { kind: 'error', code: 180 };
    case 'permissionLost':
      return { kind: 'error', code: 101 };
    case 'protocolError':
      return { kind: 'error', code: 199 };
    case 'hostClosedByLocalUser':
    case 'idleTimeout':
      return { kind: 'error', reason };
    default:
      break;
  }

  const code = lastError?.code;
  if (code !== undefined && code >= 101 && code <= 199) {
    const route: EndRoute = { kind: 'error', code };
    if (lastError?.detail !== undefined) {
      route.detail = lastError.detail;
    }
    return route;
  }
  return { kind: 'error', code: 199 };
}

export type SessionView = {
  overlay: 'opening' | 'connecting' | 'approval' | 'reconnecting' | 'none';
  end: EndRoute;
  role: 'controller' | 'viewer';
  inputBlocked: boolean;
  shared: 'view' | 'taken' | null;
  localUse: boolean;
  relay: boolean;
  tcp: boolean;
  idle: boolean;
  toolbar: {
    displayLabel: string | null;
    displayA11y: string | null;
    qualityLabel: string;
    qualityA11y: string;
    showCtrlAltDel: boolean;
    showModeSwitch: boolean;
    keyboardEnabled: boolean;
  };
};

function overlayFor(rec: SessionRecord | undefined): SessionView['overlay'] {
  if (rec === undefined) {
    return 'opening';
  }
  switch (rec.phase) {
    case 'connecting':
    case 'authenticating':
      return 'connecting';
    case 'pendingApproval':
      return 'approval';
    case 'reconnecting':
      return 'reconnecting';
    case 'ended':
      return rec.lastError?.reason === 'hostUpdating' ? 'reconnecting' : 'none';
    default:
      return 'none';
  }
}

function hasRateReason(reason: string | undefined): boolean {
  return reason !== undefined && reason.trim() !== '';
}

function toolbarFor(
  rec: SessionRecord | undefined,
  ctx: { os: 'macos' | 'windows'; quality: Quality; features?: number },
  role: SessionView['role'],
  inputBlocked: boolean,
): SessionView['toolbar'] {
  const displays = rec?.displays ?? [];
  let displayLabel: string | null = null;
  let displayA11y: string | null = null;
  if (displays.length > 1) {
    const found = displays.findIndex(display => display.id === rec?.displayId);
    const current = (found < 0 ? 0 : found) + 1;
    displayLabel = sessionHud.toolbar.display.label(current, displays.length);
    displayA11y = sessionHud.toolbar.display.a11y(current, displays.length);
  } else if (
    role === 'controller' &&
    hasHostFeature(ctx.features ?? 0, HOST_FEATURE_VIRTUAL_DISPLAY)
  ) {
    // 只有一块屏也给入口：控制者可以新建虚拟屏。没有数字标签，工具栏只放图标。
    displayA11y = sessionHud.toolbar.display.entryA11y;
  }

  const qualityBase = sessionHud.toolbar.quality.labels[ctx.quality];
  const stats = rec?.stats ?? null;
  const rateLimited = hasRateReason(stats?.rateReason);
  const reducedTo1080 = rateLimited && stats !== null && stats.height <= 1080;
  const qualityLabel = reducedTo1080
    ? `${qualityBase}${sessionHud.toolbar.quality.downgradedSuffix}`
    : qualityBase;

  const tcp = rec?.transport === 'tcp';
  const qualityA11y = tcp
    ? sessionHud.toolbar.quality.tcpA11y(qualityBase)
    : rateLimited
    ? sessionHud.toolbar.quality.downgradedA11y(qualityBase)
    : sessionHud.toolbar.quality.a11y(qualityBase);

  return {
    displayLabel,
    displayA11y,
    qualityLabel,
    qualityA11y,
    showCtrlAltDel: ctx.os === 'windows',
    showModeSwitch: true,
    keyboardEnabled: !inputBlocked,
  };
}

export function deriveSessionView(
  rec: SessionRecord | undefined,
  ctx: {
    os: 'macos' | 'windows';
    quality: Quality;
    myDeviceId: string | null;
    /** Host 的 features 位；判断能不能新建虚拟屏。 */
    features?: number;
  },
): SessionView {
  const role = rec?.role ?? 'controller';
  const inputBlocked =
    role === 'viewer' ||
    rec?.inputPausedLocal === true ||
    rec?.phase === 'reconnecting';

  const controllerIsSelf =
    ctx.myDeviceId !== null && rec?.controller?.deviceId === ctx.myDeviceId;
  let shared: SessionView['shared'] = null;
  if (role === 'viewer' && !controllerIsSelf) {
    shared = 'view';
  } else if (role === 'controller' && (rec?.viewers.length ?? 0) > 0) {
    shared = 'taken';
  }

  return {
    overlay: overlayFor(rec),
    end: rec === undefined ? { kind: 'stay' } : endRoute(rec),
    role,
    inputBlocked,
    shared,
    localUse: rec?.inputPausedLocal === true,
    relay: rec?.path === 'tailnetDerp',
    tcp: rec?.transport === 'tcp',
    idle: rec?.idleWarning !== null && rec?.idleWarning !== undefined,
    toolbar: toolbarFor(rec, ctx, role, inputBlocked),
  };
}
