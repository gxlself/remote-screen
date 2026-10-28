import { common } from '../../i18n/zh/common';
import { localIdleReconnect } from '../../i18n/zh/localIdleReconnect';
import type {
  DialAttempt,
  GoodbyeReason,
  NetPath,
} from '../../native/rscore/types';
import { formatHm } from '../format/time';
import type { SessionRecord } from './store';

const COPY = localIdleReconnect.reconnect;
const RECENT_NETWORK_WINDOW_MS = 5_000;

export type ReconnectStepState = 'done' | 'active' | 'pending';

export type ReconnectStep = {
  label: string;
  state: ReconnectStepState;
  detail: string | null;
};

export type ReconnectViewModel = {
  title: string;
  desc: string;
  steps: ReconnectStep[];
  waitedText: string;
  showTerminal: boolean;
};

export type ReconnectViewInput = {
  reason: SessionRecord['reconnectReason'];
  endedReason?: GoodbyeReason;
  os: 'macos' | 'windows';
  dial: DialAttempt[];
  sinceMs: number;
  nowMs: number;
  netChangedRecently: boolean;
  /** 这台电脑有没有可用的 SSH 服务器；没有就不给「改用终端」。不传按有。 */
  hasTerminal?: boolean;
};

export function formatReconnectClock(ms: number): string {
  const seconds = String(new Date(ms).getSeconds()).padStart(2, '0');
  return `${formatHm(ms * 1000)}:${seconds}`;
}

export function netChangedRecently(
  updatedAtUs: number | null,
  sinceMs: number,
): boolean {
  return (
    updatedAtUs !== null &&
    Math.abs(updatedAtUs - sinceMs * 1000) <= RECENT_NETWORK_WINDOW_MS * 1000
  );
}

function successfulPathDetail(dial: DialAttempt[]): string | null {
  const paths: NetPath[] = [];
  for (const attempt of dial) {
    if (attempt.state === 'ok' && !paths.includes(attempt.path)) {
      paths.push(attempt.path);
    }
  }
  if (paths.length === 0) {
    return null;
  }
  return paths.map(path => `· ${common.path[path]}`).join(' ');
}

export function reconnectView(input: ReconnectViewInput): ReconnectViewModel {
  const updating =
    input.reason === 'hostUpdating' || input.endedReason === 'hostUpdating';
  const tunnelDone = input.dial.some(attempt => attempt.state === 'ok');

  let title: string;
  let desc: string;
  if (updating) {
    title = COPY.hostUpdating.title;
    desc = COPY.hostUpdating.description;
  } else if (input.reason === 'resume') {
    title = COPY.resume.title;
    desc = COPY.resume.description;
  } else if (input.netChangedRecently) {
    title = COPY.reconnecting.title;
    desc = COPY.reconnecting.networkChanged[input.os];
  } else {
    title = COPY.reconnecting.title;
    desc = COPY.reconnecting.pathLost[input.os];
  }

  const waitedSeconds = Math.max(0, input.nowMs - input.sinceMs) / 1000;

  return {
    title,
    desc,
    steps: [
      {
        label: tunnelDone ? COPY.steps.tunnel.done : COPY.steps.tunnel.active,
        state: tunnelDone ? 'done' : 'active',
        detail: successfulPathDetail(input.dial),
      },
      {
        label: tunnelDone ? COPY.steps.host.active : COPY.steps.host.pending,
        state: tunnelDone ? 'active' : 'pending',
        detail: null,
      },
      {
        label: COPY.steps.frame.pending,
        state: 'pending',
        detail: null,
      },
    ],
    waitedText: COPY.waited(waitedSeconds),
    showTerminal: !updating && input.hasTerminal !== false,
  };
}
