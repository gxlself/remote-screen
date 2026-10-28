import { useCallback, useState } from 'react';
import type { AdapterTarget } from '../../native/rscore/types';
import { useKv } from '../storage/StorageContext';
import type { KvStore } from '../storage/kv';

/** RDP 声明的显示器数量（1–4，默认 1）。 */
export const MONITOR_COUNTS = [1, 2, 3, 4] as const;
export type MonitorCount = (typeof MONITOR_COUNTS)[number];

/** 和保存的密码同一个键（Rust `AdapterTarget::store_key`）；只存本机，不同步。 */
function key(target: AdapterTarget): string {
  const id = target.hostId ?? `${target.addr}:${target.port ?? 3389}`;
  return `adapters.monitors.${target.protocol}:${id}`;
}

/** 这台电脑上次选的显示器数量；没选过、或是 VNC，都是 1。 */
export function rememberedMonitors(
  kv: KvStore,
  target: AdapterTarget,
): MonitorCount {
  if (target.protocol !== 'rdp') {
    return 1;
  }
  const value = kv.getJSON<unknown>(key(target));
  return MONITOR_COUNTS.find(count => count === value) ?? 1;
}

export function rememberMonitors(
  kv: KvStore,
  target: AdapterTarget,
  count: MonitorCount,
): void {
  kv.setJSON(key(target), count);
}

/** 按设备记住的显示器数量：会话工具栏和设备详情页用同一个值。 */
export function useMonitorCount(
  target: AdapterTarget,
): [MonitorCount, (count: MonitorCount) => void] {
  const kv = useKv();
  const [count, setCount] = useState(() => rememberedMonitors(kv, target));
  const save = useCallback(
    (next: MonitorCount) => {
      setCount(next);
      rememberMonitors(kv, target, next);
    },
    [kv, target],
  );
  return [count, save];
}
