import { sshPrompt } from '../../i18n/zh/sshPrompt';
import type { SshServer } from '../../native/rscore/types';

export type CheckStepState = 'done' | 'active' | 'pending';

export type CheckStepText = {
  faceIdAsk: (authName: string) => string;
  faceIdDone: (authName: string) => string;
  browser: (host: string, waiting: boolean) => string;
  backToTerminal: string;
};

export type CheckStep = {
  key: 'faceId' | 'browser' | 'return';
  title: string;
  state: CheckStepState;
};

const SHA256_PREFIX = 'SHA256:';

function groupFp(value: string): string[] {
  return value.match(/.{1,4}/g) ?? [];
}

export function formatFp(fp: string): string[] {
  if (!fp.startsWith(SHA256_PREFIX)) {
    return groupFp(fp);
  }
  return [SHA256_PREFIX, ...groupFp(fp.slice(SHA256_PREFIX.length))];
}

export function spokenFp(fp: string): string {
  return formatFp(fp).join('，');
}

export function authUrlHost(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

export function checkSteps(
  step: 'ask' | 'wait',
  host: string,
  authName = 'Face ID',
  t: CheckStepText = sshPrompt.rootConfirm.steps,
): CheckStep[] {
  const states: CheckStepState[] =
    step === 'ask'
      ? ['active', 'pending', 'pending']
      : ['done', 'active', 'pending'];

  return [
    {
      key: 'faceId',
      title: step === 'ask' ? t.faceIdAsk(authName) : t.faceIdDone(authName),
      state: states[0],
    },
    {
      key: 'browser',
      title: t.browser(host, step === 'wait'),
      state: states[1],
    },
    {
      key: 'return',
      title: t.backToTerminal,
      state: states[2],
    },
  ];
}

export function sshServerName(
  servers: readonly SshServer[] | undefined,
  serverId: string,
): string {
  return (
    servers?.find(server => server.serverId === serverId)?.name ?? serverId
  );
}
