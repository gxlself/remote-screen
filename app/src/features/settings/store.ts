import { createContext, createElement, useCallback, useContext } from 'react';
import type { ReactNode } from 'react';
import { create, useStore } from 'zustand';
import type { StoreApi, UseBoundStore } from 'zustand';
import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import type { KvStore } from '../storage/kv';
import type { HostPrefs, SettingValue, SettingsKey } from './schema';
import { DEFAULT_HOST_PREFS, settingsSchema } from './schema';
import { isPlainObject, isSettingValue } from './domain';

type SettingsValues = {
  [K in SettingsKey]: SettingValue<K>;
};

type SettingsActions = {
  get<K extends SettingsKey>(key: K): SettingValue<K>;
  set<K extends SettingsKey>(key: K, value: SettingValue<K>): void;
  setHostPrefs(hostId: string, patch: Partial<HostPrefs>): void;
};

export type SettingsState = SettingsValues & SettingsActions;

export type SettingsStore = UseBoundStore<StoreApi<SettingsState>> &
  SettingsActions;

type CoreSettings = Pick<RsCoreApi, 'settings'>;

const SETTINGS_PREFIX = 'settings.';
let currentSettingsKv: KvStore | undefined;

/** Bind storage before core.init so boot and non-hook callers read the same settings. */
export function setSettingsStorage(kv: KvStore): void {
  currentSettingsKv = kv;
}

/** The storage bound above; Hub-off sync writes go here from non-hook callers. */
export function getSettingsStorage(): KvStore | undefined {
  return currentSettingsKv;
}

export function getSetting<K extends SettingsKey>(key: K): SettingValue<K> {
  return cloneValue(
    currentSettingsKv === undefined
      ? settingsSchema[key].def as SettingValue<K>
      : readValue(currentSettingsKv, key),
  );
}

function cloneValue<T>(value: T): T {
  if (Array.isArray(value)) {
    return value.map(item => cloneValue(item)) as T;
  }
  if (isPlainObject(value)) {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, cloneValue(item)]),
    ) as T;
  }
  return value;
}

function readValue<K extends SettingsKey>(
  kv: KvStore,
  key: K,
): SettingValue<K> {
  const fallback = settingsSchema[key].def;
  const stored = kv.getJSON<unknown>(`${SETTINGS_PREFIX}${key}`);
  return isSettingValue(key, stored)
    ? (stored as SettingValue<K>)
    : (fallback as SettingValue<K>);
}

function readAll(kv: KvStore): SettingsValues {
  const values = {} as SettingsValues;
  for (const key of Object.keys(settingsSchema) as SettingsKey[]) {
    (values as Record<SettingsKey, unknown>)[key] = readValue(kv, key);
  }
  return values;
}

export function createSettingsStore(
  kv: KvStore,
  core?: CoreSettings,
): SettingsStore {
  setSettingsStorage(kv);
  let warned = false;

  const push = (
    key: SettingsKey,
    value: unknown,
    tailnetEnabled: boolean,
  ) => {
    if (!('sync' in settingsSchema[key]) || core === undefined) {
      return;
    }
    try {
      // With Tailscale switched off the core must not start its embedded
      // node, whatever mode was picked back when it was on.
      core.settings.set(
        key,
        key === 'tailnet.mode' && !tailnetEnabled ? 'system' : value,
      );
    } catch {
      if (!warned) {
        warned = true;
        console.warn(`RsCore.settings.set 不支持 ${key}，设置只保存在本地`);
      }
    }
  };

  const initialValues = readAll(kv);
  const store = create<SettingsState>((setState, getState) => {
    const get = <K extends SettingsKey>(key: K): SettingValue<K> =>
      cloneValue(getState()[key]);
    const set = <K extends SettingsKey>(
      key: K,
      value: SettingValue<K>,
    ): void => {
      setState({ [key]: value });
      kv.setJSON(`${SETTINGS_PREFIX}${key}`, value);
      const enabled = getState()['tailnet.enabled'];
      push(key, value, enabled);
      if (key === 'tailnet.enabled') {
        push('tailnet.mode', getState()['tailnet.mode'], enabled);
      }
    };
    const setHostPrefs = (hostId: string, patch: Partial<HostPrefs>): void => {
      const current = getState()['hosts.prefs'][hostId] ?? {};
      const prefs = {
        ...getState()['hosts.prefs'],
        [hostId]: { ...current, ...patch },
      };
      setState({ 'hosts.prefs': prefs });
      kv.setJSON(`${SETTINGS_PREFIX}hosts.prefs`, prefs);
    };
    return { ...initialValues, get, set, setHostPrefs };
  });

  const settingsStore = Object.assign(store, {
    get: store.getState().get,
    set: store.getState().set,
    setHostPrefs: store.getState().setHostPrefs,
  }) as SettingsStore;

  for (const key of Object.keys(settingsSchema) as SettingsKey[]) {
    push(key, initialValues[key], initialValues['tailnet.enabled']);
  }

  return settingsStore;
}

const SettingsContext = createContext<SettingsStore | null>(null);

export function SettingsProvider(p: {
  store: SettingsStore;
  children: ReactNode;
}) {
  return createElement(
    SettingsContext.Provider,
    { value: p.store },
    p.children,
  );
}

export function useSettingsStore(): SettingsStore {
  const store = useContext(SettingsContext);
  if (store === null) {
    throw new Error('useSettingsStore 必须在 SettingsProvider 里使用');
  }
  return store;
}

export function useSetting<K extends SettingsKey>(
  key: K,
): [SettingValue<K>, (v: SettingValue<K>) => void] {
  const store = useSettingsStore();
  const value = useStore(store, (state: SettingsState) => state[key]);
  const setter = useCallback(
    (v: SettingValue<K>) => store.set(key, v),
    [store, key],
  );
  return [value, setter];
}

export function useHostPrefs(
  hostId: string,
): [HostPrefs, (patch: Partial<HostPrefs>) => void] {
  const store = useSettingsStore();
  const prefs = useStore(
    store,
    (state: SettingsState) => state['hosts.prefs'][hostId],
  );
  const setter = useCallback(
    (patch: Partial<HostPrefs>) => store.setHostPrefs(hostId, patch),
    [store, hostId],
  );
  return [{ ...DEFAULT_HOST_PREFS, ...prefs }, setter];
}
