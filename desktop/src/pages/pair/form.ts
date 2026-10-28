import type { PairOutcome } from '@app/native/rscore/types';

export type PairMethod = 'address' | 'file';

/** A computer is added by address (with its pairing code) or with a pairing file. */
export const PAIR_METHODS: readonly PairMethod[] = ['address', 'file'];

export function codeDigits(input: string): string {
  return input.replace(/\D/g, '').slice(0, 6);
}

export function codeComplete(code: string): boolean {
  return /^\d{6}$/.test(code);
}

/** Empty = default port; otherwise 1–65535. Everything else about the address is Rust's call. */
export function portValid(port: string): boolean {
  const value = port.trim();
  if (value === '') {
    return true;
  }
  return /^\d{1,5}$/.test(value) && Number(value) >= 1 && Number(value) <= 65535;
}

export function addressTarget(
  addr: string,
  port: string,
  code: string,
): { addr: string; port?: number; code: string } {
  const value = port.trim();
  return value === ''
    ? { addr: addr.trim(), code }
    : { addr: addr.trim(), port: Number(value), code };
}

export function outcomeTone(outcome: PairOutcome): 'good' | 'bad' | 'info' {
  return outcome.status === 'ok' ? 'good' : outcome.status === 'mitm' ? 'bad' : 'info';
}

/** Local `YYYY-MM-DD HH:mm` for a Unix-microsecond timestamp. */
export function formatTime(us: number): string {
  const date = new Date(us / 1000);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
