import type { JSX } from 'react';
import { StackActions } from '@react-navigation/native';
import { navigationRef } from '../../app/navigation/navigationRef';
import { adapters } from '../../i18n/zh/adapters';
import { native } from '../../i18n/zh/native';
import { ADAPTER_RS_E } from '../../native/rscore/types';
import type {
  AdapterErrorCode,
  AdapterErrorInfo,
  AdapterProtocol,
} from '../../native/rscore/types';
import { ErrorFrame } from '../../screens/SessionEnded/ErrorFrame';
import type { EndAction } from '../../screens/SessionEnded/boardTypes';
import { entryName, entryProtocol } from '../native/entries';
import { useTailnetEnabled } from '../tailnet/enabled';
import type { ComputerEntry } from '../native/types';
import { useComputerEntries } from '../native/useComputerEntries';

export type AdapterErrorProps = {
  error: AdapterErrorInfo;
  protocol: AdapterProtocol;
  name: string;
  onRetry(): void;
  /** nlaRequired：重连并先问账户密码。 */
  onRetryWithPassword?(): void;
  onBack(): void;
  onOtherApp(): void;
  onCopyCode(): void;
  /** 这个安装包没有内置屏幕共享 / 远程桌面，重试没有用。 */
  unavailable?: boolean;
  /** 证书确认框 60 秒没人答，适配器按拒绝断开。 */
  certTimedOut?: boolean;
};

type ErrorButtons = 'retryOrOtherApp' | 'retry' | 'otherApp' | 'password';

// 10-appendix C11.3; refused / timeout also offer the external app (card P).
function buttonsFor(
  code: AdapterErrorCode,
  canAskPassword: boolean,
): ErrorButtons {
  switch (code) {
    case 'refused':
    case 'timeout':
      return 'retryOrOtherApp';
    case 'nlaRequired':
      return canAskPassword ? 'password' : 'otherApp';
    case 'unsupportedSecurity':
      return 'otherApp';
    default:
      return 'retry';
  }
}

/**
 * The session is opened under the entry's display name (C7), so a unique
 * name and protocol match finds the entry for EnableSharing's retry.
 */
function entryKeyFor(
  entries: ComputerEntry[],
  protocol: AdapterProtocol,
  name: string,
): string | undefined {
  const matches = entries.filter(
    entry => entryProtocol(entry) === protocol && entryName(entry) === name,
  );
  return matches.length === 1 ? matches[0]?.key : undefined;
}

export function AdapterError(p: AdapterErrorProps): JSX.Element {
  const entries = useComputerEntries();
  const tailnetEnabled = useTailnetEnabled();
  const buttons = p.unavailable
    ? 'otherApp'
    : buttonsFor(p.error.code, p.onRetryWithPassword !== undefined);
  const retry: EndAction = { label: adapters.errors.retry, onPress: p.onRetry };
  const otherApp: EndAction = {
    label: adapters.errors.otherApp,
    onPress: p.onOtherApp,
  };
  const password: EndAction | undefined =
    p.onRetryWithPassword === undefined
      ? undefined
      : {
          label: adapters.errors.retryWithPassword,
          onPress: p.onRetryWithPassword,
        };
  const primary =
    buttons === 'otherApp'
      ? otherApp
      : buttons === 'password' && password !== undefined
      ? password
      : retry;
  const secondary =
    buttons === 'retryOrOtherApp' || buttons === 'password'
      ? otherApp
      : undefined;
  // V10 C8: 205 means the computer is up with sharing off; show how to turn
  // it on before the external app. Replace this page so that leaving the
  // guide does not land on the stale error again.
  const howTo: EndAction | undefined =
    !p.unavailable && p.error.rsCode === ADAPTER_RS_E.serviceOff
      ? {
          label: native.enable.howTo,
          onPress: () => {
            const entryKey = entryKeyFor(entries, p.protocol, p.name);
            navigationRef.dispatch(
              StackActions.replace('EnableSharing', {
                os: p.protocol === 'rdp' ? 'windows' : 'macos',
                name: p.name,
                ...(entryKey === undefined ? {} : { entryKey }),
              }),
            );
          },
        }
      : undefined;
  const title = p.unavailable
    ? adapters.errors.unavailableTitle
    : p.certTimedOut && p.error.code === 'certRejected'
    ? adapters.errors.certTimeoutTitle
    : adapters.errors.title(p.error.code, p.name, p.protocol);
  const desc = p.unavailable
    ? adapters.errors.unavailableMessage
    : p.certTimedOut && p.error.code === 'certRejected'
    ? adapters.errors.certTimeoutMessage
    : adapters.errors.message(p.error.code, p.protocol, tailnetEnabled);
  return (
    <ErrorFrame
      back={{ label: adapters.errors.back, onPress: p.onBack }}
      code={p.error.rsCode > 0 ? p.error.rsCode : null}
      desc={desc}
      kicker={adapters.title(p.protocol)}
      more={howTo === undefined || secondary === undefined ? [] : [secondary]}
      onCopyCode={p.onCopyCode}
      primary={primary}
      secondary={howTo ?? secondary}
      testID="adapter-error"
      title={title}
    />
  );
}

export default AdapterError;
