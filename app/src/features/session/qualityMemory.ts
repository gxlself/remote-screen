import type { Quality } from '../../native/rscore/types';
import type { KvStore } from '../storage/kv';

const QUALITIES: readonly Quality[] = ['auto', 'clear', 'smooth', 'eco'];

function key(hostId: string): string {
  return `session.quality.${hostId}`;
}

/** 这台电脑上次在会话里选的画质；没选过返回 null，按设置里的默认画质连接。 */
export function rememberedQuality(kv: KvStore, hostId: string): Quality | null {
  const value = kv.getString(key(hostId));
  return QUALITIES.find(quality => quality === value) ?? null;
}

export function rememberQuality(
  kv: KvStore,
  hostId: string,
  quality: Quality,
): void {
  kv.set(key(hostId), quality);
}
