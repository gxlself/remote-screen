import type {
  CoreEventOf,
  SshAuth,
  SshId,
  SshServer,
} from '../../native/rscore/types';
import { terminal as copy } from '../../i18n/zh/terminal';
import type { SshRecord } from '../sessions/sshStore';

export type SshState = CoreEventOf<'ssh.state'>['state'];

export type TermTab = {
  key: string;
  serverId: string;
  sshId: SshId | null;
  state: SshState | null;
  message: string | null;
  openError: string | null;
  pendingCheck: boolean;
  prefill: string | null;
  prefillSent: boolean;
  // Set once the user reconnects the tab, so tmux reads as re-attached.
  reopened: boolean;
};

export type TabsState = {
  tabs: TermTab[];
  activeKey: string | null;
  seq: number;
};

export type TabsAction =
  | { type: 'open'; serverId: string; prefill?: string | null }
  | {
      type: 'attach';
      serverId: string;
      sshId: SshId;
      state: SshState;
      activate?: boolean;
    }
  | { type: 'opened'; key: string; sshId: SshId }
  | { type: 'openFailed'; key: string; message: string }
  | { type: 'reopen'; key: string }
  | { type: 'close'; key: string }
  | { type: 'activate'; key: string }
  | { type: 'authUrl'; sshId: SshId }
  | { type: 'sshState'; sshId: SshId; state: SshState; message?: string }
  | { type: 'prefillSent'; key: string };

const EMPTY_TABS: TabsState = { tabs: [], activeKey: null, seq: 0 };

function nextTab(tab: TermTab, patch: Partial<TermTab>): TermTab {
  return { ...tab, ...patch };
}

function replaceTab(
  state: TabsState,
  key: string,
  change: (tab: TermTab) => TermTab,
): TabsState {
  let changed = false;
  const tabs = state.tabs.map(tab => {
    if (tab.key !== key) {
      return tab;
    }
    const next = change(tab);
    changed = next !== tab;
    return next;
  });
  return changed ? { ...state, tabs } : state;
}

function appendTab(
  state: TabsState,
  serverId: string,
  fields: Partial<Omit<TermTab, 'key' | 'serverId'>> = {},
): { state: TabsState; key: string } {
  const seq = state.seq + 1;
  const key = `t${seq}`;
  const tab: TermTab = {
    key,
    serverId,
    sshId: null,
    state: null,
    message: null,
    openError: null,
    pendingCheck: false,
    prefill: null,
    prefillSent: false,
    reopened: false,
    ...fields,
  };
  return {
    key,
    state: {
      tabs: [...state.tabs, tab],
      activeKey: state.activeKey,
      seq,
    },
  };
}

export function initTabs(
  p: { serverId: string; sshId?: string; prefill?: string },
  live: SshRecord[],
): TabsState {
  let state = EMPTY_TABS;
  for (const record of live) {
    state = tabsReducer(state, {
      type: 'attach',
      serverId: record.serverId,
      sshId: record.sshId,
      state: record.state,
    });
  }

  const requestedSshId = p.sshId;
  const requested = live.find(record => record.sshId === requestedSshId);
  if (requested !== undefined) {
    const tab = matchTab(state, requested.sshId, requested.serverId);
    return tab === null
      ? state
      : tabsReducer(state, {
          type: 'activate',
          key: tab.key,
        });
  }

  const hasPrefill = p.prefill !== undefined && p.prefill !== '';
  const hasSshId = requestedSshId !== undefined && requestedSshId !== '';
  if (!hasPrefill && !hasSshId) {
    const sameServer = live.find(record => record.serverId === p.serverId);
    if (sameServer !== undefined) {
      const tab = matchTab(state, sameServer.sshId, sameServer.serverId);
      return tab === null
        ? state
        : tabsReducer(state, {
            type: 'activate',
            key: tab.key,
          });
    }
  }

  return tabsReducer(state, {
    type: 'open',
    serverId: p.serverId,
    prefill: p.prefill ?? null,
  });
}

