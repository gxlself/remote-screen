import type { IconName } from '../../components/icons/Icon';
import { common } from '../../i18n/zh/common';
import { pairing } from '../../i18n/zh/pairing';
import type { PairOutcome } from '../../native/rscore/types';
import { isTailnetEnabled } from '../tailnet/enabled';

export type PairMethod = 'qr' | 'code' | 'file';

export type FailureModel = {
  icon: IconName;
  tone: 'bad' | 'warn';
  title: string;
  message: string;
  primary: {
    label: string;
    action:
      | 'reenterCode'
      | 'rescan'
      | 'retry'
      | 'close'
      | 'reimportFile'
      | 'otherMethod';
  };
  secondary?: { label: string; action: 'close' | 'help' };
};

export function describeFailure(
  outcome: PairOutcome,
  hostName: string,
  os: 'macos' | 'windows' | null,
  method: PairMethod = 'code',
): FailureModel {
  const model = failureModel(outcome, hostName, os, method);
  if (model.primary.action !== 'close') {
    return model;
  }
  // The primary button already closes; a second 关闭 would be a duplicate.
  return {
    icon: model.icon,
    tone: model.tone,
    title: model.title,
    message: model.message,
    primary: model.primary,
  };
}

function failureModel(
  outcome: PairOutcome,
  hostName: string,
  os: 'macos' | 'windows' | null,
  method: PairMethod,
): FailureModel {
  const host = pairing.hostWord(os);
  const secondary: FailureModel['secondary'] = {
    label: common.close,
    action: 'close',
  };
  // A code typed by hand is typed again (the address stays filled in); only a
  // scanned QR goes back to the camera.
  const again: FailureModel['primary'] =
    method === 'qr'
      ? { label: pairing.actions.rescan, action: 'rescan' }
      : { label: pairing.actions.reenterCode, action: 'reenterCode' };
  const codeMethod = method === 'qr' ? 'qr' : 'code';

  switch (outcome.status) {
    case 'badCode': {
      if (method === 'file') {
        const exhausted = outcome.attemptsLeft <= 0;
        return {
          icon: 'alert',
          tone: 'warn',
          title: pairing.failure.file.badCode.title,
          message: exhausted
            ? pairing.failure.file.badCode.exhausted
            : pairing.failure.file.badCode.attempts(outcome.attemptsLeft),
          primary: {
            label: pairing.actions.reimportFile,
            action: 'reimportFile',
          },
          secondary,
        };
      }
      const exhausted = outcome.attemptsLeft <= 0;
      return {
        icon: 'alert',
        tone: 'warn',
        title: pairing.failure.badCode.title,
        message: exhausted
          ? pairing.failure.badCode.invalidCode(host)
          : pairing.attemptsLeft(outcome.attemptsLeft),
        primary: exhausted
          ? again
          : { label: pairing.actions.reenterCode, action: 'reenterCode' },
        secondary,
      };
    }
    case 'expired':
      if (method === 'file') {
        return {
          icon: 'clock',
          tone: 'warn',
          title: pairing.failure.file.expired.title,
          message: pairing.failure.file.expired.message,
          primary: {
            label: pairing.actions.reimportFile,
            action: 'reimportFile',
          },
          secondary,
        };
      }
      return {
        icon: 'clock',
        tone: 'warn',
        title: pairing.failure.expired.title,
        message: pairing.failure.expired.message(host, codeMethod),
        primary: again,
        secondary,
      };
    case 'denied':
      return {
        icon: 'blocked',
        tone: 'warn',
        title: pairing.failure.denied.title(host),
        message: pairing.failure.denied.message(hostName),
        primary: again,
        secondary,
      };
    case 'timeout':
      return {
        icon: 'clock',
        tone: 'warn',
        title: pairing.failure.timeout.title,
        message: pairing.failure.timeout.message(host, codeMethod),
        primary: again,
        secondary,
      };
    case 'locked': {
      const lockedMessage =
        outcome.retryAfterS === undefined
          ? pairing.failure.locked.message
          : pairing.failure.locked.retryIn(
              formatRetryAfter(outcome.retryAfterS),
            );
      return {
        icon: 'lock',
        tone: 'bad',
        title: pairing.failure.locked.title,
        message: lockedMessage,
        primary: { label: common.close, action: 'close' },
        secondary,
      };
    }
    case 'methodDisabled':
      return {
        icon: 'wifi',
        tone: 'warn',
        title: pairing.failure.methodDisabled.title,
        message: pairing.failure.methodDisabled.message,
        primary: {
          label: pairing.actions.otherMethod,
          action: 'otherMethod',
        },
        secondary,
      };
    case 'disabledOnPath':
      return {
        icon: 'wifi',
        tone: 'warn',
        title: pairing.failure.disabledOnPath.title,
        message: isTailnetEnabled()
          ? pairing.failure.disabledOnPath.message
          : pairing.failure.disabledOnPath.messageLan,
        primary: { label: common.close, action: 'close' },
        secondary,
      };
    case 'mitm':
      return {
        icon: 'shield',
        tone: 'bad',
        title: pairing.failure.mitm.title,
        message: pairing.failure.mitm.message,
        primary: { label: common.close, action: 'close' },
        secondary,
      };
    case 'error':
      if (method === 'file' && outcome.code === 170) {
        return {
          icon: 'warning',
          tone: 'warn',
          title: pairing.failure.file.hostTooOld.title,
          message: pairing.failure.file.hostTooOld.message,
          primary: {
            label: pairing.actions.otherMethod,
            action: 'otherMethod',
          },
          secondary,
        };
      }
      if (method === 'file' && outcome.message === 'pairing file not loaded') {
        return {
          icon: 'warning',
          tone: 'warn',
          title: pairing.failure.file.notLoaded.title,
          message: pairing.failure.file.notLoaded.message,
          primary: {
            label: pairing.actions.reimportFile,
            action: 'reimportFile',
          },
          secondary,
        };
      }
      return {
        icon: 'warning',
        tone: 'warn',
        title: pairing.failure.error.title,
        message: userFacingErrorMessage(outcome.message),
        primary: { label: common.retry, action: 'retry' },
        secondary,
      };
    case 'cancelled':
      throw new Error('Cancelled pairing does not have a failure page');
    case 'ok':
      throw new Error('Successful pairing does not have a failure page');
  }
}

export function formatRetryAfter(seconds: number): string {
  if (seconds <= 0) {
    return pairing.retryAfter.seconds(0);
  }
  if (seconds < 60) {
    return pairing.retryAfter.seconds(Math.ceil(seconds));
  }
  return pairing.retryAfter.minutes(Math.ceil(seconds / 60));
}

function userFacingErrorMessage(message: string): string {
  const trimmed = message.trim();
  if (trimmed.length === 0) {
    return pairing.failure.error.fallbackMessage;
  }
  if (/[A-Za-z]/.test(trimmed) && !/[\u3400-\u9fff]/.test(trimmed)) {
    return pairing.failure.error.fallbackMessage;
  }
  return trimmed;
}
