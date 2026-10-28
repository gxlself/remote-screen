const EM_DASH = '—';

function pad2(value: number): string {
  return String(value).padStart(2, '0');
}

export function decodeBase64Url(
  value: string | null | undefined,
  allowPadding = true,
): Uint8Array | null {
  if (value === null || value === undefined || value.length === 0) {
    return null;
  }
  const pattern = allowPadding ? /^[A-Za-z0-9_-]+={0,2}$/ : /^[A-Za-z0-9_-]+$/;
  const match = pattern.exec(value);
  if (match === null || match[0] !== value) {
    return null;
  }

  const unpadded = value.replace(/=+$/, '');
  const paddingLength = value.length - unpadded.length;
  if (paddingLength > 0 && value.length % 4 !== 0) {
    return null;
  }
  if (unpadded.length % 4 === 1) {
    return null;
  }

  const standard = unpadded.replace(/-/g, '+').replace(/_/g, '/');
  const padded = standard + '='.repeat((4 - (standard.length % 4)) % 4);
  try {
    const decoded = atob(padded);
    return Uint8Array.from(decoded, (character) => character.charCodeAt(0));
  } catch {
    return null;
  }
}

function fingerprintGroups(value: string | null | undefined): string[] | null {
  const bytes = decodeBase64Url(value);
  if (bytes === null || bytes.length !== 32) {
    return null;
  }

  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0'))
    .join('')
    .toUpperCase();
  const groups: string[] = [];
  for (let index = 0; index < hex.length; index += 4) {
    groups.push(hex.slice(index, index + 4));
  }
  return groups;
}

function formatFpShort(bytes: Uint8Array): string {
  const alphabet = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';
  const characters: string[] = [];
  let buffer = 0;
  let bitCount = 0;

  for (const byte of bytes.subarray(0, 10)) {
    buffer = (buffer << 8) | byte;
    bitCount += 8;
    while (bitCount >= 5) {
      bitCount -= 5;
      characters.push(alphabet[(buffer >>> bitCount) & 0x1f] ?? '');
    }
    buffer &= (1 << bitCount) - 1;
  }

  return characters.join('').replace(/(.{4})(?=.)/g, '$1 ');
}

/**
 * Formats a full SPKI SHA-256 fingerprint for display as the first 10 bytes in
 * Crockford base32, grouped into four 4-character blocks (D39).
 */
export function formatFp(value: string | null | undefined): string {
  const bytes = decodeBase64Url(value);
  if (bytes === null || bytes.length !== 32) {
    return EM_DASH;
  }
  return formatFpShort(bytes);
}

export function formatFpFull(value: string | null | undefined): string {
  const groups = fingerprintGroups(value);
  return groups === null ? EM_DASH : groups.join(' ');
}

export function formatCode(code: string): string {
  return code.length === 6 && /^[0-9]{6}$/.test(code)
    ? `${code.slice(0, 3)} ${code.slice(3)}`
    : code;
}

export function formatElapsed(ms: number): string {
  const safeMs = Number.isFinite(ms) ? Math.max(0, ms) : 0;
  const totalSeconds = Math.floor(safeMs / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;

  if (hours > 0) {
    return `${hours}:${pad2(minutes)}:${pad2(seconds)}`;
  }
  return `${minutes}:${pad2(seconds)}`;
}

export function formatCountdown(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) {
    return '0:00';
  }
  return formatElapsed(Math.ceil(ms / 1000) * 1000);
}

export function secondsLeft(ms: number): number {
  return Math.max(0, Math.ceil(ms / 1000));
}

function sameLocalDay(left: Date, right: Date): boolean {
  return (
    left.getFullYear() === right.getFullYear() &&
    left.getMonth() === right.getMonth() &&
    left.getDate() === right.getDate()
  );
}

function formatClock(date: Date): string {
  return `${pad2(date.getHours())}:${pad2(date.getMinutes())}`;
}

export function formatWhen(us: number | null | undefined, nowMs: number): string {
  if (us === null || us === undefined) {
    return '';
  }

  const date = new Date(us / 1000);
  if (Number.isNaN(date.getTime())) {
    return '';
  }
  const now = new Date(nowMs);
  if (Number.isNaN(now.getTime())) {
    return '';
  }

  if (sameLocalDay(date, now)) {
    return `今天 ${formatClock(date)}`;
  }
  const previousDay = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1);
  if (sameLocalDay(date, previousDay)) {
    return `昨天 ${formatClock(date)}`;
  }
  if (date.getFullYear() === now.getFullYear()) {
    return `${date.getMonth() + 1}月${date.getDate()}日`;
  }
  return `${String(date.getFullYear()).padStart(4, '0')}-${pad2(date.getMonth() + 1)}-${pad2(
    date.getDate(),
  )}`;
}

export function formatDateTime(us: number): string {
  const date = new Date(us / 1000);
  if (Number.isNaN(date.getTime())) {
    return '';
  }
  return `${String(date.getFullYear()).padStart(4, '0')}-${pad2(date.getMonth() + 1)}-${pad2(
    date.getDate(),
  )} ${formatClock(date)}`;
}

export function formatDurationS(seconds: number | null | undefined): string {
  return seconds === null || seconds === undefined ? '' : formatElapsed(seconds * 1000);
}
