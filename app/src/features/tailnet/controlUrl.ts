export const CONTROL_URL_MAX = 512;

const SCHEME_PREFIX = /^[a-z][a-z0-9+.-]*:\/\//i;
const CONTROL_URL = /^(https?):\/\/([^\s/?#@]+)(\/[^\s?#]*)?$/i;

/**
 * Normalizes the user-facing Headscale URL without relying on RN's partial URL
 * implementation. Only HTTP(S) origins with an optional numeric port are
 * accepted; query strings, fragments and credentials are deliberately not.
 */
export function normalizeControlUrl(input: string): string | null {
  const trimmed = input.trim();
  if (trimmed === '') {
    return null;
  }

  const withScheme = SCHEME_PREFIX.test(trimmed)
    ? trimmed
    : `https://${trimmed}`;
  const match = CONTROL_URL.exec(withScheme);
  if (match === null) {
    return null;
  }

  const scheme = match[1];
  const hostPort = match[2];
  const authority = /^(\[[^\]]+\]|[^:]+)(?::(\d+))?$/.exec(hostPort);
  if (authority === null || !/[a-z0-9]/i.test(authority[1])) {
    return null;
  }
  const portText = authority[2];
  if (portText !== undefined) {
    const port = Number(portText);
    if (!Number.isSafeInteger(port) || port < 1 || port > 65535) {
      return null;
    }
  }

  const path = match[3] ?? '';
  const normalized =
    `${scheme.toLowerCase()}://${hostPort.toLowerCase()}${path}`.replace(
      /\/+$/,
      '',
    );
  return normalized.length <= CONTROL_URL_MAX ? normalized : null;
}
