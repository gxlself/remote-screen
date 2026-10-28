import { createMMKV } from 'react-native-mmkv';
import type { KvStore } from './kv';

export function createMmkvKv(id = 'rs-app'): KvStore {
  const mmkv = createMMKV({ id });

  return {
    getString(k) {
      return mmkv.getString(k);
    },
    set(k, v) {
      mmkv.set(k, v);
    },
    delete(k) {
      mmkv.remove(k);
    },
    getJSON<T>(k: string): T | undefined {
      const raw = mmkv.getString(k);
      if (raw === undefined) {
        return undefined;
      }
      try {
        return JSON.parse(raw) as T;
      } catch {
        return undefined;
      }
    },
    setJSON(k, v) {
      mmkv.set(k, JSON.stringify(v));
    },
    keys() {
      return mmkv.getAllKeys();
    },
  };
}
