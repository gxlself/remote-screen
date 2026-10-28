import { createMMKV } from 'react-native-mmkv';
import { EARLY_KEY, type EarlyStore } from './earlyCrash';

export function createMmkvEarlyStore(id = 'rs-crash'): EarlyStore | null {
  try {
    const mmkv = createMMKV({ id });
    return {
      read: () => mmkv.getString(EARLY_KEY),
      write: value => mmkv.set(EARLY_KEY, value),
      clear: () => mmkv.remove(EARLY_KEY),
    };
  } catch {
    return null;
  }
}