export function tabsReducer(state: TabsState, action: TabsAction): TabsState {
  switch (action.type) {
    case 'open': {
      const appended = appendTab(state, action.serverId, {
        prefill: action.prefill ?? null,
      });
      return { ...appended.state, activeKey: appended.key };
    }
    case 'attach': {
      const attached = state.tabs.find(tab => tab.sshId === action.sshId);
      if (attached !== undefined) {
        return action.activate === true
          ? { ...state, activeKey: attached.key }
          : state;
      }
      const appended = appendTab(state, action.serverId, {
        sshId: action.sshId,
        state: action.state,
      });
      return {
        ...appended.state,
        activeKey:
          action.activate === true ? appended.key : appended.state.activeKey,
      };
    }
    case 'opened':
      return replaceTab(state, action.key, tab =>
        tab.sshId === action.sshId
          ? tab
          : nextTab(tab, { sshId: action.sshId }),
      );
    case 'openFailed':
      return replaceTab(state, action.key, tab =>
        tab.openError === action.message
          ? tab
          : nextTab(tab, { openError: action.message }),
      );
    case 'reopen':
      return replaceTab(state, action.key, tab =>
        nextTab(tab, {
          sshId: null,
          state: null,
          message: null,
          openError: null,
          pendingCheck: false,
          reopened: true,
        }),
      );
    case 'close': {
      const index = state.tabs.findIndex(tab => tab.key === action.key);
      if (index === -1) {
        return state;
      }
      const tabs = state.tabs.filter(tab => tab.key !== action.key);
      if (state.activeKey !== action.key) {
        return { ...state, tabs };
      }
      const neighbor = tabs[index] ?? tabs[index - 1] ?? null;
      return { ...state, tabs, activeKey: neighbor?.key ?? null };
    }
    case 'activate':
      return state.tabs.some(tab => tab.key === action.key)
        ? { ...state, activeKey: action.key }
        : state;
    case 'authUrl':
      return replaceTab(
        state,
        state.tabs.find(tab => tab.sshId === action.sshId)?.key ?? '',
        tab => (tab.pendingCheck ? tab : nextTab(tab, { pendingCheck: true })),
      );
    case 'sshState': {
      const key = state.tabs.find(tab => tab.sshId === action.sshId)?.key;
      if (key === undefined) {
        return state;
      }
      return replaceTab(state, key, tab => {
        const terminal =
          action.state === 'open' ||
          action.state === 'closed' ||
          action.state === 'failed';
        const message = action.message ?? null;
        if (
          tab.state === action.state &&
          tab.message === message &&
          (!terminal || !tab.pendingCheck)
        ) {
          return tab;
        }
        return nextTab(tab, {
          state: action.state,
          message,
          pendingCheck: terminal ? false : tab.pendingCheck,
        });
      });
    }
    case 'prefillSent':
      return replaceTab(state, action.key, tab =>
        tab.prefillSent ? tab : nextTab(tab, { prefillSent: true }),
      );
  }
}

export function matchTab(
  state: TabsState,
  sshId: SshId,
  serverId: string,
): TermTab | null {
  const exact = state.tabs.find(tab => tab.sshId === sshId);
  if (exact !== undefined) {
    return exact;
  }
  return (
    state.tabs.find(
      tab =>
        tab.sshId === null &&
        tab.openError === null &&
        tab.serverId === serverId,
    ) ?? null
  );
}

export type TermPhase =
  | 'opening'
  | 'connecting'
  | 'authenticating'
  | 'check'
  | 'open'
  | 'failed'
  | 'closed';

export function tabPhase(tab: TermTab): TermPhase {
  if (tab.openError !== null) {
    return 'failed';
  }
  if (tab.sshId === null) {
    return 'opening';
  }
  if (tab.state === null) {
    return 'connecting';
  }
  if (tab.state === 'authenticating' && tab.pendingCheck) {
    return 'check';
  }
  return tab.state;
}

export type TermStatus = {
  phase: TermPhase;
  auth: SshAuth | null;
  jump: string | null;
  tmux: boolean;
  // tmux after a reconnect: the old tmux session was re-attached.
  resumed: boolean;
  message: string | null;
  canReconnect: boolean;
  tone: 'good' | 'warn' | 'bad' | 'muted';
};

export function termStatus(
  tab: TermTab,
  server: SshServer | undefined,
  tmuxSetting: boolean,
): TermStatus {
  const phase = tabPhase(tab);
  const result: TermStatus = {
    phase,
    auth: server?.auth ?? null,
    jump: server?.jump ?? null,
    tmux: tmuxSetting && phase === 'open',
    resumed: tmuxSetting && phase === 'open' && tab.reopened,
    message: null,
    canReconnect: false,
    tone: 'muted',
  };

  switch (phase) {
    case 'open':
      result.tone = 'good';
      break;
    case 'check':
      result.tone = 'warn';
      break;
    case 'failed':
      result.tone = 'bad';
      result.message = tab.openError ?? tab.message;
      result.canReconnect = true;
      break;
    case 'closed':
      result.message = tab.message;
      result.canReconnect = true;
      break;
  }

  return result;
}

export type FailureKind = 'auth' | 'network' | 'other';

const AUTH_FAILURE = /auth|denied|publickey|password|credential/i;
const NETWORK_FAILURE =
  /timed? ?out|timeout|unreachable|no route|refused|reset|resolve|dns|network|host not found|eof|broken pipe/i;

/** Sorts a core failure message into a next step; null without a message. */
export function failureKind(message: string | null): FailureKind | null {
  if (message === null || message.trim() === '') {
    return null;
  }
  if (AUTH_FAILURE.test(message)) {
    return 'auth';
  }
  return NETWORK_FAILURE.test(message) ? 'network' : 'other';
}

export function shouldSendPrefill(tab: TermTab): boolean {
  return (
    tab.prefill !== null &&
    tab.prefill.trim() !== '' &&
    !tab.prefillSent &&
    tabPhase(tab) === 'open'
  );
}

/**
 * The server name; with `tabs`, a second tab on the same server reads
 * `name 2`, a third `name 3`.
 */
export function tabTitle(
  tab: TermTab,
  servers: SshServer[],
  tabs?: readonly TermTab[],
): string {
  const name =
    servers.find(server => server.serverId === tab.serverId)?.name ??
    tab.serverId;
  if (tabs === undefined) {
    return name;
  }
  const same = tabs.filter(item => item.serverId === tab.serverId);
  return copy.tabName(name, same.indexOf(tab) + 1);
}
