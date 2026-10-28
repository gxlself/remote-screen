import { RsCoreError } from './RsCoreApi';
import type { CoreErrorTag } from './bindings';
import type { SyncValue } from './types';

const kinds: Record<CoreErrorTag, RsCoreError['kind']> = {
  Network: 'network',
  Http: 'http',
  Conflict: 'conflict',
  Unsupported: 'unsupported',
  Cancelled: 'cancelled',
  Internal: 'internal',
};

const record = (value: unknown): Record<string, unknown> | undefined =>
  typeof value === 'object' && value !== null
    ? (value as Record<string, unknown>)
    : undefined;

export function toRsCoreError(e: unknown): RsCoreError {
  if (e instanceof RsCoreError) {
    return e;
  }
  const error = record(e);
  const tag = error?.tag;
  if (
    typeof tag === 'string' &&
    Object.prototype.hasOwnProperty.call(kinds, tag)
  ) {
    const inner = record(error?.inner);
    const message = String(inner?.message ?? error?.message ?? e);
    if (tag === 'Http') {
      return new RsCoreError(
        'http',
        message,
        inner?.status as number | undefined,
        (inner?.hubCode ?? inner?.hub_code) as string | undefined,
      );
    }
    if (tag === 'Conflict') {
      let current: SyncValue | undefined;
      const json = inner?.currentJson ?? inner?.current_json;
      if (typeof json === 'string') {
        try {
          current = JSON.parse(json) as SyncValue;
        } catch {
          // A malformed conflict body must not hide the original conflict.
        }
      }
      return new RsCoreError('conflict', message, 409, 'conflict', current);
    }
    return new RsCoreError(kinds[tag as CoreErrorTag], message);
  }
  return new RsCoreError('internal', String(error?.message ?? e));
}

const MAX_U64 = 18446744073709551615n;

export function toNativeId(id: string): bigint {
  if (!/^\d+$/.test(id) || id.trim() !== id) {
    throw new RsCoreError('internal', `bad id ${id}`);
  }
  const nativeId = BigInt(id);
  if (nativeId > MAX_U64) {
    throw new RsCoreError('internal', `bad id ${id}`);
  }
  return nativeId;
}

export function fromNativeId(id: bigint): string {
  return id.toString();
}
