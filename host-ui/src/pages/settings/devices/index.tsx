import { useCallback, useState, type JSX } from 'react';

import { useIpc, useIpcQuery, type DeviceSummary, type Permission } from '../../../ipc';
import { useNow } from '../../../lib';
import { usePlatform } from '../../../platform';

import { DeviceDetail } from './DeviceDetail';
import { DeviceList } from './DeviceList';
import styles from './Devices.module.css';

interface DeviceChanges {
  permission: Permission;
  paths: string[];
}

const EMPTY_DEVICES: DeviceSummary[] = [];

export default function DevicesSection(): JSX.Element {
  const ipc = useIpc();
  const platform = usePlatform();
  const now = useNow();
  const devicesQuery = useIpcQuery('devices.list', { refetchOn: ['event.pairingResult'] });
  const settingsQuery = useIpcQuery('settings.get');
  const devices = devicesQuery.data?.devices ?? EMPTY_DEVICES;
  const { mutate: mutateDevices, refetch: refetchDevices } = devicesQuery;
  const [selectedId, setSelectedId] = useState<string | null>(null);

  const selectedDevice = devices.find((device) => device.device_id === selectedId) ?? devices[0];
  const intranetEnabled = settingsQuery.data?.intranet.enabled ?? false;

  const saveDevice = useCallback(
    async (deviceId: string, changes: DeviceChanges): Promise<void> => {
      const previousDevice = devices.find((device) => device.device_id === deviceId);
      mutateDevices((previous) => ({
        devices: (previous?.devices ?? []).map((device) =>
          device.device_id === deviceId ? { ...device, ...changes } : device,
        ),
      }));

      try {
        await ipc.call('devices.update', {
          device_id: deviceId,
          permission: changes.permission,
          paths: changes.paths,
        });
      } catch (error) {
        if (previousDevice !== undefined) {
          mutateDevices((current) => ({
            devices: (current?.devices ?? []).map((device) =>
              device.device_id === deviceId
                ? {
                    ...device,
                    permission: previousDevice.permission,
                    paths: previousDevice.paths,
                  }
                : device,
            ),
          }));
        }
        throw error;
      }
    },
    [devices, ipc, mutateDevices],
  );

  const revokeDevice = useCallback(
    async (deviceId: string): Promise<void> => {
      const remaining = devices.filter((device) => device.device_id !== deviceId);
      await ipc.call('devices.revoke', { device_id: deviceId });

      mutateDevices({ devices: remaining });
      setSelectedId(remaining[0]?.device_id ?? null);
      await refetchDevices();
    },
    [devices, ipc, mutateDevices, refetchDevices],
  );

  return (
    <section className={styles.layout} data-section="devices">
      <DeviceList
        devices={devices}
        now={now}
        onPair={() => void platform.showWindow('pair', 'pair')}
        onSelect={setSelectedId}
        selectedId={selectedDevice?.device_id ?? null}
      />

      {selectedDevice === undefined ? null : (
        <DeviceDetail
          device={selectedDevice}
          intranetEnabled={intranetEnabled}
          key={selectedDevice.device_id}
          now={now}
          onRevoke={() => revokeDevice(selectedDevice.device_id)}
          onSave={(changes) => saveDevice(selectedDevice.device_id, changes)}
        />
      )}
    </section>
  );
}
