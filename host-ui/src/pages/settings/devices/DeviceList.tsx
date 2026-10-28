import { useRef, type JSX, type KeyboardEvent } from 'react';

import { Button, DeviceGlyph, Tag, type TagTone } from '../../../components';
import { zh } from '../../../i18n';
import type { DeviceSummary, Permission } from '../../../ipc';
import { formatWhen, pathLabel } from '../../../lib';

import styles from './Devices.module.css';

interface DeviceListProps {
  devices: DeviceSummary[];
  now: number;
  selectedId: string | null;
  onSelect(deviceId: string): void;
  onPair(): void;
}

const PERMISSION_TONES: Readonly<Record<Permission, TagTone>> = {
  view_only: 'neutral',
  control: 'accent',
  unattended: 'relay',
};

function pathSuffix(paths: string[]): string {
  if (paths.length === 0) {
    return zh.devices.noPath;
  }
  if (paths.length === 1) {
    return zh.devices.onlyPath(pathLabel(paths[0]));
  }
  return '';
}

export function DeviceList({
  devices,
  now,
  selectedId,
  onSelect,
  onPair,
}: DeviceListProps): JSX.Element {
  const listRef = useRef<HTMLDivElement>(null);

  const handleKeyDown = (event: KeyboardEvent<HTMLButtonElement>, currentIndex: number): void => {
    const step = event.key === 'ArrowDown' ? 1 : event.key === 'ArrowUp' ? -1 : 0;
    if (step === 0 || devices.length === 0) {
      return;
    }

    event.preventDefault();
    const nextIndex = (currentIndex + step + devices.length) % devices.length;
    const next = devices[nextIndex];
    if (next === undefined) {
      return;
    }
    onSelect(next.device_id);
    const items = listRef.current?.querySelectorAll<HTMLButtonElement>('[role="listitem"]');
    items?.[nextIndex]?.focus();
  };

  return (
    <aside aria-label={zh.devices.listLabel} className={styles.listPane}>
      <header className={styles.listHeader}>
        <h2 className={styles.listTitle}>{zh.devices.title}</h2>
        <span className={styles.count}>{zh.devices.count(devices.length)}</span>
      </header>

      {devices.length === 0 ? (
        <p className={styles.empty}>{zh.devices.empty}</p>
      ) : (
        <div className={styles.list} ref={listRef} role="list">
          {devices.map((device, index) => {
            const selected = device.device_id === selectedId;
            const lastConnected =
              device.last_connected_at_us === null
                ? zh.devices.notConnected
                : formatWhen(device.last_connected_at_us, now);
            const tone = PERMISSION_TONES[device.permission];

            return (
              <button
                aria-current={selected}
                className={[styles.deviceButton, selected ? styles.deviceSelected : undefined]
                  .filter(Boolean)
                  .join(' ')}
                data-device-id={device.device_id}
                key={device.device_id}
                onClick={() => onSelect(device.device_id)}
                onKeyDown={(event) => handleKeyDown(event, index)}
                role="listitem"
                type="button"
              >
                <DeviceGlyph platform="unknown" />
                <span className={styles.deviceCopy}>
                  <span className={styles.deviceTop}>
                    <span className={styles.deviceName}>{device.name}</span>
                    <span className={styles.lastConnected}>{lastConnected}</span>
                  </span>
                  <span className={styles.deviceId}>{device.device_id}</span>
                  <span className={styles.tagLine}>
                    <Tag tone={tone}>
                      {zh.devices.permissionTag[device.permission]}
                      {pathSuffix(device.paths)}
                    </Tag>
                  </span>
                </span>
              </button>
            );
          })}
        </div>
      )}

      <div className={styles.listFooter}>
        <Button variant="secondary" onClick={onPair}>
          {zh.devices.pairDevice}
        </Button>
        <p className={styles.pairHint}>{zh.devices.pairHint}</p>
      </div>
    </aside>
  );
}
