import { filesErrors } from '../../i18n/zh/filesErrors';
import { RsCoreError } from '../../native/rscore/RsCoreApi';

const MESSAGES: Readonly<Record<string, string>> = filesErrors;

function messageFor(message: string): string | undefined {
  const separator = message.indexOf(':');
  const code = (
    separator === -1 ? message : message.slice(0, separator)
  ).trim();
  return MESSAGES[code];
}

export function friendlyError(error: unknown): unknown {
  if (!(error instanceof RsCoreError)) {
    return error;
  }
  const message = messageFor(error.message);
  if (message === undefined) {
    return error;
  }
  return new RsCoreError(
    error.kind,
    message,
    error.httpStatus,
    error.hubCode,
    error.current,
  );
}
